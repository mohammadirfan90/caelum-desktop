//! M11 replication through two stations (F-SYS-04, R-OPS-02, SR-007; CR-017,
//! docs/integration/M11.md items 1–2): with capture on, every write of the primary's record
//! becomes a signed `row` entry; the standby applies each in the transaction that stores it,
//! keeps the primary's audit chain apart from its own, refuses writes and runs no alarm logic,
//! and on promotion resumes the replicated alarm at its escalation stage.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::sync::atomic::Ordering;

use caelum_cluster::fence::{Fence, NodeId};
use caelum_cluster::log::{ChainHead, InstanceSigner, LogEntry};
use caelum_cluster::replication::LogStore;
use caelum_core::acquisition::Acquisition;
use caelum_core::cluster::{pending_rows, row_applier, DbLogStore};
use caelum_core::security::keys::{database_key, instance_key, IfMissing};
use caelum_core::security::role::Role;
use caelum_core::station::{Station, StationError, DATABASE_FILE, STANDBY_REFUSAL};
use caelum_core::store::Database;
use caelum_devices::registry::{DeviceIdentity, Registry};
use caelum_domain::ids::CrewMemberId;
use caelum_domain::time::TaiNs;
use caelum_test_equipment::sig::{self as emu, uuid as gatt};
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN: &str = "ground station passphrase";
const CREW_A: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a41";
const CREW_B: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a42";
const T0: i64 = 2_100_000_000_000_000_000;
const S: i64 = 1_000_000_000;

fn crew(id: &str) -> CrewMemberId {
    CrewMemberId::from_uuid(uuid::Uuid::parse_str(id).unwrap())
}

struct Node {
    dir: DataDir,
    keys: MemoryKeyStore,
    clock: TestClock,
    station: Station,
}

impl Node {
    fn new() -> Self {
        let dir = DataDir::new();
        let keys = MemoryKeyStore::default();
        let clock = TestClock::default();
        clock.0.tai.store(T0, Ordering::SeqCst);
        let station = open(&dir, &keys, &clock).unwrap();
        Self {
            dir,
            keys,
            clock,
            station,
        }
    }

    fn db(&self) -> Database {
        let (key, _) = database_key(&self.keys, IfMissing::Fail).unwrap();
        Database::open(&self.dir.0.join(DATABASE_FILE), &key).unwrap()
    }

    fn at(&self, t: i64) {
        self.clock.0.tai.store(t, Ordering::SeqCst);
    }
}

/// Primary "a" sending to standby "b" (the node driver's job, done here by hand).
struct Link {
    signer: InstanceSigner,
    fence: Fence,
    head: ChainHead,
    standby: DbLogStore,
    primary_db: Database,
}

impl Link {
    /// Sends everything queued on the primary; returns how many entries.
    fn sync(&mut self, now: i64) -> usize {
        let mut n = 0;
        loop {
            let (last, bodies) = pending_rows(&self.primary_db, 64, now).unwrap();
            let Some(last) = last else { return n };
            for body in bodies {
                let e =
                    LogEntry::create(&self.head, self.fence.clone(), body, &self.signer).unwrap();
                self.standby.append(&e).unwrap();
                self.head = ChainHead::of(&e);
                n += 1;
            }
            self.primary_db.cluster_outbox_done(last).unwrap();
        }
    }
}

fn spo2(
    a: &mut Node,
    acq: &mut Acquisition,
    device: caelum_domain::ids::DeviceId,
    pct: i16,
    t: i64,
) {
    a.at(t);
    let payload = emu::plx_continuous(emu::sfloat(pct, 0), emu::sfloat(80, 0), 0);
    let out = acq
        .notification(
            a.station.db(),
            device,
            gatt::PLX_CONTINUOUS,
            &payload,
            TaiNs(t),
        )
        .unwrap();
    for obs in &out.observations {
        a.station.observe(obs).unwrap();
    }
    a.station.tick().unwrap();
}

