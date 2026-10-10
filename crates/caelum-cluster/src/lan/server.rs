//! The LAN server: mutual-TLS accept loop, station identification and the role-checked endpoints
//! (F-SYS-05, CLAUDE.md rule 6).

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use caelum_security::role::UserIdentity;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::service::TowerToHyperService;
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;

use super::access::{
    authorize, project, AlarmView, Endpoint, EndpointAccess, LanAlarm, LanDenied, Viewer,
};
use super::sessions::{LanSessions, SessionPolicy};
use super::{fingerprint, StationIdentity, StationRegistry};
use crate::failover::InstanceRole;
use crate::fence::{Fence, NodeId};
use crate::wire::{hex, MAX_FRAME};

/// TLS handshakes taking longer than this are dropped.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Streams re-check the session and the instance role this often.
const STREAM_RECHECK: Duration = Duration::from_secs(1);

/// A refused sign-in (the reason is audited by the authority, never disclosed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("sign-in refused")]
pub struct SignInRefused;

/// The alarm source is not available; the station shows NO DATA.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("alarm source unavailable")]
pub struct SourceUnavailable;

/// Security events the LAN service reports for the audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanAuditEvent {
    /// A user signed in from a station.
    SignedIn {
        /// Station.
        station: NodeId,
        /// Account.
        user: UserIdentity,
    },
    /// A sign-in was refused.
    SignInFailed {
        /// Station.
        station: NodeId,
        /// Claimed user name.
        username: String,
    },
    /// A user signed out.
    SignedOut {
        /// Station.
        station: NodeId,
        /// Account.
        user: UserIdentity,
    },
    /// The crew-wide emergency view was opened without sign-in (SR-014: audited and notified).
    EmergencyViewOpened {
        /// Station.
        station: NodeId,
    },
    /// A request was refused.
    Refused {
        /// Station.
        station: NodeId,
        /// Endpoint path.
        endpoint: &'static str,
        /// Why.
        reason: LanDenied,
    },
    /// A TLS client presented a CA-issued certificate that is not provisioned.
    UnknownStation {
        /// Certificate fingerprint, hex.
        fingerprint: String,
    },
}

/// Accounts and the audit log, implemented by the core on `caelum-security`.
pub trait UserAuthority: Send + Sync + 'static {
    /// Verifies a user's credentials (with the sign-in lockout of `caelum-security`). May take
    /// seconds (argon2); called on a blocking thread.
    fn sign_in(
        &self,
        station: &StationIdentity,
        username: &str,
        secret: &[u8],
    ) -> Result<UserIdentity, SignInRefused>;
    /// Whether the account is still active (not disabled).
    fn is_active(&self, user: &UserIdentity) -> bool;
    /// Records a security event.
    fn audit(&self, event: LanAuditEvent);
}

/// The alarm bus, implemented by the core.
pub trait AlarmSource: Send + Sync + 'static {
    /// Active alarms and those ended in the last 24 h (as `alarm_list`).
    fn snapshot(&self) -> Result<Vec<LanAlarm>, SourceUnavailable>;
    /// Every alarm change from now on.
    fn subscribe(&self) -> broadcast::Receiver<LanAlarm>;
}

/// The instance status served at `/v1/status` and shown in the status footer. No physiology.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusterStatus {
    /// This node.
    pub node: NodeId,
    /// Role.
    pub role: InstanceRole,
    /// The primary this node acts as or follows.
    pub primary: Option<Fence>,
    /// Highest fence seen.
    pub highest_seen: Option<Fence>,
    /// Last log sequence number.
    pub head_seq: u64,
    /// Primary only: highest sequence number the standby has persisted; `None` when no standby
    /// is connected (status NOT REPLICATED, never assumed).
    pub standby_acked_seq: Option<u64>,
}

/// A boxed future.
pub type BoxFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// What the LAN service needs from the cluster node.
pub trait ClusterView: Send + Sync + 'static {
    /// Current status.
    fn status(&self) -> ClusterStatus;
    /// Serves a replication session from the peer (this node is the follower).
    fn serve_follower(&self, peer: StationIdentity, socket: WebSocket) -> BoxFuture;
}

