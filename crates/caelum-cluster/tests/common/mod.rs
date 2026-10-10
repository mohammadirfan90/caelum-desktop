//! Test support: in-memory stores, test PKI (generated per test run), test authority and alarm
//! bus, node assembly. Never fabricates crew data: alarms are built explicitly by each test.
#![allow(dead_code, missing_docs, clippy::expect_used, clippy::unwrap_used)] // test code

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use caelum_cluster::fence::{Fence, NodeId};
use caelum_cluster::lan::access::{AlarmPriority, LanAlarm, OpsFields};
use caelum_cluster::lan::client;
use caelum_cluster::lan::server::{
    AlarmSource, LanAuditEvent, SignInRefused, SourceUnavailable, UserAuthority,
};
use caelum_cluster::lan::tls::TlsMaterial;
use caelum_cluster::lan::{
    fingerprint, Fingerprint, StationIdentity, StationKind, StationRegistry,
};
use caelum_cluster::log::{
    ChainHead, EntryBody, EventKind, InstanceSigner, LogEntry, TrustedSigners,
};
use caelum_cluster::node::{NodeConfig, NodeParts};
use caelum_cluster::replication::{FenceStore, LogStore};
use caelum_cluster::timing::Timing;
use caelum_cluster::StoreFailure;
use caelum_domain::ids::{AlarmId, CrewMemberId, UserId};
use caelum_security::keys::InstanceKey;
use caelum_security::role::{Role, UserIdentity};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair,
};
use tokio::sync::broadcast;

pub fn node(id: &str) -> NodeId {
    NodeId::new(id).unwrap()
}

// ---------- stores ----------

#[derive(Default)]
pub struct MemLogInner {
    pub entries: Vec<LogEntry>,
    pub quarantine: Vec<LogEntry>,
    pub fail_appends: bool,
}

/// In-memory log; clones share the same log, so a test keeps reading it after a node is killed.
#[derive(Clone, Default)]
pub struct MemLog(pub Arc<Mutex<MemLogInner>>);

impl MemLog {
    pub fn entries(&self) -> Vec<LogEntry> {
        self.0.lock().unwrap().entries.clone()
    }
    pub fn quarantine(&self) -> Vec<LogEntry> {
        self.0.lock().unwrap().quarantine.clone()
    }
    pub fn len(&self) -> usize {
        self.0.lock().unwrap().entries.len()
    }
    pub fn set_failing(&self, fail: bool) {
        self.0.lock().unwrap().fail_appends = fail;
    }
}

impl LogStore for MemLog {
    fn head(&self) -> Result<ChainHead, StoreFailure> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .entries
            .last()
            .map_or_else(ChainHead::genesis, ChainHead::of))
    }
    fn append(&mut self, entry: &LogEntry) -> Result<(), StoreFailure> {
        let mut inner = self.0.lock().unwrap();
        if inner.fail_appends {
            return Err(StoreFailure("disk full".into()));
        }
        inner.entries.push(entry.clone());
        Ok(())
    }
    fn get(&self, seq: u64) -> Result<Option<LogEntry>, StoreFailure> {
        let inner = self.0.lock().unwrap();
        Ok(seq
            .checked_sub(1)
            .and_then(|i| inner.entries.get(i as usize))
            .cloned())
    }
    fn range(&self, from: u64, limit: usize) -> Result<Vec<LogEntry>, StoreFailure> {
        let inner = self.0.lock().unwrap();
        let start = from.saturating_sub(1) as usize;
        Ok(inner
            .entries
            .iter()
            .skip(start)
            .take(limit)
            .cloned()
            .collect())
    }
    fn divert_after(&mut self, seq: u64) -> Result<Vec<LogEntry>, StoreFailure> {
        let mut inner = self.0.lock().unwrap();
        let diverted = inner.entries.split_off(seq as usize);
        inner.quarantine.extend(diverted.iter().cloned());
        Ok(diverted)
    }
}

#[derive(Clone, Default)]
pub struct MemFences(pub Arc<Mutex<Option<Fence>>>);

impl MemFences {
    pub fn get(&self) -> Option<Fence> {
        self.0.lock().unwrap().clone()
    }
}

impl FenceStore for MemFences {
    fn load(&self) -> Result<Option<Fence>, StoreFailure> {
        Ok(self.get())
    }
    fn save(&mut self, fence: &Fence) -> Result<(), StoreFailure> {
        *self.0.lock().unwrap() = Some(fence.clone());
        Ok(())
    }
}

