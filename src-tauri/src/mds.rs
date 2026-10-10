//! Medical data system task and commands (F-GND-12, F-GND-13, F-GND-14, SR-016; CR-016).
//!
//! The service runs on its own thread and its own database connection, so network waits never
//! hold the station lock or delay alarms. Each round: follow the signed configuration in force,
//! build the mutual-TLS transport from it and the client identity, enqueue closed summary
//! windows, send and read back; read-back observations then go through the station (stored as
//! imported and evaluated), and the outage alarm is raised or cleared.

use std::sync::Mutex;
use std::time::Duration;

use caelum_core::mds::{MdsService, MdsStatus};
use caelum_core::mds_transport::{HttpsConfig, HttpsTransport};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event as _;
use zeroize::Zeroizing;

use crate::commands::CommandError;
use crate::events::{bus_events, ObservationsChanged};
use crate::state::AppState;

const ROUND: Duration = Duration::from_secs(5);

/// Link status (counts only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MdsLink {
    /// Configured with a signed configuration and a client identity.
    pub configured: bool,
    /// Configuration version.
    pub version: Option<String>,
    /// `connected` / `degraded` / `unreachable` / `not_configured`.
    pub health: String,
    /// Records not yet acknowledged.
    pub pending: u32,
    /// Records held for review.
    pub held: u32,
    /// Why it is not running.
    pub problem: Option<String>,
}

impl From<MdsStatus> for MdsLink {
    fn from(s: MdsStatus) -> Self {
        Self {
            configured: s.configured,
            version: s.version,
            health: s.health.to_owned(),
            pending: u32::try_from(s.pending).unwrap_or(u32::MAX),
            held: u32::try_from(s.held).unwrap_or(u32::MAX),
            problem: s.problem,
        }
    }
}

/// Shared status of the task.
pub struct MdsState(Mutex<MdsLink>);

impl Default for MdsState {
    fn default() -> Self {
        Self(Mutex::new(MdsLink {
            configured: false,
            version: None,
            health: "not_configured".to_owned(),
            pending: 0,
            held: 0,
            problem: None,
        }))
    }
}

/// Starts the medical data system task with its own service (call once in `setup`).
pub fn start_mds(app: AppHandle, service: Option<MdsService>) {
    app.manage(MdsState::default());
    let Some(mut service) = service else { return };
    std::thread::spawn(move || loop {
        std::thread::sleep(ROUND);
        let Ok(now) = app
            .state::<AppState>()
            .with(|s| s.now_tai().map_err(Into::into))
        else {
            continue;
        };
        if let Some(config) = service.refresh(now) {
            let transport = match service.identity() {
                Ok(Some((cert, key))) => HttpsTransport::new(HttpsConfig {
                    base_url: config.base_url.clone(),
                    ca_pem: config.ca_pem.clone().into_bytes(),
                    client_cert_pem: cert.into_bytes(),
                    client_key_pem: Zeroizing::new(key.as_bytes().to_vec()),
                    timeout: Duration::from_millis(config.timeout_ms),
                })
                .ok(),
                _ => None,
            };
            if let Some(mut transport) = transport {
                let outcome = service.sync(now, &mut transport);
                let state = app.state::<AppState>();
                for obs in &outcome.inbound {
                    if let Ok((out, crew)) = state.with(|s| Ok(s.ingest_imported(obs)?)) {
                        for e in bus_events(&out) {
                            let _ = e.emit(&app);
                        }
                        if let Some(crew_id) = crew {
                            let _ = ObservationsChanged { crew_id }.emit(&app);
                        }
                    }
                }
                if let Some(active) = outcome.technical_alarm {
                    let _ = state.with(|s| Ok(s.mds_technical(active)?));
                }
            }
        }
        if let Ok(mut s) = app.state::<MdsState>().0.lock() {
            *s = service.status(now).into();
        }
    });
}

/// Applies the signed medical data system configuration (administrator, re-authentication).
#[tauri::command]
#[specta::specta]
pub async fn config_apply_mds(
    state: State<'_, AppState>,
    bundle: String,
    signature: String,
) -> Result<String, CommandError> {
    state.with(|s| Ok(s.config_apply_mds(bundle.as_bytes(), &signature)?))
}

/// Sets this station's client certificate and key (PEM; administrator, re-authentication).
#[tauri::command]
#[specta::specta]
pub async fn mds_identity_set(
    state: State<'_, AppState>,
    cert_pem: String,
    key_pem: String,
) -> Result<(), CommandError> {
    let key = Zeroizing::new(key_pem);
    state.with(|s| Ok(s.mds_identity_set(&cert_pem, &key)?))
}

/// Medical data system link status (counts only).
#[tauri::command]
#[specta::specta]
pub async fn mds_status(
    state: State<'_, AppState>,
    mds: State<'_, MdsState>,
) -> Result<MdsLink, CommandError> {
    state.with(|s| Ok(s.mds_status_allowed()?))?;
    Ok(mds.0.lock().map(|s| s.clone()).unwrap_or_default())
}

impl Default for MdsLink {
    fn default() -> Self {
        Self {
            configured: false,
            version: None,
            health: "not_configured".to_owned(),
            pending: 0,
            held: 0,
            problem: Some("status unavailable".to_owned()),
        }
    }
}

/// Result of an uplink.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UplinkOutcome {
    /// Verified version.
    pub version: String,
    /// Takes effect at the next start (rulesets).
    pub at_next_start: bool,
}

/// Verifies and applies an uplinked item (administrator, re-authentication; F-GND-07).
#[tauri::command]
#[specta::specta]
pub async fn uplink_apply(
    state: State<'_, AppState>,
    kind: String,
    content: String,
    signature: String,
) -> Result<UplinkOutcome, CommandError> {
    state.with(|s| {
        let r = s.uplink_apply(&kind, content.as_bytes(), &signature)?;
        Ok(UplinkOutcome {
            version: r.version,
            at_next_start: r.at_next_start,
        })
    })
}
