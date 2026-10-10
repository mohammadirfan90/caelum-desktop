//! The core side of the fail-over pair and the crew-LAN service (F-SYS-04, F-SYS-05, R-OPS-02;
//! CR-017, docs/integration/M11.md): the signed `cluster` configuration, the event log and fence
//! persisted in the encrypted database, crew-LAN sign-in on the accounts of this station, and the
//! alarm source the LAN streams to secondary stations and kiosks.
//!
//! Everything here runs on its own database connections, outside the station lock: a slow LAN
//! client or an argon2 verification never delays alarm handling.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use caelum_alarms::signing::{built_in_keys, verify_config, TrustPolicy};
use caelum_cluster::fence::{Fence, NodeId};
use caelum_cluster::lan::access::{AlarmPriority, LanAlarm, OpsFields};
use caelum_cluster::lan::server::{
    AlarmSource, LanAuditEvent, SignInRefused, SourceUnavailable, UserAuthority,
};
use caelum_cluster::lan::{Fingerprint, StationIdentity, StationKind, StationRegistry};
use caelum_cluster::log::{ChainHead, EventKind, LogEntry, TrustedSigners};
use caelum_cluster::node::PeerConfig;
use caelum_cluster::replication::{FenceStore, LogStore};
use caelum_cluster::StoreFailure;
use caelum_domain::ids::{AlarmId, CrewMemberId, UserId};
use caelum_security::audit::{Actor, AuditAction, AuditLog};
use caelum_security::clock::Clock;
use caelum_security::credential::{verify_against_dummy, verify_secret};
use caelum_security::keys::InstanceKey;
use caelum_security::role::{Role, UserIdentity};
use caelum_store::{AlarmView, ClusterLogRecord, Database, UserRecord};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::broadcast;

/// Configuration kind of the fail-over pair and crew LAN (CR-017 §4).
pub const CLUSTER_KIND: &str = "cluster";
/// State key: the highest fence seen (JSON).
pub const FENCE_KEY: &str = "fence";
/// State key: this station's crew-LAN certificate chain (PEM).
pub const IDENTITY_CHAIN: &str = "identity.chain_pem";
/// State key: this station's crew-LAN private key (PEM; encrypted database, never logged).
pub const IDENTITY_KEY: &str = "identity.key_pem";

/// Sign-in failures from one name before the crew LAN refuses it for a while.
const MAX_FAILURES: u32 = 5;
/// First refusal period after [`MAX_FAILURES`]; doubles with each further failure.
const THROTTLE_BASE: Duration = Duration::from_secs(30);
/// Longest refusal period.
const THROTTLE_MAX: Duration = Duration::from_secs(15 * 60);
/// Ended alarms stay in the snapshot this long (as `alarm_list`).
const ENDED_WINDOW_NS: i64 = 24 * 3600 * 1_000_000_000;

fn failure(e: impl std::fmt::Display) -> StoreFailure {
    StoreFailure(e.to_string())
}

fn hex32(text: &str, what: &str) -> Result<[u8; 32], String> {
    let t = text.trim();
    if t.len() != 64 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{what} must be 64 hexadecimal digits"));
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&t[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// The other node of the pair, as configured.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerEntry {
    /// Its node ID.
    pub node_id: String,
    /// Its LAN address (`ip:port`).
    pub address: String,
    /// The name its certificate is issued for.
    pub server_name: String,
    /// SHA-256 of its certificate (hex).
    pub fingerprint: String,
    /// Its instance public key (Ed25519, hex), which signs its log entries.
    pub public_key: String,
}

/// A provisioned crew station.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StationEntry {
    /// Station ID.
    pub station_id: String,
    /// `secondary` or `kiosk`.
    pub kind: String,
    /// SHA-256 of its certificate (hex).
    pub fingerprint: String,
}

/// The signed `cluster` bundle (CR-017 §4). Every value is required: no defaults.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterConfig {
    /// Bundle version.
    pub version: String,
    /// This node's ID.
    pub node_id: String,
    /// Address to serve the crew LAN on (`ip:port`).
    pub listen: String,
    /// Crew-LAN CA certificate (PEM).
    pub ca_pem: String,
    /// The other node of the fail-over pair; absent for a single instance with crew stations.
    pub peer: Option<PeerEntry>,
    /// Secondary stations and kiosks.
    pub stations: Vec<StationEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClusterFile {
    cluster: ClusterConfig,
}

