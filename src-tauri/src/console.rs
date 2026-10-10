//! Ground console profile in the shell (F-GND-08, F-GND-04; CR-025): the downlink receiver, the
//! CFDP acknowledgements back toward the onboard instance, the Open MCT gateway (`/history`,
//! `/ws`), and the flight-surgeon commands.
//!
//! The gateway carries OPS fields only (no measured value, parameter name, rule ID or free text;
//! R-SEC-06). It binds only to the configured address on the mission-control console network.

use std::net::SocketAddr;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State as AxState};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use caelum_core::station::console::ConsoleConfig;
use caelum_domain::ids::CrewMemberId;
use caelum_domain::time::{LeapSeconds, TaiNs};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager, State};
use tokio::sync::broadcast;

use crate::commands::{CommandError, ErrorKind};
use crate::reads::utc_ms;
use crate::state::AppState;

/// One downlinked summary.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GroundSummary {
    /// Parameter key.
    pub parameter: String,
    /// Window end, UTC ms.
    pub window_end_utc_ms: Option<f64>,
    /// Worst quality in the window.
    pub quality: String,
    /// Minimum.
    pub min: Option<f64>,
    /// Mean.
    pub mean: Option<f64>,
    /// Maximum.
    pub max: Option<f64>,
    /// Last state (enumerated parameters).
    pub state: Option<String>,
    /// Readings in the window.
    pub count: u32,
}

/// One received file.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GroundFile {
    /// ID.
    pub id: String,
    /// Name as sent.
    pub name: String,
    /// `event_packet` or `file`.
    pub kind: String,
    /// Received, UTC ms.
    pub received_utc_ms: Option<f64>,
}

/// One private medical conference record.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Pmc {
    /// ID.
    pub id: String,
    /// Author (user ID).
    pub author: String,
    /// Held, UTC ms.
    pub held_utc_ms: Option<f64>,
    /// Notes.
    pub notes: String,
    /// Signed.
    pub signed: bool,
}

fn crew(id: &str) -> Result<CrewMemberId, CommandError> {
    id.parse().map_err(|_| CommandError {
        kind: ErrorKind::Rejected,
        message: "not a crew member ID".to_owned(),
        retry_after_s: None,
    })
}

fn tai_of_utc_ms(ms: f64) -> Result<TaiNs, CommandError> {
    let leap = LeapSeconds::bundled().map_err(|e| CommandError {
        kind: ErrorKind::Failed,
        message: e.to_string(),
        retry_after_s: None,
    })?;
    leap.utc_to_tai((ms * 1e6) as i64)
        .map(|(t, _)| t)
        .map_err(|e| CommandError {
            kind: ErrorKind::Rejected,
            message: e.to_string(),
            retry_after_s: None,
        })
}

/// Applies the signed ground-console configuration (administrator, re-authentication).
#[tauri::command]
#[specta::specta]
pub async fn config_apply_console(
    state: State<'_, AppState>,
    bundle: String,
    signature: String,
) -> Result<String, CommandError> {
    state.with(|s| Ok(s.config_apply_console(bundle.as_bytes(), &signature)?))
}

/// Downlinked summaries of a crew member over the last `window_s` (flight surgeon).
#[tauri::command]
#[specta::specta]
pub async fn console_summaries(
    state: State<'_, AppState>,
    crew_member_id: String,
    window_s: u32,
) -> Result<Vec<GroundSummary>, CommandError> {
    let c = crew(&crew_member_id)?;
    let rows = state.with(|s| Ok(s.console_summaries(c, window_s)?))?;
    Ok(rows
        .into_iter()
        .map(|r| GroundSummary {
            parameter: r.parameter,
            window_end_utc_ms: utc_ms(r.window_end),
            quality: r.quality,
            min: r.min_value,
            mean: r.mean_value,
            max: r.max_value,
            state: r.state,
            count: u32::try_from(r.count).unwrap_or(u32::MAX),
        })
        .collect())
}

/// Received files (flight surgeon).
#[tauri::command]
#[specta::specta]
pub async fn console_files(state: State<'_, AppState>) -> Result<Vec<GroundFile>, CommandError> {
    let rows = state.with(|s| Ok(s.console_files()?))?;
    Ok(rows
        .into_iter()
        .map(|r| GroundFile {
            id: r.id,
            name: r.name,
            kind: r.kind,
            received_utc_ms: utc_ms(r.received_at),
        })
        .collect())
}

