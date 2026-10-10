//! Crew-station client (F-SYS-05, F-SYS-13; CR-017 §4–5, docs/integration/M11.md item 5).
//!
//! With a signed `crew-station` configuration, this installation is a secondary display or a
//! kiosk on the crew LAN, not a node. It follows whichever node of the pair reports itself as
//! primary (pinned certificates, mutual TLS 1.3). A secondary display signs users in on the
//! primary and shows the alarms the primary streams to that user. A kiosk shows the crew-wide
//! emergency view without sign-in (SR-014). Everything shown is as the primary projected it per
//! role; nothing is computed here, and a lost link shows as NO DATA, never as the last state.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use caelum_cluster::lan::access::AlarmView;
use caelum_cluster::lan::client::{connect, request, websocket};
use caelum_cluster::lan::server::StreamMessage;
use caelum_cluster::lan::tls::TlsMaterial;
use caelum_cluster::lan::Fingerprint;
use caelum_core::cluster::{crew_station_in_force, identity};
use caelum_core::security::keys::{database_key, IfMissing};
use caelum_core::security::keystore::OsKeyStore;
use caelum_core::station::DATABASE_FILE;
use caelum_core::store::Database;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event as _;
use tokio_tungstenite::tungstenite::Message;

use crate::commands::{CommandError, ErrorKind};

const RETRY: Duration = Duration::from_secs(2);

/// One alarm as the primary serves it to this station.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAlarm {
    /// Alarm ID.
    pub alarm_id: String,
    /// Crew member.
    pub crew_id: String,
    /// `critical` / `warning` / `observe`.
    pub priority: String,
    /// Category.
    pub category: String,
    /// State.
    pub state: String,
    /// Escalation stage.
    pub stage: u8,
    /// Title, when the viewer may see the detail.
    pub title: Option<String>,
}

/// This crew station's view.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CrewLanView {
    /// A signed crew-station configuration is in force.
    pub configured: bool,
    /// A kiosk (emergency view without sign-in) rather than a secondary display.
    pub kiosk: bool,
    /// Streaming from the primary now.
    pub connected: bool,
    /// The user signed in on the primary from here.
    pub signed_in: Option<String>,
    /// Why the station shows NO DATA.
    pub problem: Option<String>,
    /// Alarms as served (empty when not connected: never the last known state).
    pub alarms: Vec<RemoteAlarm>,
}

/// The view changed (value-free: windows call `crew_lan_status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct CrewLanChanged;

struct Link {
    tls: TlsMaterial,
    nodes: Vec<(SocketAddr, String, Fingerprint)>,
    kiosk: bool,
}

#[derive(Default)]
struct Inner {
    link: Option<Arc<Link>>,
    primary: Option<(SocketAddr, String, Fingerprint)>,
    token: Option<String>,
    user: Option<String>,
    alarms: BTreeMap<String, RemoteAlarm>,
    connected: bool,
    problem: Option<String>,
}

/// Tauri-managed crew-station state.
#[derive(Default)]
pub struct CrewLanState(Mutex<Inner>);

impl CrewLanState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn view(&self) -> CrewLanView {
        let i = self.lock();
        CrewLanView {
            configured: i.link.is_some(),
            kiosk: i.link.as_ref().is_some_and(|l| l.kiosk),
            connected: i.connected,
            signed_in: i.user.clone(),
            problem: i.problem.clone(),
            alarms: if i.connected {
                i.alarms.values().cloned().collect()
            } else {
                Vec::new()
            },
        }
    }
}

fn remote(a: &AlarmView) -> RemoteAlarm {
    let o = &a.ops;
    let name = |v: &serde_json::Value| v.as_str().map(str::to_owned);
    RemoteAlarm {
        alarm_id: o.alarm_id.to_string(),
        crew_id: o.crew_id.to_string(),
        priority: serde_json::to_value(o.priority)
            .ok()
            .as_ref()
            .and_then(name)
            .unwrap_or_default(),
        category: o.category.clone(),
        state: o.state.clone(),
        stage: o.stage,
        title: a
            .detail
            .as_ref()
            .and_then(|d| d.get("title"))
            .and_then(name),
    }
}