/// A checked configuration, ready to start a node from.
#[derive(Debug, Clone)]
pub struct ClusterPlan {
    /// The configuration.
    pub config: ClusterConfig,
    /// This node.
    pub node: NodeId,
    /// LAN address.
    pub listen: SocketAddr,
    /// The peer.
    pub peer: Option<(PeerConfig, [u8; 32])>,
    /// Provisioned stations (the peer included).
    pub registry: StationRegistry,
}

impl ClusterConfig {
    /// Parses and checks the bundle content.
    pub fn parse(text: &str) -> Result<ClusterPlan, String> {
        let f: ClusterFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let c = f.cluster;
        let node = NodeId::new(&c.node_id).map_err(|_| "node_id is not a valid node ID")?;
        let listen: SocketAddr = c
            .listen
            .parse()
            .map_err(|_| "listen must be ip:port".to_owned())?;
        if !c.ca_pem.contains("BEGIN CERTIFICATE") {
            return Err("ca_pem must be a PEM certificate".into());
        }
        let mut registry = StationRegistry::new();
        let mut seen: Vec<Fingerprint> = Vec::new();
        let mut add = |fp: Fingerprint, id: StationIdentity| -> Result<(), String> {
            if seen.contains(&fp) {
                return Err("a certificate fingerprint is listed twice".into());
            }
            seen.push(fp);
            registry.insert(fp, id);
            Ok(())
        };
        let peer = match &c.peer {
            None => None,
            Some(p) => {
                let pnode =
                    NodeId::new(&p.node_id).map_err(|_| "peer node_id is not a valid node ID")?;
                if pnode == node {
                    return Err("the peer cannot be this node".into());
                }
                let addr: SocketAddr = p
                    .address
                    .parse()
                    .map_err(|_| "peer address must be ip:port".to_owned())?;
                if p.server_name.trim().is_empty() {
                    return Err("peer server_name is required".into());
                }
                let fingerprint = hex32(&p.fingerprint, "peer fingerprint")?;
                let public_key = hex32(&p.public_key, "peer public_key")?;
                add(
                    fingerprint,
                    StationIdentity {
                        station_id: pnode.clone(),
                        kind: StationKind::Peer,
                    },
                )?;
                Some((
                    PeerConfig {
                        node: pnode,
                        addr,
                        server_name: p.server_name.clone(),
                        fingerprint,
                    },
                    public_key,
                ))
            }
        };
        for s in &c.stations {
            let kind = match s.kind.as_str() {
                "secondary" => StationKind::Secondary,
                "kiosk" => StationKind::Kiosk,
                _ => {
                    return Err(format!(
                        "station {}: kind is secondary or kiosk",
                        s.station_id
                    ))
                }
            };
            let id = NodeId::new(&s.station_id)
                .map_err(|_| format!("station {}: not a valid station ID", s.station_id))?;
            add(
                hex32(&s.fingerprint, "station fingerprint")?,
                StationIdentity {
                    station_id: id,
                    kind,
                },
            )?;
        }
        Ok(ClusterPlan {
            config: c,
            node,
            listen,
            peer,
            registry,
        })
    }
}

/// The configuration in force (re-verified from its stored bundle, SR-009), if any.
pub fn config_in_force(db: &Database) -> Result<Option<ClusterPlan>, String> {
    let Some((version, signature, content)) = db
        .latest_config_bundle(CLUSTER_KIND)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    verify_config(
        CLUSTER_KIND,
        content.as_bytes(),
        &signature,
        &built_in_keys(),
        TrustPolicy::for_this_build(),
    )
    .map_err(|e| format!("the stored cluster configuration no longer verifies: {e}"))?;
    let plan = ClusterConfig::parse(&content)?;
    Ok((plan.config.version == version).then_some(plan))
}

/// This station's crew-LAN identity (chain PEM, key PEM), if an administrator set one.
pub fn identity(db: &Database) -> Result<Option<(String, zeroize::Zeroizing<String>)>, String> {
    let chain = db
        .cluster_state(IDENTITY_CHAIN)
        .map_err(|e| e.to_string())?;
    let key = db.cluster_state(IDENTITY_KEY).map_err(|e| e.to_string())?;
    Ok(chain.zip(key).map(|(c, k)| (c, zeroize::Zeroizing::new(k))))
}

/// The trusted log signers: this node and the peer.
pub fn trusted_signers(plan: &ClusterPlan, own: &InstanceKey) -> Result<TrustedSigners, String> {
    let mut t = TrustedSigners::new();
    t.insert(plan.node.clone(), &own.verifying_key().to_bytes())
        .map_err(|e| e.to_string())?;
    if let Some((peer, key)) = &plan.peer {
        t.insert(peer.node.clone(), key)
            .map_err(|e| e.to_string())?;
    }
    Ok(t)
}