// ---------- keys and entries ----------

pub fn key(seed: u8) -> InstanceKey {
    InstanceKey::from_seed(&[seed; 32])
}

pub fn signer(id: &str, seed: u8) -> InstanceSigner {
    InstanceSigner::new(node(id), key(seed))
}

/// Trust for nodes `a` (seed 1) and `b` (seed 2).
pub fn trust_ab() -> TrustedSigners {
    let mut t = TrustedSigners::new();
    t.insert(node("a"), &key(1).verifying_key().to_bytes())
        .unwrap();
    t.insert(node("b"), &key(2).verifying_key().to_bytes())
        .unwrap();
    t
}

pub fn body(i: u64) -> EntryBody {
    EntryBody {
        kind: EventKind::new("observation").unwrap(),
        at_tai_ns: 1_000_000_000 + i as i64,
        payload: format!("event-{i}").into_bytes(),
    }
}

// ---------- PKI ----------

pub struct Pki {
    ca: CertifiedIssuer<'static, KeyPair>,
}

impl Pki {
    pub fn new(name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.distinguished_name.push(DnType::CommonName, name);
        let ca = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
        Self { ca }
    }

    /// A station certificate for `localhost` / 127.0.0.1, usable as server and client.
    pub fn station(&self, name: &str) -> (TlsMaterial, Fingerprint) {
        let key = KeyPair::generate().unwrap();
        let mut params =
            CertificateParams::new(vec!["localhost".to_owned(), "127.0.0.1".to_owned()]).unwrap();
        params.distinguished_name.push(DnType::CommonName, name);
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        let cert = params.signed_by(&key, &*self.ca).unwrap();
        let fp = fingerprint(cert.der());
        let tls = TlsMaterial::from_pem(&self.ca.pem(), &cert.pem(), &key.serialize_pem()).unwrap();
        (tls, fp)
    }

    pub fn ca_der(&self) -> rustls::pki_types::CertificateDer<'static> {
        self.ca.der().clone()
    }
}

pub fn station(id: &str, kind: StationKind) -> StationIdentity {
    StationIdentity {
        station_id: node(id),
        kind,
    }
}

// ---------- users, audit, alarms ----------

pub fn crew(n: u8) -> CrewMemberId {
    format!("00000000-0000-4000-8000-0000000000{n:02x}")
        .parse()
        .unwrap()
}

pub fn user(name: &str, role: Role, crew_member: Option<CrewMemberId>, n: u8) -> UserIdentity {
    UserIdentity {
        user_id: format!("10000000-0000-4000-8000-0000000000{n:02x}")
            .parse::<UserId>()
            .unwrap(),
        username: name.into(),
        role,
        crew_member_id: crew_member,
    }
}

#[derive(Default)]
pub struct TestAuthority {
    pub users: Mutex<HashMap<String, (String, UserIdentity, bool)>>,
    pub audit: Mutex<Vec<LanAuditEvent>>,
}

impl TestAuthority {
    pub fn with_crew() -> Arc<Self> {
        let a = Arc::new(Self::default());
        a.add(
            "astro1",
            "1111",
            user("astro1", Role::Astronaut, Some(crew(1)), 1),
        );
        a.add("cmo", "2222", user("cmo", Role::Cmo, Some(crew(2)), 2));
        a.add("mcc", "3333", user("mcc", Role::Mcc, None, 3));
        a.add("admin", "4444", user("admin", Role::Administrator, None, 4));
        a.add("bme", "5555", user("bme", Role::Bme, None, 5));
        a.add(
            "gateway",
            "6666",
            user("gateway", Role::DeviceGateway, None, 6),
        );
        a
    }
    pub fn add(&self, name: &str, secret: &str, identity: UserIdentity) {
        self.users
            .lock()
            .unwrap()
            .insert(name.into(), (secret.into(), identity, true));
    }
    pub fn disable(&self, name: &str) {
        if let Some(u) = self.users.lock().unwrap().get_mut(name) {
            u.2 = false;
        }
    }
    pub fn events(&self) -> Vec<LanAuditEvent> {
        self.audit.lock().unwrap().clone()
    }
}