/// The content of a received file, as text (event packets are JSON; flight surgeon).
#[tauri::command]
#[specta::specta]
pub async fn console_file(state: State<'_, AppState>, id: String) -> Result<String, CommandError> {
    let bytes = state.with(|s| Ok(s.console_file(&id)?))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Downlinked OPS records of the last `window_s` (flight surgeon, BME, MCC), as JSON text.
#[tauri::command]
#[specta::specta]
pub async fn console_ops(
    state: State<'_, AppState>,
    kind: String,
    window_s: u32,
) -> Result<Vec<String>, CommandError> {
    let rows = state.with(|s| Ok(s.console_ops(&kind, window_s)?))?;
    Ok(rows.into_iter().map(|v| v.to_string()).collect())
}

/// Drafts or edits a private medical conference record (flight surgeon, author).
#[tauri::command]
#[specta::specta]
pub async fn pmc_save(
    state: State<'_, AppState>,
    id: Option<String>,
    crew_member_id: String,
    held_utc_ms: f64,
    notes: String,
) -> Result<String, CommandError> {
    let c = crew(&crew_member_id)?;
    let held = tai_of_utc_ms(held_utc_ms)?;
    state.with(|s| Ok(s.pmc_save(id.as_deref(), c, held, &notes)?))
}

/// Signs a private medical conference record (author, re-authentication).
#[tauri::command]
#[specta::specta]
pub async fn pmc_sign(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    state.with(|s| Ok(s.pmc_sign(&id)?))
}

/// A crew member's private medical conference records (flight surgeon).
#[tauri::command]
#[specta::specta]
pub async fn pmc_list(
    state: State<'_, AppState>,
    crew_member_id: String,
) -> Result<Vec<Pmc>, CommandError> {
    let c = crew(&crew_member_id)?;
    let rows = state.with(|s| Ok(s.pmc_list(c)?))?;
    Ok(rows
        .into_iter()
        .map(|r| Pmc {
            id: r.id,
            author: r.author,
            held_utc_ms: utc_ms(r.held_at),
            notes: r.notes,
            signed: r.signed_at.is_some(),
        })
        .collect())
}

#[derive(Clone)]
struct Gateway {
    app: AppHandle,
    live: broadcast::Sender<String>,
}

#[derive(Deserialize)]
struct HistoryQuery {
    #[serde(rename = "type")]
    kind: String,
    start: f64,
    end: f64,
}

async fn history(AxState(g): AxState<Gateway>, Query(q): Query<HistoryQuery>) -> Response {
    let (Ok(from), Ok(to)) = (tai_of_utc_ms(q.start), tai_of_utc_ms(q.end)) else {
        return (axum::http::StatusCode::BAD_REQUEST, "bad time range").into_response();
    };
    match g
        .app
        .state::<AppState>()
        .with(|s| Ok(s.console_history(&q.kind, from, to)?))
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (axum::http::StatusCode::BAD_REQUEST, e.message).into_response(),
    }
}

async fn ws(AxState(g): AxState<Gateway>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| stream(socket, g.live.subscribe()))
}

async fn stream(mut socket: WebSocket, mut live: broadcast::Receiver<String>) {
    loop {
        match live.recv().await {
            Ok(text) => {
                if socket.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Starts the ground console if this installation is configured as one (call once in `setup`).
pub fn start_console(app: AppHandle) {
    let config: Option<ConsoleConfig> = app
        .state::<AppState>()
        .with(|s| Ok(s.console_config()?))
        .ok()
        .flatten();
    let Some(config) = config else { return };
    let (live, _) = broadcast::channel::<String>(256);
    let gateway = Gateway {
        app: app.clone(),
        live: live.clone(),
    };
    let addr: SocketAddr = config.gateway;
    tauri::async_runtime::spawn(async move {
        let router = Router::new()
            .route("/history", get(history))
            .route("/ws", get(ws))
            .with_state(gateway);
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => {
                if let Err(e) = axum::serve(listener, router).await {
                    tracing::error!(error = %e, "Open MCT gateway stopped");
                }
            }
            Err(e) => tracing::error!(error = %e, %addr, "Open MCT gateway cannot listen"),
        }
    });
    let (receive, ack_to) = (config.receive, config.ack_to);
    tauri::async_runtime::spawn(async move {
        let socket = match tokio::net::UdpSocket::bind(receive).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, %receive, "downlink receiver cannot listen");
                return;
            }
        };
        let mut buf = vec![0_u8; 65_536];
        loop {
            let recv =
                tokio::time::timeout(std::time::Duration::from_millis(250), socket.recv(&mut buf))
                    .await;
            if let Ok(Ok(n)) = recv {
                if let Ok(messages) = app
                    .state::<AppState>()
                    .with(|s| Ok(s.console_ingest(&buf[..n])?))
                {
                    for m in messages {
                        let _ = live.send(m.to_string());
                    }
                }
            }
            // CFDP acknowledgements back toward the onboard instance.
            if let Ok(pdus) = app.state::<AppState>().with(|s| Ok(s.console_cfdp_poll()?)) {
                for pdu in pdus {
                    let _ = socket.send_to(&pdu, ack_to).await;
                }
            }
        }
    });
}