fn open_db(data_dir: &Path) -> Result<Database, String> {
    let keystore = OsKeyStore::open().map_err(|e| e.to_string())?;
    let (key, _) = database_key(&keystore, IfMissing::Fail).map_err(|e| e.to_string())?;
    Database::open(&data_dir.join(DATABASE_FILE), &key).map_err(|e| e.to_string())
}

fn prepare(data_dir: &Path) -> Result<Option<Link>, String> {
    let db = open_db(data_dir)?;
    let Some(plan) = crew_station_in_force(&db)? else {
        return Ok(None);
    };
    let (chain, key) = identity(&db)?
        .ok_or("the crew-LAN certificate and key are not set (cluster_identity_set)")?;
    let tls = TlsMaterial::from_pem(&plan.config.ca_pem, &chain, &key)
        .map_err(|e| format!("crew-LAN certificate: {e}"))?;
    Ok(Some(Link {
        tls,
        nodes: plan.nodes,
        kiosk: plan.kiosk,
    }))
}

fn changed(app: &AppHandle) {
    let _ = CrewLanChanged.emit(app);
}

/// The node that reports itself as primary, if any answers.
async fn find_primary(link: &Link) -> Option<(SocketAddr, String, Fingerprint)> {
    let config = link.tls.client_config().ok()?;
    for (addr, name, pin) in &link.nodes {
        let Ok(stream) = connect(config.clone(), *addr, name, Some(pin)).await else {
            continue;
        };
        let Ok((status, body)) =
            request(stream, "GET".parse().ok()?, "/v1/status", None, None).await
        else {
            continue;
        };
        let role = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("role").and_then(|r| r.as_str()).map(str::to_owned));
        if status.is_success() && role.as_deref() == Some("primary") {
            return Some((*addr, name.clone(), *pin));
        }
    }
    None
}

/// Starts the crew-station client if this installation is configured as one (call in `setup`).
pub fn start_crew_lan(app: AppHandle, data_dir: &Path) {
    app.manage(CrewLanState::default());
    let link = match prepare(data_dir) {
        Ok(Some(l)) => Arc::new(l),
        Ok(None) => return,
        Err(reason) => {
            app.state::<CrewLanState>().lock().problem = Some(reason);
            return;
        }
    };
    app.state::<CrewLanState>().lock().link = Some(link.clone());
    tauri::async_runtime::spawn(async move {
        loop {
            run_once(&app, &link).await;
            {
                let state = app.state::<CrewLanState>();
                let mut i = state.lock();
                i.connected = false;
                i.alarms.clear();
            }
            changed(&app);
            tokio::time::sleep(RETRY).await;
        }
    });
}

async fn run_once(app: &AppHandle, link: &Link) {
    let Some(primary) = find_primary(link).await else {
        app.state::<CrewLanState>().lock().problem =
            Some("no primary answers on the crew LAN".to_owned());
        return;
    };
    let (path, token) = if link.kiosk {
        ("/v1/emergency/stream", None)
    } else {
        let token = {
            let state = app.state::<CrewLanState>();
            let mut i = state.lock();
            i.primary = Some(primary.clone());
            i.token.clone()
        };
        let Some(token) = token else {
            app.state::<CrewLanState>().lock().problem = Some("sign in to see alarms".to_owned());
            changed(app);
            return;
        };
        ("/v1/alarms/stream", Some(token))
    };
    let Ok(config) = link.tls.client_config() else {
        return;
    };
    let Ok(stream) = connect(config, primary.0, &primary.1, Some(&primary.2)).await else {
        return;
    };
    let Ok(mut ws) = websocket(stream, path, token.as_deref()).await else {
        return;
    };
    {
        let state = app.state::<CrewLanState>();
        let mut i = state.lock();
        i.primary = Some(primary);
        i.connected = true;
        i.problem = None;
    }
    changed(app);
    while let Some(Ok(msg)) = ws.next().await {
        let Message::Text(text) = msg else { continue };
        let Ok(m) = serde_json::from_str::<StreamMessage>(&text) else {
            continue;
        };
        {
            let state = app.state::<CrewLanState>();
            let mut i = state.lock();
            match m {
                StreamMessage::Snapshot { alarms } => {
                    i.alarms = alarms
                        .iter()
                        .map(|a| (a.ops.alarm_id.to_string(), remote(a)))
                        .collect();
                }
                StreamMessage::Update { alarm } => {
                    i.alarms
                        .insert(alarm.ops.alarm_id.to_string(), remote(&alarm));
                }
                StreamMessage::Closed { reason } => {
                    if reason == "session_ended" {
                        i.token = None;
                        i.user = None;
                    }
                    i.problem = Some(reason);
                    drop(i);
                    changed(app);
                    return;
                }
            }
        }
        changed(app);
    }
}