impl UserAuthority for TestAuthority {
    fn sign_in(
        &self,
        _station: &StationIdentity,
        username: &str,
        secret: &[u8],
    ) -> Result<UserIdentity, SignInRefused> {
        match self.users.lock().unwrap().get(username) {
            Some((s, identity, true)) if s.as_bytes() == secret => Ok(identity.clone()),
            _ => Err(SignInRefused),
        }
    }
    fn is_active(&self, user: &UserIdentity) -> bool {
        self.users
            .lock()
            .unwrap()
            .get(&user.username)
            .is_some_and(|u| u.2)
    }
    fn audit(&self, event: LanAuditEvent) {
        self.audit.lock().unwrap().push(event);
    }
}

pub struct TestAlarms {
    pub current: Mutex<Vec<LanAlarm>>,
    pub tx: broadcast::Sender<LanAlarm>,
}

impl TestAlarms {
    pub fn new(initial: Vec<LanAlarm>) -> Arc<Self> {
        Arc::new(Self {
            current: Mutex::new(initial),
            tx: broadcast::channel(64).0,
        })
    }
    pub fn publish(&self, alarm: LanAlarm) {
        let mut current = self.current.lock().unwrap();
        current.retain(|a| a.ops().alarm_id != alarm.ops().alarm_id);
        current.push(alarm.clone());
        let _ = self.tx.send(alarm);
    }
}

impl AlarmSource for TestAlarms {
    fn snapshot(&self) -> Result<Vec<LanAlarm>, SourceUnavailable> {
        Ok(self.current.lock().unwrap().clone())
    }
    fn subscribe(&self) -> broadcast::Receiver<LanAlarm> {
        self.tx.subscribe()
    }
}

pub fn alarm(n: u8, crew_member: CrewMemberId, priority: AlarmPriority) -> LanAlarm {
    LanAlarm::new(
        OpsFields {
            alarm_id: format!("20000000-0000-4000-8000-0000000000{n:02x}")
                .parse::<AlarmId>()
                .unwrap(),
            crew_id: crew_member,
            priority,
            category: "cardiac".into(),
            state: "active".into(),
            stage: 1,
            created_at_tai_ns: 1_000,
            updated_at_tai_ns: 2_000,
        },
        Some(serde_json::json!({ "parameter": "heart_rate", "value": 182.0, "rule": "hr.high.critical" })),
    )
    .unwrap()
}

// ---------- nodes ----------

/// Fast timing for tests that are not about the 10 s budget itself.
pub fn fast_timing() -> Timing {
    Timing {
        heartbeat_interval: Duration::from_millis(100),
        missed_heartbeats: 5,
        tick_interval: Duration::from_millis(20),
        device_reconnect_budget: Duration::from_secs(1),
        connect_retry: Duration::from_millis(50),
    }
}

pub struct NodeSetup {
    pub log: MemLog,
    pub fences: MemFences,
}

#[allow(clippy::too_many_arguments)]
pub fn parts(
    tls: TlsMaterial,
    id: &str,
    seed: u8,
    log: &MemLog,
    fences: &MemFences,
    registry: StationRegistry,
    authority: Arc<TestAuthority>,
    alarms: Arc<TestAlarms>,
) -> NodeParts {
    NodeParts {
        log: Box::new(log.clone()),
        fences: Box::new(fences.clone()),
        signer: Box::new(signer(id, seed)),
        trust: trust_ab(),
        tls,
        registry,
        authority,
        alarms,
    }
}

pub fn config(id: &str, timing: Timing) -> NodeConfig {
    let mut c = NodeConfig::new(node(id), "127.0.0.1:0".parse().unwrap());
    c.timing = timing;
    c
}

pub async fn wait_until(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    check()
}

pub async fn http(
    tls: &TlsMaterial,
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> Result<(u16, serde_json::Value), client::ClientError> {
    let stream = client::connect(tls.client_config().unwrap(), addr, "localhost", None).await?;
    let (status, bytes) = client::request(
        stream,
        method.parse().unwrap(),
        path,
        token,
        body.map(|b| serde_json::to_vec(&b).unwrap()),
    )
    .await?;
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    Ok((status.as_u16(), json))
}

pub async fn sign_in(tls: &TlsMaterial, addr: SocketAddr, name: &str, secret: &str) -> String {
    let (status, body) = http(
        tls,
        addr,
        "POST",
        "/v1/sessions",
        None,
        Some(serde_json::json!({ "username": name, "secret": secret })),
    )
    .await
    .unwrap();
    assert_eq!(status, 200, "sign-in of {name}: {body}");
    body["token"].as_str().unwrap().to_owned()
}
