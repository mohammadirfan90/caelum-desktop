//! M11 integration in the core (F-SYS-04, F-SYS-05, R-OPS-02; CR-017, docs/integration/M11.md):
//! the signed `cluster` configuration, the event log and fence in the encrypted database
//! (append, chain, quarantine, restart), crew-LAN sign-in on the station's accounts with
//! throttling and audit, the alarm source's OPS mapping, and a real node on these stores that
//! promotes itself, appends to the encrypted log and signs a user in over mutual TLS.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use caelum_cluster::failover::InstanceRole;
use caelum_cluster::fence::{Fence, NodeId};
use caelum_cluster::lan::access::AlarmPriority;
use caelum_cluster::lan::server::{AlarmSource, UserAuthority};
use caelum_cluster::lan::tls::TlsMaterial;
use caelum_cluster::lan::{
    fingerprint, Fingerprint, StationIdentity, StationKind, StationRegistry,
};
use caelum_cluster::log::{ChainHead, EntryBody, EventKind, InstanceSigner, LogEntry};
use caelum_cluster::node::{Node, NodeConfig, NodeParts};
use caelum_cluster::replication::{FenceStore, LogStore};
use caelum_cluster::timing::Timing;
use caelum_core::cluster::{
    lan_alarm, ClusterConfig, DbFenceStore, DbLogStore, LanAlarms, LanAuthority, CLUSTER_KIND,
};
use caelum_core::security::keys::{database_key, instance_key, IfMissing, InstanceKey};
use caelum_core::security::role::Role;
use caelum_core::station::{Station, StationError, DATABASE_FILE};
use caelum_core::store::{AlarmView, Database};
use common::{open, DataDir, MemoryKeyStore, TestClock};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair,
};

const ADMIN: &str = "ground station passphrase";
const CREW_A: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a41";

struct World {
    dir: DataDir,
    keys: MemoryKeyStore,
    clock: TestClock,
    station: Station,
}

impl World {
    fn db(&self) -> Database {
        let (key, _) = database_key(&self.keys, IfMissing::Fail).unwrap();
        Database::open(&self.dir.0.join(DATABASE_FILE), &key).unwrap()
    }

    fn key(&self) -> InstanceKey {
        instance_key(&self.keys, IfMissing::Fail).unwrap().0
    }

    fn actions(&self) -> Vec<String> {
        self.db()
            .audit_records()
            .unwrap()
            .into_iter()
            .map(|e| e.action)
            .collect()
    }
}

fn world() -> World {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    clock
        .0
        .tai
        .store(2_100_000_000_000_000_000, Ordering::SeqCst);
    let mut station = open(&dir, &keys, &clock).unwrap();
    station.create_first_administrator("admin", ADMIN).unwrap();
    station.login("admin", ADMIN.as_bytes()).unwrap();
    station.reauth(ADMIN.as_bytes()).unwrap();
    station
        .user_create("fs.one", Role::FlightSurgeon, None, ADMIN)
        .unwrap();
    World {
        dir,
        keys,
        clock,
        station,
    }
}

fn fixture(name: &str) -> (Vec<u8>, String) {
    let d = format!("{}/tests/fixtures/cluster/", env!("CARGO_MANIFEST_DIR"));
    (
        std::fs::read(format!("{d}{name}.toml")).unwrap(),
        std::fs::read_to_string(format!("{d}{name}.sig.toml")).unwrap(),
    )
}

fn node(id: &str) -> NodeId {
    NodeId::new(id).unwrap()
}

fn body(n: u8) -> EntryBody {
    EntryBody {
        kind: EventKind::new("audit").unwrap(),
        at_tai_ns: i64::from(n),
        payload: vec![n],
    }
}

const CA: &str = "-----BEGIN CERTIFICATE-----\nMA==\n-----END CERTIFICATE-----";

fn config_text(extra: &str) -> String {
    format!(
        "[cluster]\nversion = \"1\"\nnode_id = \"caelum-a\"\nlisten = \"127.0.0.1:0\"\nca_pem = \"\"\"\n{CA}\n\"\"\"\nstations = []\n{extra}"
    )
}

