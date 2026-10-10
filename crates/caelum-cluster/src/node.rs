//! The async driver of one CAELUM instance (F-SYS-04, F-SYS-05, R-OPS-02).
//!
//! Runs the fail-over state machine on a tick, appends station events to the signed log while
//! primary, pushes the log to the peer (the primary connects to the standby's replication
//! endpoint and is the leader), follows a primary's session when standby, and serves the crew
//! LAN API. All state lives behind one mutex that is never held across an `await`.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::extract::ws::{Message as AxMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message as TsMessage;

use crate::failover::{Action, Failover};
use crate::fence::{Fence, NodeId};
use crate::lan::client::{connect, websocket, LanStream};
use crate::lan::server::{
    serve, AlarmSource, BoxFuture, ClusterStatus, ClusterView, LanContext, LanServer, UserAuthority,
};
use crate::lan::sessions::SessionPolicy;
use crate::lan::tls::{TlsError, TlsMaterial};
use crate::lan::{Fingerprint, StationIdentity, StationRegistry};
use crate::log::{ChainHead, EntryBody, EntrySigner, LogEntry, LogError, TrustedSigners};
use crate::replication::{
    checkpoints, checkpoints_between, Checkpoint, FenceStore, FollowerSession, LeaderSession,
    LogStore, ReplicationError, SyncStep, DEFAULT_WINDOW,
};
use crate::timing::{duration_ms, Timing, TimingError};
use crate::wire::{PeerMessage, WireCheckpoint, WireEntry, BATCH};
use crate::StoreFailure;

/// The other node of the fail-over pair.
#[derive(Debug, Clone)]
pub struct PeerConfig {
    /// Its node ID.
    pub node: NodeId,
    /// Its LAN address.
    pub addr: SocketAddr,
    /// The name its certificate is issued for (DNS name or IP address).
    pub server_name: String,
    /// Its certificate fingerprint (pinned).
    pub fingerprint: Fingerprint,
}

/// Node configuration.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// This node.
    pub node: NodeId,
    /// Address to serve the LAN API on.
    pub listen: SocketAddr,
    /// Fail-over timing (validated against R-OPS-02).
    pub timing: Timing,
    /// Unacknowledged entries in flight to the standby.
    pub window: u64,
    /// The peer; `None` for a single-instance deployment.
    pub peer: Option<PeerConfig>,
    /// LAN session timing.
    pub sessions: SessionPolicy,
}

impl NodeConfig {
    /// Defaults for `node` listening on `listen`.
    pub fn new(node: NodeId, listen: SocketAddr) -> Self {
        Self {
            node,
            listen,
            timing: Timing::default(),
            window: DEFAULT_WINDOW,
            peer: None,
            sessions: SessionPolicy::default(),
        }
    }
}

/// What the core supplies.
pub struct NodeParts {
    /// The event log.
    pub log: Box<dyn LogStore>,
    /// The highest fence seen.
    pub fences: Box<dyn FenceStore>,
    /// This node's signing key.
    pub signer: Box<dyn EntrySigner>,
    /// Public keys of every cluster node (this one included).
    pub trust: TrustedSigners,
    /// This node's TLS certificate and the crew-LAN CA.
    pub tls: TlsMaterial,
    /// Provisioned stations (the peer included).
    pub registry: StationRegistry,
    /// Accounts and audit.
    pub authority: Arc<dyn UserAuthority>,
    /// Alarm bus.
    pub alarms: Arc<dyn AlarmSource>,
}

/// Something the core must react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeEvent {
    /// This node is now primary: restore escalation from the replicated record and act.
    BecamePrimary {
        /// The fence it acts under.
        fence: Fence,
        /// From the last message of the previous primary (or from start) to promotion.
        silence: Duration,
    },
    /// Hand-off: connect the devices of the replicated registry by `deadline`.
    ReconnectDevices {
        /// The fence the node acts under.
        fence: Fence,
        /// Device reconnection deadline.
        deadline: Instant,
    },
    /// A newer primary exists: stop acting at once and release the devices.
    Demoted {
        /// The fence this node acted under.
        from: Fence,
        /// The newer fence.
        to: Fence,
    },
    /// Entries this node wrote as a fenced primary were never acknowledged and were moved to
    /// quarantine (`LogStore::divert_after`), for review; never silently dropped.
    Diverted {
        /// The entries.
        entries: Vec<LogEntry>,
    },
    /// The peer sent something refused (tampered entry, protocol violation): technical alarm.
    ReplicationRefused {
        /// Why.
        reason: String,
    },
    /// A store failed: technical alarm.
    StoreFailed {
        /// Why.
        reason: String,
    },
}