fn to_record(e: &LogEntry) -> Result<ClusterLogRecord, StoreFailure> {
    Ok(ClusterLogRecord {
        seq: i64::try_from(e.seq).map_err(failure)?,
        fence_epoch: i64::try_from(e.fence.epoch).map_err(failure)?,
        fence_node: e.fence.node.as_str().to_owned(),
        kind: e.kind.as_str().to_owned(),
        at_tai: e.at_tai_ns,
        payload: e.payload.clone(),
        prev_hash: e.prev_hash.to_vec(),
        signature: e.signature.to_vec(),
    })
}

fn from_record(r: ClusterLogRecord) -> Result<LogEntry, StoreFailure> {
    Ok(LogEntry {
        seq: u64::try_from(r.seq).map_err(failure)?,
        fence: Fence::new(
            u64::try_from(r.fence_epoch).map_err(failure)?,
            NodeId::new(&r.fence_node).map_err(|_| failure("stored node ID is invalid"))?,
        ),
        kind: EventKind::new(&r.kind).map_err(failure)?,
        at_tai_ns: r.at_tai,
        payload: r.payload,
        prev_hash: r
            .prev_hash
            .try_into()
            .map_err(|_| failure("stored hash is not 32 bytes"))?,
        signature: r
            .signature
            .try_into()
            .map_err(|_| failure("stored signature is not 64 bytes"))?,
    })
}

/// Applies a replicated entry to this station's record (step 2 of docs/integration/M11.md);
/// runs in the transaction that stores the entry.
pub type Applier = Box<dyn FnMut(&Database, &LogEntry) -> Result<(), StoreFailure> + Send>;

/// The event log in the encrypted database (its own connection).
pub struct DbLogStore {
    db: Database,
    clock: Box<dyn Clock>,
    apply: Option<Applier>,
}

impl DbLogStore {
    /// A store on `db`; `apply` applies each appended entry to the record in the same
    /// transaction (`None`: the log is kept without applying).
    pub fn new(db: Database, clock: Box<dyn Clock>, apply: Option<Applier>) -> Self {
        Self { db, clock, apply }
    }
}

impl LogStore for DbLogStore {
    fn head(&self) -> Result<ChainHead, StoreFailure> {
        match self.db.cluster_log_last().map_err(failure)? {
            None => Ok(ChainHead::genesis()),
            Some(r) => Ok(ChainHead::of(&from_record(r)?)),
        }
    }

    fn append(&mut self, entry: &LogEntry) -> Result<(), StoreFailure> {
        let record = to_record(entry)?;
        let mut outcome = Ok(());
        let apply = &mut self.apply;
        self.db
            .cluster_log_append(&record, |db| {
                if let Some(f) = apply {
                    outcome = f(db, entry);
                    if let Err(e) = &outcome {
                        return Err(caelum_store::StoreError::Invalid(e.0.clone()));
                    }
                }
                Ok(())
            })
            .map_err(failure)?;
        outcome
    }

    fn get(&self, seq: u64) -> Result<Option<LogEntry>, StoreFailure> {
        let seq = i64::try_from(seq).map_err(failure)?;
        self.db
            .cluster_log_get(seq)
            .map_err(failure)?
            .map(from_record)
            .transpose()
    }

    fn range(&self, from: u64, limit: usize) -> Result<Vec<LogEntry>, StoreFailure> {
        let from = i64::try_from(from).map_err(failure)?;
        let limit = i64::try_from(limit).map_err(failure)?;
        self.db
            .cluster_log_range(from, limit)
            .map_err(failure)?
            .into_iter()
            .map(from_record)
            .collect()
    }

    fn divert_after(&mut self, seq: u64) -> Result<Vec<LogEntry>, StoreFailure> {
        let seq = i64::try_from(seq).map_err(failure)?;
        let now = self.clock.now_tai().map_err(failure)?.0;
        self.db
            .cluster_log_divert_after(seq, now, |_, _| Ok(()))
            .map_err(failure)?
            .into_iter()
            .map(from_record)
            .collect()
    }
}

/// The highest fence seen, in the encrypted database (its own connection).
pub struct DbFenceStore {
    db: Database,
}

impl DbFenceStore {
    /// A store on `db`.
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

impl FenceStore for DbFenceStore {
    fn load(&self) -> Result<Option<Fence>, StoreFailure> {
        self.db
            .cluster_state(FENCE_KEY)
            .map_err(failure)?
            .map(|t| serde_json::from_str(&t).map_err(failure))
            .transpose()
    }