/// Everything the LAN service is built from.
pub struct LanContext {
    /// Provisioned stations.
    pub registry: StationRegistry,
    /// Accounts and audit.
    pub authority: Arc<dyn UserAuthority>,
    /// Alarm bus.
    pub alarms: Arc<dyn AlarmSource>,
    /// Cluster node.
    pub cluster: Arc<dyn ClusterView>,
    /// Session timing.
    pub policy: SessionPolicy,
}

struct AppState {
    authority: Arc<dyn UserAuthority>,
    alarms: Arc<dyn AlarmSource>,
    cluster: Arc<dyn ClusterView>,
    sessions: Mutex<LanSessions>,
    started: Instant,
}

impl AppState {
    fn now_ms(&self) -> u64 {
        crate::timing::duration_ms(self.started.elapsed())
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, LanSessions> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn resolve(&self, token: &str, station: &StationIdentity) -> Option<UserIdentity> {
        let user = self.sessions().resolve(token, station, self.now_ms())?;
        if self.authority.is_active(&user) {
            Some(user)
        } else {
            self.sessions().revoke_user(user.user_id);
            None
        }
    }

    fn is_primary(&self) -> bool {
        self.cluster.status().role == InstanceRole::Primary
    }
}

/// A running LAN server. Dropping it (or [`Self::shutdown`]) closes the listener and every
/// connection.
pub struct LanServer {
    local_addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl LanServer {
    /// The bound address.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Stops accepting and drops every connection.
    pub fn shutdown(&self) {
        self.task.abort();
    }
}

impl Drop for LanServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Serves the LAN API on `listener` with mutual TLS.
pub fn serve(
    listener: TcpListener,
    tls: Arc<rustls::ServerConfig>,
    ctx: LanContext,
) -> std::io::Result<LanServer> {
    let local_addr = listener.local_addr()?;
    let registry = Arc::new(ctx.registry);
    let state = Arc::new(AppState {
        authority: ctx.authority,
        alarms: ctx.alarms,
        cluster: ctx.cluster,
        sessions: Mutex::new(LanSessions::new(ctx.policy)),
        started: Instant::now(),
    });
    let app = router(state.clone());
    let acceptor = TlsAcceptor::from(tls);
    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                Some(_) = connections.join_next(), if !connections.is_empty() => continue,
            };
            let (tcp, _) = match accepted {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("lan accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            let acceptor = acceptor.clone();
            let registry = registry.clone();
            let app = app.clone();
            let state = state.clone();
            connections.spawn(async move {
                let Ok(Ok(tls)) =
                    tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await
                else {
                    return;
                };
                let Some(cert) = tls.get_ref().1.peer_certificates().and_then(|c| c.first()) else {
                    return;
                };
                let Some(station) = registry.identify(cert).cloned() else {
                    state.authority.audit(LanAuditEvent::UnknownStation {
                        fingerprint: hex(&fingerprint(cert)),
                    });
                    return;
                };
                let service = TowerToHyperService::new(app.layer(Extension(station)));
                let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
                if let Err(e) = builder
                    .serve_connection_with_upgrades(TokioIo::new(tls), service)
                    .await
                {
                    tracing::debug!("lan connection ended: {e}");
                }
            });
        }
    });
    Ok(LanServer { local_addr, task })
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/status", get(status))
        .route(
            "/v1/sessions",
            axum::routing::post(session_open).delete(session_close),
        )
        .route("/v1/alarms", get(alarm_list))
        .route("/v1/alarms/stream", get(alarm_stream))
        .route("/v1/emergency/stream", get(emergency_stream))
        .route("/v1/cluster/replicate", get(replicate))
        .with_state(state)
}

/// An error body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// `unauthenticated`, `station`, `role`, `not_primary`, `no_data`, `bad_request`.
    pub error: String,
    /// For `not_primary`: the primary this node follows, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<Fence>,
}