/// Why an append was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppendError {
    /// This node is not the primary (or has been fenced).
    #[error("this instance is not the primary")]
    NotPrimary,
    /// The entry is malformed.
    #[error(transparent)]
    Log(#[from] LogError),
    /// Persistence failed.
    #[error(transparent)]
    Store(#[from] StoreFailure),
}

/// Why a node could not start.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// Timing breaks R-OPS-02.
    #[error(transparent)]
    Timing(#[from] TimingError),
    /// TLS configuration.
    #[error(transparent)]
    Tls(#[from] TlsError),
    /// A store failed.
    #[error(transparent)]
    Store(#[from] StoreFailure),
    /// The listener could not be bound.
    #[error("listen: {0}")]
    Io(#[from] std::io::Error),
}

struct Inner {
    failover: Failover,
    log: Box<dyn LogStore>,
    fences: Box<dyn FenceStore>,
    head: ChainHead,
    standby_acked: Option<u64>,
}

struct Shared {
    me: NodeId,
    window: u64,
    timing: Timing,
    started: Instant,
    signer: Box<dyn EntrySigner>,
    trust: TrustedSigners,
    inner: Mutex<Inner>,
    head_tx: watch::Sender<u64>,
    role_tx: watch::Sender<Option<Fence>>,
    heartbeat_tx: watch::Sender<u64>,
    acked_tx: watch::Sender<Option<u64>>,
    events: broadcast::Sender<NodeEvent>,
}

impl Shared {
    fn now_ms(&self) -> u64 {
        duration_ms(self.started.elapsed())
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn emit(&self, event: NodeEvent) {
        let _ = self.events.send(event);
    }

    /// Executes state-machine actions in order (under the lock).
    fn apply(&self, inner: &mut Inner, actions: Vec<Action>, now_ms: u64) {
        for action in actions {
            match action {
                Action::PersistFence(fence) => match inner.fences.save(&fence) {
                    Ok(()) => {
                        let more = inner.failover.fence_persisted(&fence, now_ms);
                        self.apply(inner, more, now_ms);
                    }
                    Err(e) => self.emit(NodeEvent::StoreFailed { reason: e.0 }),
                },
                Action::BecomePrimary {
                    fence,
                    silent_since_ms,
                    promoted_at_ms,
                } => {
                    inner.standby_acked = None;
                    tracing::warn!(%fence, "instance promoted to primary");
                    self.emit(NodeEvent::BecamePrimary {
                        fence,
                        silence: Duration::from_millis(
                            promoted_at_ms.saturating_sub(silent_since_ms),
                        ),
                    });
                }
                Action::ReconnectDevices { fence, deadline_ms } => {
                    self.emit(NodeEvent::ReconnectDevices {
                        fence,
                        deadline: self.started + Duration::from_millis(deadline_ms),
                    });
                }
                Action::Demote { from, to } => {
                    inner.standby_acked = None;
                    tracing::warn!(%from, %to, "instance fenced: demoted to standby");
                    self.emit(NodeEvent::Demoted { from, to });
                }
                Action::SendHeartbeat(_) => {
                    self.heartbeat_tx.send_modify(|n| *n = n.wrapping_add(1));
                }
            }
        }
        self.role_tx
            .send_if_modified(|f| replace_if_changed(f, inner.failover.acting_fence().cloned()));
        self.acked_tx
            .send_if_modified(|a| replace_if_changed(a, inner.standby_acked));
    }

    fn learn_fence(&self, fence: &Fence) {
        let now = self.now_ms();
        let mut inner = self.lock();
        if let Ok(actions) = inner.failover.on_peer_fence(fence, now) {
            self.apply(&mut inner, actions, now);
        }
    }

    fn set_acked(&self, acked: Option<u64>) {
        let mut inner = self.lock();
        inner.standby_acked = acked;
        self.acked_tx
            .send_if_modified(|a| replace_if_changed(a, acked));
    }

    fn acting_fence(&self) -> Option<Fence> {
        self.lock().failover.acting_fence().cloned()
    }

    fn status(&self) -> ClusterStatus {
        let inner = self.lock();
        ClusterStatus {
            node: self.me.clone(),
            role: inner.failover.role(),
            primary: inner.failover.current_primary().cloned(),
            highest_seen: inner.failover.highest_seen().cloned(),
            head_seq: inner.head.seq,
            standby_acked_seq: if inner.failover.may_act() {
                inner.standby_acked
            } else {
                None
            },
        }
    }
}

fn replace_if_changed<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        false
    } else {
        *slot = value;
        true
    }
}

struct View(Arc<Shared>);

impl ClusterView for View {
    fn status(&self) -> ClusterStatus {
        self.0.status()
    }

    fn serve_follower(&self, peer: StationIdentity, socket: WebSocket) -> BoxFuture {
        Box::pin(follow(self.0.clone(), peer, socket))
    }
}

/// A running instance. Dropping it stops every task and closes every connection, as a crash
/// would.
pub struct Node {
    shared: Arc<Shared>,
    server: LanServer,
    tasks: Vec<JoinHandle<()>>,
}

impl Node {
    /// Starts the instance as a standby; it promotes itself if no primary is heard within the
    /// detection window.
    pub async fn start(config: NodeConfig, parts: NodeParts) -> Result<Node, NodeError> {
        let listener = TcpListener::bind(config.listen).await?;
        Self::start_on(listener, config, parts)
    }

    /// As [`Self::start`], on an already bound listener (`config.listen` is ignored).
    pub fn start_on(
        listener: TcpListener,
        config: NodeConfig,
        parts: NodeParts,
    ) -> Result<Node, NodeError> {
        config.timing.validate()?;
        let head = parts.log.head()?;
        let persisted = std::cmp::max(parts.fences.load()?, head.fence.clone());
        let started = Instant::now();
        let failover = Failover::new(config.node.clone(), config.timing, persisted, 0);
        let (events, _) = broadcast::channel(256);
        let shared = Arc::new(Shared {
            me: config.node.clone(),
            window: config.window,
            timing: config.timing,
            started,
            signer: parts.signer,
            trust: parts.trust,
            head_tx: watch::Sender::new(head.seq),
            inner: Mutex::new(Inner {
                failover,
                log: parts.log,
                fences: parts.fences,
                head,
                standby_acked: None,
            }),
            role_tx: watch::Sender::new(None),
            heartbeat_tx: watch::Sender::new(0),
            acked_tx: watch::Sender::new(None),
            events,
        });
        let server = serve(
            listener,
            parts.tls.server_config()?,
            LanContext {
                registry: parts.registry,
                authority: parts.authority,
                alarms: parts.alarms,
                cluster: Arc::new(View(shared.clone())),
                policy: config.sessions,
            },
        )?;
        let mut tasks = vec![tokio::spawn(ticker(shared.clone()))];
        if let Some(peer) = config.peer {
            let client = parts.tls.client_config()?;
            tasks.push(tokio::spawn(push_loop(shared.clone(), peer, client)));
        }
        Ok(Node {
            shared,
            server,
            tasks,
        })
    }

    /// The LAN address.
    pub fn local_addr(&self) -> SocketAddr {
        self.server.local_addr()
    }

    /// Current status (for the status footer).
    pub fn status(&self) -> ClusterStatus {
        self.shared.status()
    }

    /// Records a station event. Only the primary may append; a fenced node gets
    /// [`AppendError::NotPrimary`]. Returns the sequence number.
    pub fn append(&self, body: EntryBody) -> Result<u64, AppendError> {
        let mut inner = self.shared.lock();
        let fence = inner
            .failover
            .acting_fence()
            .cloned()
            .ok_or(AppendError::NotPrimary)?;
        let entry = LogEntry::create(&inner.head, fence, body, self.shared.signer.as_ref())?;
        inner.log.append(&entry)?;
        inner.head = ChainHead::of(&entry);
        drop(inner);
        self.shared.head_tx.send_replace(entry.seq);
        Ok(entry.seq)
    }

    /// Highest sequence number the standby has persisted, if one is connected.
    pub fn standby_acked(&self) -> Option<u64> {
        *self.shared.acked_tx.borrow()
    }

    /// Waits until the standby has persisted `seq` (the entry is then acknowledged and survives
    /// a takeover). `false` on timeout.
    pub async fn wait_acknowledged(&self, seq: u64, timeout: Duration) -> bool {
        let mut rx = self.shared.acked_tx.subscribe();
        tokio::time::timeout(timeout, rx.wait_for(|a| a.is_some_and(|a| a >= seq)))
            .await
            .is_ok_and(|r| r.is_ok())
    }

    /// Events for the core.
    pub fn events(&self) -> broadcast::Receiver<NodeEvent> {
        self.shared.events.subscribe()
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        self.server.shutdown();
    }
}

async fn ticker(shared: Arc<Shared>) {
    let mut interval = tokio::time::interval(shared.timing.tick_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let now = shared.now_ms();
        let mut inner = shared.lock();
        let actions = inner.failover.tick(now);
        shared.apply(&mut inner, actions, now);
    }
}

/// Why a session ended.
#[derive(Debug)]
enum SessionEnd {
    Closed,
    Fenced,
    Refused(String),
}

impl From<ReplicationError> for SessionEnd {
    fn from(e: ReplicationError) -> Self {
        SessionEnd::Refused(e.to_string())
    }
}

impl From<crate::wire::WireError> for SessionEnd {
    fn from(e: crate::wire::WireError) -> Self {
        SessionEnd::Refused(e.to_string())
    }
}

impl From<StoreFailure> for SessionEnd {
    fn from(e: StoreFailure) -> Self {
        SessionEnd::Refused(e.to_string())
    }
}

/// The primary's side: keeps a leader session open to the peer while primary.
async fn push_loop(shared: Arc<Shared>, peer: PeerConfig, client: Arc<rustls::ClientConfig>) {
    let mut role_rx = shared.role_tx.subscribe();
    loop {
        let fence = loop {
            if let Some(fence) = shared.acting_fence() {
                break fence;
            }
            if role_rx.changed().await.is_err() {
                return;
            }
        };
        let end = match connect(
            client.clone(),
            peer.addr,
            &peer.server_name,
            Some(&peer.fingerprint),
        )
        .await
        {
            Ok(stream) => lead(&shared, stream, fence).await,
            Err(e) => Err(SessionEnd::Refused(e.to_string())),
        };
        shared.set_acked(None);
        if let Err(SessionEnd::Refused(reason)) = end {
            tracing::debug!("replication to {} ended: {reason}", peer.node);
        }
        tokio::time::sleep(shared.timing.connect_retry).await;
    }
}

type ClientSocket = tokio_tungstenite::WebSocketStream<LanStream>;

async fn send_ts(ws: &mut ClientSocket, message: &PeerMessage) -> Result<(), SessionEnd> {
    let text = message.encode()?;
    ws.send(TsMessage::Text(text.into()))
        .await
        .map_err(|_| SessionEnd::Closed)
}

async fn recv_ts(ws: &mut ClientSocket) -> Result<PeerMessage, SessionEnd> {
    loop {
        match ws.next().await {
            Some(Ok(TsMessage::Text(text))) => return Ok(PeerMessage::decode(text.as_str())?),
            Some(Ok(TsMessage::Ping(_) | TsMessage::Pong(_))) => {}
            _ => return Err(SessionEnd::Closed),
        }
    }
}

async fn lead(shared: &Arc<Shared>, stream: LanStream, fence: Fence) -> Result<(), SessionEnd> {
    let mut ws = websocket(stream, "/v1/cluster/replicate", None)
        .await
        .map_err(|e| SessionEnd::Refused(e.to_string()))?;
    let head_seq = shared.lock().head.seq;
    send_ts(
        &mut ws,
        &PeerMessage::Hello {
            fence: fence.clone(),
            head_seq,
        },
    )
    .await?;
    let mut session = LeaderSession::new(fence.clone(), shared.window);
    loop {
        let reply = tokio::time::timeout(shared.timing.detection_window(), recv_ts(&mut ws))
            .await
            .map_err(|_| SessionEnd::Closed)??;
        match reply {
            PeerMessage::Fenced { fence: newer } => {
                shared.learn_fence(&newer);
                return Err(SessionEnd::Fenced);
            }
            PeerMessage::Checkpoints { points } => {
                let points = points
                    .iter()
                    .map(Checkpoint::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                let next = {
                    let inner = shared.lock();
                    session.on_checkpoints(inner.log.as_ref(), &points)?
                };
                match next {
                    SyncStep::Probe { after, before } => {
                        send_ts(&mut ws, &PeerMessage::Probe { after, before }).await?;
                    }
                    SyncStep::Resume(after) => {
                        send_ts(&mut ws, &PeerMessage::Resume { after }).await?;
                        shared.set_acked(Some(after));
                        break;
                    }
                }
            }
            _ => return Err(SessionEnd::Refused("unexpected reply to hello".into())),
        }
    }
    let mut head_rx = shared.head_tx.subscribe();
    let mut heartbeat_rx = shared.heartbeat_tx.subscribe();
    let mut role_rx = shared.role_tx.subscribe();
    loop {
        loop {
            let batch = {
                let inner = shared.lock();
                if inner.failover.acting_fence() != Some(&fence) {
                    return Err(SessionEnd::Fenced);
                }
                session.next_batch(inner.log.as_ref(), BATCH)?
            };
            if batch.is_empty() {
                break;
            }
            let entries = batch.iter().map(WireEntry::from).collect();
            send_ts(&mut ws, &PeerMessage::Entries { entries }).await?;
        }
        tokio::select! {
            changed = head_rx.changed() => {
                if changed.is_err() {
                    return Err(SessionEnd::Closed);
                }
            }
            changed = heartbeat_rx.changed() => {
                if changed.is_err() {
                    return Err(SessionEnd::Closed);
                }
                let head_seq = shared.lock().head.seq;
                send_ts(&mut ws, &PeerMessage::Heartbeat { fence: fence.clone(), head_seq }).await?;
            }
            changed = role_rx.changed() => {
                if changed.is_err() || role_rx.borrow_and_update().as_ref() != Some(&fence) {
                    return Err(SessionEnd::Fenced);
                }
            }
            message = recv_ts(&mut ws) => match message? {
                PeerMessage::Ack { fence: f, seq } => {
                    let acked = session.on_ack(&f, seq)?;
                    shared.set_acked(Some(acked));
                }
                PeerMessage::Fenced { fence: newer } => {
                    shared.learn_fence(&newer);
                    return Err(SessionEnd::Fenced);
                }
                _ => return Err(SessionEnd::Refused("unexpected message from follower".into())),
            },
        }
    }
}

async fn send_ax(socket: &mut WebSocket, message: &PeerMessage) -> Result<(), SessionEnd> {
    let text = message.encode()?;
    socket
        .send(AxMessage::Text(text.into()))
        .await
        .map_err(|_| SessionEnd::Closed)
}

async fn recv_ax(socket: &mut WebSocket) -> Result<PeerMessage, SessionEnd> {
    loop {
        match socket.recv().await {
            Some(Ok(AxMessage::Text(text))) => return Ok(PeerMessage::decode(text.as_str())?),
            Some(Ok(AxMessage::Ping(_) | AxMessage::Pong(_))) => {}
            _ => return Err(SessionEnd::Closed),
        }
    }
}

/// The standby's side of a session opened by the peer.
async fn follow(shared: Arc<Shared>, peer: StationIdentity, mut socket: WebSocket) {
    match follow_session(&shared, &peer, &mut socket).await {
        Ok(()) | Err(SessionEnd::Closed) => {}
        Err(SessionEnd::Fenced) => {}
        Err(SessionEnd::Refused(reason)) => {
            tracing::warn!("replication from {} refused: {reason}", peer.station_id);
            shared.emit(NodeEvent::ReplicationRefused { reason });
        }
    }
    let _ = socket.send(AxMessage::Close(None)).await;
}

/// Checks a leader's fence; answers a stale leader with the current fence.
async fn admit(
    shared: &Arc<Shared>,
    socket: &mut WebSocket,
    fence: &Fence,
) -> Result<(), SessionEnd> {
    let stale = {
        let now = shared.now_ms();
        let mut inner = shared.lock();
        match inner.failover.on_heartbeat(fence, now) {
            Ok(actions) => {
                shared.apply(&mut inner, actions, now);
                if inner.failover.current_primary() == Some(fence) {
                    None
                } else {
                    // Promoting under a newer fence, or the leader is this node's own old fence.
                    inner.failover.highest_seen().cloned()
                }
            }
            Err(stale) => Some(stale.current),
        }
    };
    match stale {
        None => Ok(()),
        Some(current) => {
            send_ax(socket, &PeerMessage::Fenced { fence: current }).await?;
            Err(SessionEnd::Fenced)
        }
    }
}

async fn follow_session(
    shared: &Arc<Shared>,
    peer: &StationIdentity,
    socket: &mut WebSocket,
) -> Result<(), SessionEnd> {
    let hello = tokio::time::timeout(shared.timing.detection_window(), recv_ax(socket))
        .await
        .map_err(|_| SessionEnd::Closed)??;
    let PeerMessage::Hello { fence, .. } = hello else {
        return Err(SessionEnd::Refused("expected hello".into()));
    };
    if fence.node != peer.station_id {
        return Err(SessionEnd::Refused(
            "hello fence names a node other than the connected peer".into(),
        ));
    }
    admit(shared, socket, &fence).await?;
    let (mut session, points) = {
        let inner = shared.lock();
        let points = checkpoints(inner.log.as_ref())?;
        (
            FollowerSession::new(fence.clone(), inner.head.clone()),
            points,
        )
    };
    let points = points.iter().map(WireCheckpoint::from).collect();
    send_ax(socket, &PeerMessage::Checkpoints { points }).await?;
    loop {
        match recv_ax(socket).await? {
            PeerMessage::Probe { after, before } => {
                let points = {
                    let inner = shared.lock();
                    checkpoints_between(inner.log.as_ref(), after, before)?
                };
                let points = points.iter().map(WireCheckpoint::from).collect();
                send_ax(socket, &PeerMessage::Checkpoints { points }).await?;
            }
            PeerMessage::Resume { after } => {
                let diverted = {
                    let mut inner = shared.lock();
                    let diverted = session.on_resume(inner.log.as_mut(), after)?;
                    inner.head = session.head().clone();
                    diverted
                };
                shared.head_tx.send_replace(session.head().seq);
                if !diverted.is_empty() {
                    tracing::warn!(count = diverted.len(), "unacknowledged entries quarantined");
                    shared.emit(NodeEvent::Diverted { entries: diverted });
                }
            }
            PeerMessage::Entries { entries } => {
                admit(shared, socket, &fence).await?;
                let (ack, refused) = {
                    let mut inner = shared.lock();
                    if &inner.head != session.head() {
                        return Err(SessionEnd::Refused(
                            "log changed outside this session".into(),
                        ));
                    }
                    let mut refused = None;
                    for wire in &entries {
                        let accepted =
                            LogEntry::try_from(wire)
                                .map_err(SessionEnd::from)
                                .and_then(|entry| {
                                    session
                                        .accept(inner.log.as_mut(), &shared.trust, &entry)
                                        .map_err(SessionEnd::from)
                                });
                        inner.head = session.head().clone();
                        if let Err(e) = accepted {
                            refused = Some(e);
                            break;
                        }
                    }
                    (session.ack_seq(), refused)
                };
                shared.head_tx.send_replace(ack);
                // Entries persisted before a refusal stay persisted and are acknowledged.
                send_ax(
                    socket,
                    &PeerMessage::Ack {
                        fence: fence.clone(),
                        seq: ack,
                    },
                )
                .await?;
                if let Some(refused) = refused {
                    return Err(refused);
                }
            }
            PeerMessage::Heartbeat { fence: f, .. } => {
                if f != fence {
                    return Err(SessionEnd::Refused("heartbeat under another fence".into()));
                }
                admit(shared, socket, &fence).await?;
            }
            _ => return Err(SessionEnd::Refused("unexpected message from leader".into())),
        }
    }
}