fn alarm_rows(db: &Database, rule: &str) -> Vec<(String, i64)> {
    let conn = db.connection();
    let mut stmt = conn
        .prepare(
            "SELECT id, escalation_stage FROM alarm WHERE rule_id = ?1 AND resolved_at_tai IS NULL",
        )
        .unwrap();
    stmt.query_map([rule], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn f_sys_04_r_ops_02_the_standby_holds_the_record_and_resumes_the_alarm_on_promotion() {
    let mut a = Node::new();
    let mut b = Node::new();
    // The pair is formed before any record exists on b; a held its record already.
    a.station
        .create_first_administrator("admin", ADMIN)
        .unwrap();
    a.station.login("admin", ADMIN.as_bytes()).unwrap();
    a.station.reauth(ADMIN.as_bytes()).unwrap();
    let fx = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/roster/");
    a.station
        .config_apply_crew(
            &std::fs::read(format!("{fx}crew-v1.toml")).unwrap(),
            &std::fs::read_to_string(format!("{fx}crew-v1.sig.toml")).unwrap(),
        )
        .unwrap();
    a.station
        .user_create("cmo.b", Role::Cmo, Some(crew(CREW_B)), "135792")
        .unwrap();

    // a becomes primary: capture on, and what it held before is queued once.
    let primary_db = a.db();
    primary_db.cluster_set_capture(true).unwrap();
    assert!(primary_db.cluster_seed_outbox().unwrap());
    assert!(!primary_db.cluster_seed_outbox().unwrap(), "seeded once");
    let (a_key, _) = instance_key(&a.keys, IfMissing::Fail).unwrap();
    let node_a = NodeId::new("caelum-a").unwrap();
    let mut link = Link {
        signer: InstanceSigner::new(node_a.clone(), a_key),
        fence: Fence::new(1, node_a),
        head: ChainHead::genesis(),
        standby: DbLogStore::new(b.db(), Box::new(b.clock.clone()), Some(row_applier())),
        primary_db,
    };
    b.station.stand_by();
    assert!(link.sync(T0) > 0);

    // The roster and accounts arrived: a's CMO signs in on b with a's credentials.
    b.station.login("cmo.b", b"135792").unwrap();
    assert_eq!(b.station.crew_list().unwrap().len(), 2);

    // An alarm on a: device registered and assigned on a, critical SpO2 for 20 s.
    let device = Registry::new(a.station.db())
        .register(&DeviceIdentity {
            adapter: "ble-sig".into(),
            vendor: "V".into(),
            model: "Oximeter X".into(),
            serial: "OX-1".into(),
            firmware: None,
            accuracy_class: None,
            calibration_due: None,
        })
        .unwrap();
    a.station.logout().unwrap();
    a.station.login("cmo.b", b"135792").unwrap();
    a.station.reauth(b"135792").unwrap();
    a.station.device_assign(device, crew(CREW_A), None).unwrap();
    let mut acq = Acquisition::new().unwrap();
    for k in 0..=40 {
        spo2(&mut a, &mut acq, device, 80, T0 + k * S);
    }
    let on_a = alarm_rows(a.station.db(), "spo2.critical");
    assert_eq!(on_a.len(), 1);
    link.sync(T0 + 40 * S);

    // b holds the same alarm at the same stage, the observations, and a's audit chain apart
    // from its own.
    let on_b = alarm_rows(b.station.db(), "spo2.critical");
    assert_eq!(on_b, on_a, "same alarm, same stage");
    assert!(!b
        .station
        .db()
        .observations(CREW_A, "spo2")
        .unwrap()
        .is_empty());
    assert!(b.station.db().audit_replica_count("caelum-a").unwrap() > 0);
    assert!(b
        .station
        .db()
        .audit_records()
        .unwrap()
        .iter()
        .all(|e| e.action != "auth.login" || e.actor != "admin"));

    // The standby runs no alarm logic and refuses writes.
    b.at(T0 + 120 * S);
    assert!(b.station.tick().unwrap().is_empty());
    b.station.reauth(b"135792").unwrap();
    let alarm_id = on_b[0].0.parse().unwrap();
    assert!(matches!(
        b.station.alarm_acknowledge(alarm_id),
        Err(StationError::Rejected(r)) if r == STANDBY_REFUSAL
    ));
    assert_eq!(
        alarm_rows(b.station.db(), "spo2.critical"),
        on_a,
        "nothing changed on b"
    );

    // a fails; b is promoted and the alarm continues from its stage (SR-007 restart path).
    b.station.promote().unwrap();
    assert!(b.station.is_acting());
    let stage_before = on_b[0].1;
    for k in 0..=120 {
        b.at(T0 + 41 * S + k * S);
        b.station.tick().unwrap();
    }
    let after = alarm_rows(b.station.db(), "spo2.critical");
    assert_eq!(after.len(), 1);
    assert!(
        after[0].1 > stage_before,
        "escalation continued: {} -> {}",
        stage_before,
        after[0].1
    );
    // And b accepts the acknowledgement now.
    b.station.alarm_acknowledge(alarm_id).unwrap();
}

#[test]
fn f_sys_04_a_replicated_row_for_an_unknown_table_or_column_is_refused() {
    let b = Node::new();
    let db = b.db();
    let mut row = serde_json::Map::new();
    row.insert("id".into(), serde_json::json!("x"));
    assert!(db.replica_apply("caelum-a", "sqlite_master", &row).is_err());
    row.insert("no_such_column".into(), serde_json::json!(1));
    assert!(db.replica_apply("caelum-a", "crew_member", &row).is_err());
}

#[test]
fn f_sys_04_capture_is_off_until_an_instance_is_primary() {
    let mut a = Node::new();
    a.station
        .create_first_administrator("admin", ADMIN)
        .unwrap();
    let db = a.db();
    assert!(
        db.cluster_outbox(10).unwrap().is_empty(),
        "a standalone station queues nothing"
    );
}