fn error(status: StatusCode, error: &str, primary: Option<Fence>) -> Response {
    (
        status,
        Json(ErrorBody {
            error: error.into(),
            primary,
        }),
    )
        .into_response()
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

struct Caller {
    user: Option<UserIdentity>,
}

/// The single check every endpoint runs first (CLAUDE.md rule 6).
fn guard(
    state: &AppState,
    endpoint: Endpoint,
    station: &StationIdentity,
    headers: &HeaderMap,
) -> Result<Caller, Box<Response>> {
    let user = match endpoint.access() {
        EndpointAccess::User { .. } => bearer(headers).and_then(|t| state.resolve(t, station)),
        EndpointAccess::Stations(_) => None,
    };
    if let Err(reason) = authorize(endpoint, station, user.as_ref()) {
        state.authority.audit(LanAuditEvent::Refused {
            station: station.station_id.clone(),
            endpoint: endpoint.route().1,
            reason,
        });
        return Err(Box::new(match reason {
            LanDenied::Unauthenticated => error(StatusCode::UNAUTHORIZED, "unauthenticated", None),
            LanDenied::Station => error(StatusCode::FORBIDDEN, "station", None),
            LanDenied::Role => error(StatusCode::FORBIDDEN, "role", None),
        }));
    }
    if endpoint.primary_only() {
        let status = state.cluster.status();
        if status.role != InstanceRole::Primary {
            return Err(Box::new(error(
                StatusCode::SERVICE_UNAVAILABLE,
                "not_primary",
                status.primary,
            )));
        }
    }
    Ok(Caller { user })
}

async fn status(
    State(state): State<Arc<AppState>>,
    Extension(station): Extension<StationIdentity>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = guard(&state, Endpoint::Status, &station, &headers) {
        return *r;
    }
    Json(state.cluster.status()).into_response()
}

/// Sign-in request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInRequest {
    /// User name.
    pub username: String,
    /// PIN or password.
    pub secret: String,
}

/// Sign-in response body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInResponse {
    /// Session token for `Authorization: Bearer`.
    pub token: String,
    /// The signed-in account.
    pub user: UserIdentity,
}

async fn session_open(
    State(state): State<Arc<AppState>>,
    Extension(station): Extension<StationIdentity>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(r) = guard(&state, Endpoint::SessionOpen, &station, &headers) {
        return *r;
    }
    let Ok(request) = serde_json::from_slice::<SignInRequest>(&body) else {
        return error(StatusCode::BAD_REQUEST, "bad_request", None);
    };
    let authority = state.authority.clone();
    let station_c = station.clone();
    let username = request.username.clone();
    let result = tokio::task::spawn_blocking(move || {
        authority.sign_in(&station_c, &request.username, request.secret.as_bytes())
    })
    .await;
    match result {
        Ok(Ok(user)) => {
            let issued = state
                .sessions()
                .issue(&station, user.clone(), state.now_ms());
            match issued {
                Ok(token) => {
                    state.authority.audit(LanAuditEvent::SignedIn {
                        station: station.station_id.clone(),
                        user: user.clone(),
                    });
                    Json(SignInResponse { token, user }).into_response()
                }
                Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "no_random", None),
            }
        }
        _ => {
            state.authority.audit(LanAuditEvent::SignInFailed {
                station: station.station_id.clone(),
                username,
            });
            error(StatusCode::UNAUTHORIZED, "unauthenticated", None)
        }
    }
}