    fn save(&mut self, fence: &Fence) -> Result<(), StoreFailure> {
        let text = serde_json::to_string(fence).map_err(failure)?;
        self.db.cluster_set_state(FENCE_KEY, &text).map_err(failure)
    }
}

fn identity_of(r: &UserRecord) -> Option<UserIdentity> {
    Some(UserIdentity {
        user_id: r.id.parse::<UserId>().ok()?,
        username: r.username.clone(),
        role: Role::parse(&r.role)?,
        crew_member_id: match &r.crew_member_id {
            None => None,
            Some(c) => Some(c.parse::<CrewMemberId>().ok()?),
        },
    })
}

#[derive(Debug, Clone, Copy)]
struct Failures {
    count: u32,
    last: Instant,
}

/// Crew-LAN sign-in on this station's accounts (its own connection), with the same refusal
/// rules as the station window: disabled and machine accounts are refused, repeated failures
/// throttle the name, and every attempt is audited.
pub struct LanAuthority {
    db: Mutex<Database>,
    key: InstanceKey,
    clock: Box<dyn Clock>,
    failures: Mutex<HashMap<String, Failures>>,
}

impl LanAuthority {
    /// An authority on `db`, auditing with `key`.
    pub fn new(db: Database, key: InstanceKey, clock: Box<dyn Clock>) -> Self {
        Self {
            db: Mutex::new(db),
            key,
            clock,
            failures: Mutex::new(HashMap::new()),
        }
    }

    fn record(
        &self,
        actor: &Actor,
        action: AuditAction,
        target: Option<&str>,
        details: &serde_json::Value,
    ) {
        let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        let log = AuditLog::new(&db, &self.key, self.clock.as_ref());
        if let Err(e) = log.append(actor, action, target, details) {
            tracing::error!(error = %e, "crew-LAN audit entry could not be written");
        }
    }

    fn throttled(&self, name: &str) -> bool {
        let f = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(f) = f.get(name) else { return false };
        if f.count < MAX_FAILURES {
            return false;
        }
        let doublings = (f.count - MAX_FAILURES).min(16);
        let period = THROTTLE_BASE
            .saturating_mul(1 << doublings)
            .min(THROTTLE_MAX);
        f.last.elapsed() < period
    }

    fn fail(&self, name: &str, station: &StationIdentity, role: Option<Role>, reason: &str) {
        {
            let mut f = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
            if f.len() >= 1024 && !f.contains_key(name) {
                f.clear();
            }
            let e = f.entry(name.to_owned()).or_insert(Failures {
                count: 0,
                last: Instant::now(),
            });
            e.count = e.count.saturating_add(1);
            e.last = Instant::now();
        }
        self.record(
            &Actor::claimed(name, role),
            AuditAction::LoginFailed,
            None,
            &json!({ "reason": reason, "station": station.station_id.as_str(), "via": "crew_lan" }),
        );
    }
}

impl UserAuthority for LanAuthority {
    fn sign_in(
        &self,
        station: &StationIdentity,
        username: &str,
        secret: &[u8],
    ) -> Result<UserIdentity, SignInRefused> {
        let name = username.to_lowercase();
        if self.throttled(&name) {
            self.fail(&name, station, None, "throttled");
            return Err(SignInRefused);
        }
        let record = {
            let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
            db.user_by_username(&name).map_err(|_| SignInRefused)?
        };
        let Some(record) = record else {
            verify_against_dummy(secret);
            self.fail(&name, station, None, "invalid_credentials");
            return Err(SignInRefused);
        };
        let role = Role::parse(&record.role);
        if !verify_secret(secret, &record.credential) {
            self.fail(&name, station, role, "invalid_credentials");
            return Err(SignInRefused);
        }
        let Some(identity) = identity_of(&record) else {
            self.fail(&name, station, role, "malformed_account");
            return Err(SignInRefused);
        };
        if record.disabled {
            self.fail(&name, station, role, "disabled");
            return Err(SignInRefused);
        }
        if identity.role.is_machine() {
            self.fail(&name, station, role, "not_interactive");
            return Err(SignInRefused);
        }
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&name);
        Ok(identity)
    }

    fn is_active(&self, user: &UserIdentity) -> bool {
        let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        let id = user.user_id.to_string();
        db.users()
            .ok()
            .and_then(|all| all.into_iter().find(|u| u.id == id))
            .is_some_and(|u| !u.disabled)
    }

