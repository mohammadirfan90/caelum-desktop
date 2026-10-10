//! Vehicle bridge task and commands (M9 integration; F-ENV-01..03, F-GND-11, R-INT-04, CR-015).
//!
//! Every 10 s the task checks the signed vehicle MID table in force. When one exists (or a new
//! version is applied) it binds the read-only receiver and feeds each datagram's readings to the
//! station synchronously; the window gets the alarm bus and value-free change events. With no
//! table there is no receiver and the Environment screen shows NO DATA.

use std::sync::Mutex;
use std::time::Duration;

use caelum_core::vehicle::{ClockState, RecvError, VehicleReceiver};
use caelum_domain::time::{LeapSeconds, MetEpoch, TaiNs};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event as _;

use crate::commands::{CommandError, ErrorKind};
use crate::events::{bus_events, ObservationsChanged};
use crate::state::AppState;

const CHECK: Duration = Duration::from_secs(10);

/// Link health shown to technical roles (no physiology).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct VehicleLink {
    /// A signed MID table is in force and the receiver is bound.
    pub running: bool,
    /// MID table version.
    pub version: Option<String>,
    /// Why the receiver is not running (bind failure, no table).
    pub problem: Option<String>,
    /// Datagrams received.
    pub datagrams: u32,
    /// Readings decoded.
    pub readings: u32,
    /// Drops by reason code.
    pub rejected: Vec<(String, u32)>,
    /// Vehicle clock state: `not_configured`, `unknown`, `set`, `not_set`, `stale`.
    pub clock: Option<String>,
    /// Latest receipt − vehicle time, ms.
    pub latest_offset_ms: Option<f64>,
}

/// Shared status of the receiver task.
#[derive(Default)]
pub struct VehicleState(Mutex<VehicleLink>);

impl VehicleState {
    /// The current status (default when the lock is poisoned: shown as not running).
    pub fn snapshot(&self) -> VehicleLink {
        self.0.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

fn set_status(app: &AppHandle, f: impl FnOnce(&mut VehicleLink)) {
    if let Ok(mut s) = app.state::<VehicleState>().0.lock() {
        f(&mut s);
    }
}

fn clock_name(c: ClockState) -> &'static str {
    match c {
        ClockState::NotConfigured => "not_configured",
        ClockState::Unknown => "unknown",
        ClockState::Set => "set",
        ClockState::NotSet => "not_set",
        ClockState::Stale => "stale",
    }
}

/// Starts the vehicle bridge task (call once in `setup`).
pub fn start_vehicle_bridge(app: AppHandle) {
    app.manage(VehicleState::default());
    tauri::async_runtime::spawn(async move {
        loop {
            let config = app
                .state::<AppState>()
                .with(|s| Ok(s.vehicle_config()?))
                .ok()
                .flatten();
            let Some(config) = config else {
                set_status(&app, |s| *s = VehicleLink::default());
                tokio::time::sleep(CHECK).await;
                continue;
            };
            let version = config.version.clone();
            let Ok(leap) = LeapSeconds::bundled() else {
                set_status(&app, |s| {
                    s.problem = Some("leap-second table unavailable".to_owned())
                });
                tokio::time::sleep(CHECK).await;
                continue;
            };
            // MET needs the signed mission epoch (R-OPS-05), which does not exist yet: the
            // receiver's MET output is discarded, never stored or shown.
            let met = MetEpoch(TaiNs(0));
            let clock_app = app.clone();
            let clock = move || {
                clock_app
                    .state::<AppState>()
                    .with(|s| s.now_tai().map_err(Into::into))
                    .ok()
            };
            let mut rx = match VehicleReceiver::bind(config, leap, met, clock).await {
                Ok(rx) => rx,
                Err(e) => {
                    let msg = e.to_string();
                    set_status(&app, |s| {
                        s.running = false;
                        s.version = Some(version.clone());
                        s.problem = Some(msg);
                    });
                    tokio::time::sleep(CHECK).await;
                    continue;
                }
            };
            set_status(&app, |s| {
                *s = VehicleLink {
                    running: true,
                    version: Some(version.clone()),
                    ..VehicleLink::default()
                }
            });
            // Receive until a new table version is applied (checked between datagrams).
            loop {
                match tokio::time::timeout(CHECK, rx.recv()).await {
                    Ok(Ok(decoded)) => {
                        if let Ok(done) = app
                            .state::<AppState>()
                            .with(|s| Ok(s.ingest_vehicle(&decoded.readings)?))
                        {
                            for e in bus_events(&done.output) {
                                let _ = e.emit(&app);
                            }
                            for crew in done.crews {
                                let _ = ObservationsChanged { crew_id: crew }.emit(&app);
                            }
                        }
                    }
                    Ok(Err(RecvError::NoStationTime)) | Err(_) => {}
                    Ok(Err(RecvError::Io(e))) => {
                        let msg = e.to_string();
                        set_status(&app, |s| s.problem = Some(msg));
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
                let now = app
                    .state::<AppState>()
                    .with(|s| s.now_tai().map_err(Into::into))
                    .ok();
                let stats = rx.stats().clone();
                let corr = now.map(|n| rx.correlation(n));
                set_status(&app, |s| {
                    s.datagrams = u32::try_from(stats.datagrams).unwrap_or(u32::MAX);
                    s.readings = u32::try_from(stats.readings).unwrap_or(u32::MAX);
                    s.rejected = stats
                        .rejected
                        .iter()
                        .map(|(k, v)| ((*k).to_owned(), u32::try_from(*v).unwrap_or(u32::MAX)))
                        .collect();
                    s.clock = corr.as_ref().map(|c| clock_name(c.clock).to_owned());
                    s.latest_offset_ms = corr
                        .as_ref()
                        .and_then(|c| c.latest_offset_ns)
                        .map(|o| o as f64 / 1e6);
                });
                let current = app
                    .state::<AppState>()
                    .with(|s| Ok(s.vehicle_config()?))
                    .ok()
                    .flatten()
                    .map(|c| c.version);
                if current.as_deref() != Some(version.as_str()) {
                    break;
                }
            }
        }
    });
}

/// Applies a signed vehicle MID table (administrator, re-authentication).
#[tauri::command]
#[specta::specta]
pub async fn config_apply_vehicle(
    state: State<'_, AppState>,
    bundle: String,
    signature: String,
) -> Result<String, CommandError> {
    state.with(|s| Ok(s.config_apply_vehicle(bundle.as_bytes(), &signature)?))
}

/// Vehicle bridge health (technical roles).
#[tauri::command]
#[specta::specta]
pub async fn vehicle_link_status(
    state: State<'_, AppState>,
    vehicle: State<'_, VehicleState>,
) -> Result<VehicleLink, CommandError> {
    state.with(|s| Ok(s.vehicle_link_allowed()?))?;
    vehicle
        .0
        .lock()
        .map(|s| s.clone())
        .map_err(|_| CommandError {
            kind: ErrorKind::Failed,
            message: "vehicle status unavailable".to_owned(),
            retry_after_s: None,
        })
}