fn unavailable(message: &str) -> CommandError {
    CommandError {
        kind: ErrorKind::Unavailable,
        message: message.to_owned(),
        retry_after_s: None,
    }
}

/// This crew station's link and the alarms the primary serves it (public: the primary decides
/// what each viewer may see).
#[tauri::command]
#[specta::specta]
pub async fn crew_lan_status(state: State<'_, CrewLanState>) -> Result<CrewLanView, CommandError> {
    Ok(state.view())
}

/// Signs a user in on the primary from this station (the primary checks, throttles and audits).
#[tauri::command]
#[specta::specta]
pub async fn crew_lan_sign_in(
    state: State<'_, CrewLanState>,
    username: String,
    secret: String,
) -> Result<String, CommandError> {
    let secret = zeroize::Zeroizing::new(secret);
    let (link, primary) = {
        let i = state.lock();
        (i.link.clone(), i.primary.clone())
    };
    let link = link.ok_or_else(|| unavailable("this installation is not a crew station"))?;
    let primary = match primary {
        Some(p) => p,
        None => find_primary(&link)
            .await
            .ok_or_else(|| unavailable("no primary answers on the crew LAN"))?,
    };
    let config = link
        .tls
        .client_config()
        .map_err(|e| unavailable(&e.to_string()))?;
    let stream = connect(config, primary.0, &primary.1, Some(&primary.2))
        .await
        .map_err(|e| unavailable(&e.to_string()))?;
    let body = serde_json::to_vec(&serde_json::json!({ "username": username, "secret": &*secret }))
        .map_err(|e| unavailable(&e.to_string()))?;
    let (status, reply) = request(
        stream,
        "POST".parse().map_err(|_| unavailable("request"))?,
        "/v1/sessions",
        None,
        Some(body),
    )
    .await
    .map_err(|e| unavailable(&e.to_string()))?;
    if !status.is_success() {
        return Err(CommandError {
            kind: ErrorKind::InvalidCredentials,
            message: "the primary refused the sign-in".to_owned(),
            retry_after_s: None,
        });
    }
    let v: serde_json::Value =
        serde_json::from_slice(&reply).map_err(|e| unavailable(&e.to_string()))?;
    let token = v
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| unavailable("no session token"))?
        .to_owned();
    let mut i = state.lock();
    i.token = Some(token);
    i.user = Some(username.clone());
    i.primary = Some(primary);
    i.problem = None;
    Ok(username)
}

/// Signs the crew-station user out on the primary.
#[tauri::command]
#[specta::specta]
pub async fn crew_lan_sign_out(state: State<'_, CrewLanState>) -> Result<(), CommandError> {
    let (link, primary, token) = {
        let mut i = state.lock();
        let t = i.token.take();
        i.user = None;
        i.alarms.clear();
        (i.link.clone(), i.primary.clone(), t)
    };
    if let (Some(link), Some(p), Some(t)) = (link, primary, token) {
        if let Ok(config) = link.tls.client_config() {
            if let Ok(stream) = connect(config, p.0, &p.1, Some(&p.2)).await {
                let _ = request(
                    stream,
                    "DELETE".parse().map_err(|_| unavailable("request"))?,
                    "/v1/sessions",
                    Some(&t),
                    None,
                )
                .await;
            }
        }
    }
    Ok(())
}

/// Applies the signed crew-station configuration (administrator, re-authentication).
#[tauri::command]
#[specta::specta]
pub async fn config_apply_crew_station(
    state: State<'_, crate::state::AppState>,
    bundle: String,
    signature: String,
) -> Result<String, CommandError> {
    state.with(|s| Ok(s.config_apply_crew_station(bundle.as_bytes(), &signature)?))
}