    fn audit(&self, event: LanAuditEvent) {
        match event {
            LanAuditEvent::SignedIn { station, user } => self.record(
                &Actor::user(&user),
                AuditAction::Login,
                None,
                &json!({ "station": station.as_str(), "via": "crew_lan" }),
            ),
            LanAuditEvent::SignInFailed { station, username } => self.record(
                &Actor::claimed(&username, None),
                AuditAction::LoginFailed,
                None,
                &json!({ "station": station.as_str(), "via": "crew_lan" }),
            ),
            LanAuditEvent::SignedOut { station, user } => self.record(
                &Actor::user(&user),
                AuditAction::Logout,
                None,
                &json!({ "station": station.as_str(), "via": "crew_lan" }),
            ),
            LanAuditEvent::EmergencyViewOpened { station } => self.record(
                &Actor::claimed(station.as_str(), None),
                AuditAction::BreakGlassOpened,
                Some(station.as_str()),
                &json!({ "via": "crew_lan" }),
            ),
            LanAuditEvent::Refused {
                station,
                endpoint,
                reason,
            } => self.record(
                &Actor::claimed(station.as_str(), None),
                AuditAction::LanRefused,
                Some(endpoint),
                &json!({ "reason": format!("{reason:?}") }),
            ),
            LanAuditEvent::UnknownStation { fingerprint } => self.record(
                &Actor::claimed("unknown-station", None),
                AuditAction::LanRefused,
                None,
                &json!({ "reason": "unknown_station", "fingerprint": fingerprint }),
            ),
        }
    }
}

fn priority(p: &str) -> Option<AlarmPriority> {
    match p {
        "critical" => Some(AlarmPriority::Critical),
        "warning" => Some(AlarmPriority::Warning),
        "observe" => Some(AlarmPriority::Observe),
        _ => None,
    }
}

/// An alarm as the crew LAN serves it: OPS fields, and the detail document for viewers who may
/// read physiology. Station-wide technical alarms (no crew member) are not crew alarms here.
pub fn lan_alarm(v: &AlarmView) -> Option<LanAlarm> {
    let crew_id = v.crew_member_id.as_deref()?.parse::<CrewMemberId>().ok()?;
    let updated = [
        Some(v.created_at_tai),
        v.acknowledged_at_tai,
        v.ground_acknowledged_at_tai,
        v.resolved_at_tai,
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(v.created_at_tai);
    let ops = OpsFields {
        alarm_id: v.id.parse::<AlarmId>().ok()?,
        crew_id,
        priority: priority(&v.priority)?,
        category: v.category.clone(),
        state: v.state.clone(),
        stage: u8::try_from(v.escalation_stage.clamp(0, i64::from(u8::MAX))).ok()?,
        created_at_tai_ns: v.created_at_tai,
        updated_at_tai_ns: updated,
    };
    let why: serde_json::Value = serde_json::from_str(&v.why).unwrap_or(serde_json::Value::Null);
    let detail = json!({
        "title": v.title,
        "rule_id": v.rule_id,
        "ruleset_version": v.ruleset_version,
        "trigger_value": v.trigger_value,
        "why": why,
        "action": v.action,
    });
    match LanAlarm::new(ops, Some(detail)) {
        Ok(a) => Some(a),
        Err(e) => {
            tracing::warn!(alarm = %v.id, error = %e, "alarm not served on the crew LAN");
            None
        }
    }
}

/// The alarm bus for the crew LAN (its own connection): a snapshot from the record, and every
/// change the shell reports through [`LanAlarms::publish`].
pub struct LanAlarms {
    db: Mutex<Database>,
    clock: Box<dyn Clock>,
    tx: broadcast::Sender<LanAlarm>,
}

impl LanAlarms {
    /// A source on `db`.
    pub fn new(db: Database, clock: Box<dyn Clock>) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            db: Mutex::new(db),
            clock,
            tx,
        }
    }

    /// Sends the current state of `alarm` to every subscriber.
    pub fn publish(&self, alarm: &str) {
        let view = {
            let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
            db.alarm_view(alarm)
        };
        if let Ok(Some(v)) = view {
            if let Some(a) = lan_alarm(&v) {
                let _ = self.tx.send(a);
            }
        }
    }
}

impl AlarmSource for LanAlarms {
    fn snapshot(&self) -> Result<Vec<LanAlarm>, SourceUnavailable> {
        let now = self.clock.now_tai().map_err(|_| SourceUnavailable)?.0;
        let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
        let all = db
            .alarms_recent(now.saturating_sub(ENDED_WINDOW_NS))
            .map_err(|_| SourceUnavailable)?;
        Ok(all.iter().filter_map(lan_alarm).collect())
    }

    fn subscribe(&self) -> broadcast::Receiver<LanAlarm> {
        self.tx.subscribe()
    }
}