#[test]
fn f_sys_04_cluster_configuration_is_checked_and_nothing_is_defaulted() {
    let peer = |node: &str, fp: &str| {
        format!(
            "[cluster.peer]\nnode_id = \"{node}\"\naddress = \"127.0.0.1:47001\"\nserver_name = \"localhost\"\nfingerprint = \"{fp}\"\npublic_key = \"{}\"\n",
            "cd".repeat(32)
        )
    };
    let plan = ClusterConfig::parse(&config_text(&peer("caelum-b", &"ab".repeat(32)))).unwrap();
    assert_eq!(plan.node, node("caelum-a"));
    let (p, key) = plan.peer.unwrap();
    assert_eq!((p.node, key), (node("caelum-b"), [0xcd; 32]));
    let peer_id = plan.registry.get(&[0xab; 32]).unwrap();
    assert_eq!(peer_id.kind, StationKind::Peer);
    assert!(ClusterConfig::parse(&config_text(""))
        .unwrap()
        .peer
        .is_none());

    for bad in [
        config_text(&peer("caelum-a", &"ab".repeat(32))), // the peer is this node
        config_text(&peer("caelum-b", "abcd")),          // short fingerprint
        config_text("").replace("127.0.0.1:0", "nowhere"), // not ip:port
        config_text("").replace(CA, "not a certificate"),
        config_text("").replace("version = \"1\"\n", ""), // a missing value is not defaulted
        config_text("unknown = 1\n"),
        config_text(&format!(
            "{}\n[[cluster.stations]]\nstation_id = \"tab-1\"\nkind = \"secondary\"\nfingerprint = \"{}\"\n",
            peer("caelum-b", &"ab".repeat(32)),
            "ab".repeat(32)
        ))
        .replace("stations = []\n", ""), // one certificate listed twice
        config_text(&format!(
            "[[cluster.stations]]\nstation_id = \"tab-1\"\nkind = \"display\"\nfingerprint = \"{}\"\n",
            "ef".repeat(32)
        ))
        .replace("stations = []\n", ""), // unknown kind
    ] {
        assert!(ClusterConfig::parse(&bad).is_err(), "accepted:\n{bad}");
    }
}

#[test]
fn f_sys_04_the_signed_cluster_configuration_is_applied_by_an_administrator_only() {
    let mut w = world();
    let (b, s) = fixture("cluster-v1");
    assert_eq!(w.station.config_apply_cluster(&b, &s).unwrap(), "1");
    let plan = caelum_core::cluster::config_in_force(&w.db())
        .unwrap()
        .unwrap();
    assert_eq!(plan.node, node("caelum-a"));
    assert!(plan
        .registry
        .get(&[0xef; 32])
        .is_some_and(|k| k.kind == StationKind::Kiosk));
    // A changed byte breaks the signature.
    let mut tampered = b.clone();
    let i = tampered.iter().position(|&c| c == b'a').unwrap();
    tampered[i] = b'b';
    assert!(matches!(
        w.station.config_apply_cluster(&tampered, &s),
        Err(StationError::Rejected(_))
    ));
    assert!(matches!(
        w.station.cluster_identity_set("no", "no"),
        Err(StationError::Rejected(_))
    ));
    w.station.logout().unwrap();
    w.station.login("fs.one", ADMIN.as_bytes()).unwrap();
    assert!(matches!(
        w.station.config_apply_cluster(&b, &s),
        Err(StationError::Denied(_))
    ));
    assert!(w.actions().iter().any(|a| a == "config.apply"));
    let _ = CLUSTER_KIND;
}

#[test]
fn f_sys_04_r_ops_02_the_log_and_fence_survive_a_restart_and_unacknowledged_entries_are_quarantined(
) {
    let w = world();
    let signer = InstanceSigner::new(node("caelum-a"), w.key());
    let fence = Fence::new(1, node("caelum-a"));
    let mut log = DbLogStore::new(w.db(), Box::new(w.clock.clone()), None);
    assert_eq!(log.head().unwrap(), ChainHead::genesis());
    let mut head = ChainHead::genesis();
    let mut written = Vec::new();
    for n in 1..=4 {
        let e = LogEntry::create(&head, fence.clone(), body(n), &signer).unwrap();
        log.append(&e).unwrap();
        head = ChainHead::of(&e);
        written.push(e);
    }
    // A second append at the same position is refused (no gaps, no overwrites).
    assert!(log.append(&written[1]).is_err());
    drop(log);

    // Restart: same head, same entries, byte for byte.
    let mut log = DbLogStore::new(w.db(), Box::new(w.clock.clone()), None);
    assert_eq!(log.head().unwrap(), head);
    assert_eq!(log.get(2).unwrap().unwrap(), written[1]);
    assert_eq!(log.range(2, 2).unwrap(), written[1..3].to_vec());
    assert!(log.get(9).unwrap().is_none());

    // Entries after 2 move to quarantine, are returned, and leave the log.
    let moved = log.divert_after(2).unwrap();
    assert_eq!(moved, written[2..].to_vec());
    assert_eq!(log.head().unwrap(), ChainHead::of(&written[1]));
    assert_eq!(w.db().cluster_quarantine().unwrap().len(), 2);

    let mut fences = DbFenceStore::new(w.db());
    assert_eq!(fences.load().unwrap(), None);
    fences.save(&Fence::new(7, node("caelum-b"))).unwrap();
    assert_eq!(
        DbFenceStore::new(w.db()).load().unwrap(),
        Some(Fence::new(7, node("caelum-b")))
    );
}