async fn session_close(
    State(state): State<Arc<AppState>>,
    Extension(station): Extension<StationIdentity>,
    headers: HeaderMap,
) -> Response {
    let caller = match guard(&state, Endpoint::SessionClose, &station, &headers) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    if let Some(token) = bearer(&headers) {
        state.sessions().revoke(token);
    }
    if let Some(user) = caller.user {
        state.authority.audit(LanAuditEvent::SignedOut {
            station: station.station_id.clone(),
            user,
        });
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Alarm list response body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlarmListResponse {
    /// Alarms the caller may see, projected.
    pub alarms: Vec<AlarmView>,
}

fn views(alarms: &[LanAlarm], viewer: &Viewer) -> Vec<AlarmView> {
    alarms.iter().filter_map(|a| project(a, viewer)).collect()
}

async fn alarm_list(
    State(state): State<Arc<AppState>>,
    Extension(station): Extension<StationIdentity>,
    headers: HeaderMap,
) -> Response {
    let caller = match guard(&state, Endpoint::AlarmList, &station, &headers) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let Some(user) = caller.user else {
        return error(StatusCode::UNAUTHORIZED, "unauthenticated", None);
    };
    match state.alarms.snapshot() {
        Ok(alarms) => Json(AlarmListResponse {
            alarms: views(&alarms, &Viewer::User(user)),
        })
        .into_response(),
        Err(SourceUnavailable) => error(StatusCode::SERVICE_UNAVAILABLE, "no_data", None),
    }
}

/// A message on an alarm stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamMessage {
    /// Every alarm the viewer may see (on open, and again after the stream fell behind).
    Snapshot {
        /// Alarms.
        alarms: Vec<AlarmView>,
    },
    /// One alarm changed.
    Update {
        /// The alarm.
        alarm: AlarmView,
    },
    /// The stream ends: `session_ended`, `not_primary` or `no_data`.
    Closed {
        /// Why.
        reason: String,
    },
}

async fn alarm_stream(
    State(state): State<Arc<AppState>>,
    Extension(station): Extension<StationIdentity>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let caller = match guard(&state, Endpoint::AlarmStream, &station, &headers) {
        Ok(c) => c,
        Err(r) => return *r,
    };
    let (Some(user), Some(token)) = (caller.user, bearer(&headers).map(str::to_owned)) else {
        return error(StatusCode::UNAUTHORIZED, "unauthenticated", None);
    };
    let Ok(ws) = ws else {
        return error(StatusCode::BAD_REQUEST, "bad_request", None);
    };
    ws.on_upgrade(move |socket| {
        run_stream(state, socket, Viewer::User(user), Some((token, station)))
    })
}

async fn emergency_stream(
    State(state): State<Arc<AppState>>,
    Extension(station): Extension<StationIdentity>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if let Err(r) = guard(&state, Endpoint::EmergencyStream, &station, &headers) {
        return *r;
    }
    let Ok(ws) = ws else {
        return error(StatusCode::BAD_REQUEST, "bad_request", None);
    };
    state.authority.audit(LanAuditEvent::EmergencyViewOpened {
        station: station.station_id.clone(),
    });
    ws.on_upgrade(move |socket| run_stream(state, socket, Viewer::Emergency, None))
}

async fn send(socket: &mut WebSocket, message: &StreamMessage) -> bool {
    match serde_json::to_string(message) {
        Ok(text) => socket.send(Message::Text(text.into())).await.is_ok(),
        Err(_) => false,
    }
}

async fn close(mut socket: WebSocket, reason: &str) {
    let _ = send(
        &mut socket,
        &StreamMessage::Closed {
            reason: reason.into(),
        },
    )
    .await;
    let _ = socket.send(Message::Close(None)).await;
}

/// Streams the alarm bus to one viewer. Subscribes before taking the snapshot so no change is
/// missed; after falling behind, sends a fresh snapshot instead of dropping updates silently.
async fn run_stream(
    state: Arc<AppState>,
    mut socket: WebSocket,
    viewer: Viewer,
    session: Option<(String, StationIdentity)>,
) {
    let mut rx = state.alarms.subscribe();
    let Ok(alarms) = state.alarms.snapshot() else {
        return close(socket, "no_data").await;
    };
    if !send(
        &mut socket,
        &StreamMessage::Snapshot {
            alarms: views(&alarms, &viewer),
        },
    )
    .await
    {
        return;
    }
    let mut recheck = tokio::time::interval(STREAM_RECHECK);
    loop {
        tokio::select! {
            event = rx.recv() => {
                let message = match event {
                    Ok(alarm) => match project(&alarm, &viewer) {
                        Some(view) => StreamMessage::Update { alarm: view },
                        None => continue,
                    },
                    Err(RecvError::Lagged(_)) => match state.alarms.snapshot() {
                        Ok(alarms) => StreamMessage::Snapshot { alarms: views(&alarms, &viewer) },
                        Err(SourceUnavailable) => return close(socket, "no_data").await,
                    },
                    Err(RecvError::Closed) => return close(socket, "no_data").await,
                };
                if !send(&mut socket, &message).await {
                    return;
                }
            }
            _ = recheck.tick() => {
                if !state.is_primary() {
                    return close(socket, "not_primary").await;
                }
                if let Some((token, station)) = &session {
                    if state.resolve(token, station).is_none() {
                        return close(socket, "session_ended").await;
                    }
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    Some(Ok(_)) => {}
                }
            }
        }
    }
}

async fn replicate(
    State(state): State<Arc<AppState>>,
    Extension(station): Extension<StationIdentity>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if let Err(r) = guard(&state, Endpoint::Replicate, &station, &headers) {
        return *r;
    }
    let cluster = state.cluster.clone();
    let Ok(ws) = ws else {
        return error(StatusCode::BAD_REQUEST, "bad_request", None);
    };
    ws.max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |socket| cluster.serve_follower(station, socket))
}