fn tablet() -> StationIdentity {
    StationIdentity {
        station_id: node("tab-1"),
        kind: StationKind::Secondary,
    }
}

#[test]
fn f_sys_05_crew_lan_sign_in_uses_the_station_accounts_throttles_and_audits() {
    let w = world();
    let auth = LanAuthority::new(w.db(), w.key(), Box::new(w.clock.clone()));
    let me = auth.sign_in(&tablet(), "FS.ONE", ADMIN.as_bytes()).unwrap();
    assert_eq!(
        (me.username.as_str(), me.role),
        ("fs.one", Role::FlightSurgeon)
    );
    assert!(auth.is_active(&me));
    assert!(auth.sign_in(&tablet(), "fs.one", b"000000").is_err());
    assert!(auth.sign_in(&tablet(), "nobody", b"000000").is_err());
    // Five failures throttle the name: even the right secret is refused for a while.
    for _ in 0..5 {
        let _ = auth.sign_in(&tablet(), "fs.one", b"000000");
    }
    assert!(auth.sign_in(&tablet(), "fs.one", ADMIN.as_bytes()).is_err());
    let failed = w
        .db()
        .audit_records()
        .unwrap()
        .into_iter()
        .filter(|e| e.action == "auth.login_failed" && e.details.contains("crew_lan"))
        .count();
    assert!(failed >= 8, "{failed}");
}

#[test]
fn f_sys_05_disabled_accounts_are_refused_and_lose_their_sessions() {
    let mut w = world();
    let auth = LanAuthority::new(w.db(), w.key(), Box::new(w.clock.clone()));
    let me = auth.sign_in(&tablet(), "fs.one", ADMIN.as_bytes()).unwrap();
    w.station.user_disable(me.user_id).unwrap();
    assert!(!auth.is_active(&me));
    assert!(auth.sign_in(&tablet(), "fs.one", ADMIN.as_bytes()).is_err());
}

fn alarm_view(crew: Option<&str>, priority: &str, category: &str) -> AlarmView {
    AlarmView {
        id: "7c0b5a9e-2f3d-4a51-8b1e-0a2b3c4d5e6f".into(),
        crew_member_id: crew.map(str::to_owned),
        kind: "physiological".into(),
        category: category.into(),
        rule_id: "spo2.low.critical".into(),
        ruleset_version: "1.0.0-draft.2".into(),
        priority: priority.into(),
        state: "active".into(),
        title: Some("Low oxygen saturation".into()),
        trigger_value: Some(84.0),
        why: "{\"value\":84}".into(),
        action: None,
        created_at_tai: 100,
        acknowledged_by: None,
        acknowledged_at_tai: Some(150),
        ground_acknowledged_by: None,
        ground_acknowledged_at_tai: None,
        silenced_until_tai: None,
        resolved_at_tai: None,
        escalation_stage: 2,
    }
}

#[test]
fn f_sys_05_alarms_go_to_the_lan_with_ops_fields_and_detail_kept_apart() {
    let a = lan_alarm(&alarm_view(Some(CREW_A), "critical", "RESPIRATORY")).unwrap();
    assert_eq!(
        a.ops().category,
        "respiratory",
        "stored categories are upper case"
    );
    let ops = a.ops();
    assert_eq!(ops.priority, AlarmPriority::Critical);
    assert_eq!((ops.state.as_str(), ops.stage), ("active", 2));
    assert_eq!((ops.created_at_tai_ns, ops.updated_at_tai_ns), (100, 150));
    // Station-wide technical alarms, unknown priorities and free-text categories are not served.
    assert!(lan_alarm(&alarm_view(None, "critical", "oxygenation")).is_none());
    assert!(lan_alarm(&alarm_view(Some(CREW_A), "technical", "oxygenation")).is_none());
    assert!(lan_alarm(&alarm_view(Some(CREW_A), "critical", "Free Text!")).is_none());

    let w = world();
    let source = LanAlarms::new(w.db(), Box::new(w.clock.clone()));
    assert!(source.snapshot().unwrap().is_empty());
}

struct Pki(CertifiedIssuer<'static, KeyPair>);

impl Pki {
    fn new() -> Self {
        let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        p.distinguished_name
            .push(DnType::CommonName, "test crew-LAN CA");
        Self(CertifiedIssuer::self_signed(p, KeyPair::generate().unwrap()).unwrap())
    }

    fn station(&self, name: &str) -> (TlsMaterial, Fingerprint) {
        let key = KeyPair::generate().unwrap();
        let mut p =
            CertificateParams::new(vec!["localhost".to_owned(), "127.0.0.1".to_owned()]).unwrap();
        p.distinguished_name.push(DnType::CommonName, name);
        p.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        let cert = p.signed_by(&key, &*self.0).unwrap();
        let fp = fingerprint(cert.der());
        (
            TlsMaterial::from_pem(&self.0.pem(), &cert.pem(), &key.serialize_pem()).unwrap(),
            fp,
        )
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn f_sys_04_f_sys_05_a_single_node_on_the_encrypted_stores_acts_and_serves_the_crew_lan() {
    let w = world();
    let pki = Pki::new();
    let (server_tls, _) = pki.station("caelum-a");
    let (tab_tls, tab_fp) = pki.station("tab-1");
    let mut registry = StationRegistry::new();
    registry.insert(tab_fp, tablet());
    let own = w.key();
    let mut trust = caelum_cluster::log::TrustedSigners::new();
    trust
        .insert(node("caelum-a"), &own.verifying_key().to_bytes())
        .unwrap();
    let mut config = NodeConfig::new(node("caelum-a"), "127.0.0.1:0".parse().unwrap());
    config.timing = Timing {
        heartbeat_interval: Duration::from_millis(100),
        missed_heartbeats: 5,
        tick_interval: Duration::from_millis(20),
        device_reconnect_budget: Duration::from_secs(1),
        connect_retry: Duration::from_millis(50),
    };
    let parts = NodeParts {
        log: Box::new(DbLogStore::new(w.db(), Box::new(w.clock.clone()), None)),
        fences: Box::new(DbFenceStore::new(w.db())),
        signer: Box::new(InstanceSigner::new(node("caelum-a"), own)),
        trust,
        tls: server_tls,
        registry,
        authority: Arc::new(LanAuthority::new(
            w.db(),
            w.key(),
            Box::new(w.clock.clone()),
        )),
        alarms: Arc::new(LanAlarms::new(w.db(), Box::new(w.clock.clone()))),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let n = Node::start_on(listener, config, parts).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while n.status().role != InstanceRole::Primary && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(n.status().role, InstanceRole::Primary);
    // The promotion fence is in the encrypted database.
    assert!(DbFenceStore::new(w.db()).load().unwrap().is_some());

    let seq = n.append(body(1)).unwrap();
    assert_eq!(seq, 1);
    assert_eq!(w.db().cluster_log_last().unwrap().unwrap().payload, vec![1]);

    // A provisioned tablet signs in with a station account over mutual TLS.
    let sign_in = |secret: &'static str| {
        let tls = tab_tls.client_config().unwrap();
        async move {
            let stream = caelum_cluster::lan::client::connect(tls, addr, "localhost", None)
                .await
                .unwrap();
            caelum_cluster::lan::client::request(
                stream,
                "POST".parse().unwrap(),
                "/v1/sessions",
                None,
                Some(
                    serde_json::to_vec(
                        &serde_json::json!({ "username": "fs.one", "secret": secret }),
                    )
                    .unwrap(),
                ),
            )
            .await
            .unwrap()
            .0
            .as_u16()
        }
    };
    assert_eq!(sign_in(ADMIN).await, 200);
    assert_ne!(sign_in("000000").await, 200);
    drop(n);
    assert!(w.actions().iter().any(|a| a == "auth.login"));
}
