//! M3 gate (SR-009): a tampered ruleset is rejected and a technical alarm raised; the last
//! verified ruleset is used, or with none the station runs no physiological rules; a release build
//! rejects a development-signed ruleset. The engine runs in the ingest path (docs/03 §3).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use std::path::PathBuf;

use caelum_alarms::signing::{built_in_keys, TrustPolicy};
use caelum_alarms::{BUNDLED_RULESET, BUNDLED_SIGNATURES};
use caelum_core::acquisition::Acquisition;
use caelum_core::alarms::{AlarmService, RulesetStatus};
use caelum_devices::registry::{DeviceIdentity, Registry};
use caelum_domain::ids::CrewMemberId;
use caelum_domain::time::TaiNs;
use caelum_store::{Database, DbKey};
use caelum_test_equipment::sig::{self as emu, uuid as gatt};

struct TempDb {
    db: Database,
    _guard: Guard,
}

struct Guard(PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl TempDb {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("caelum-alarms-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("caelum.db"), &DbKey::from_bytes([4; 32])).unwrap();
        Self {
            db,
            _guard: Guard(dir),
        }
    }

    fn alarms(&self) -> Vec<(String, String, String)> {
        let mut stmt = self
            .db
            .connection()
            .prepare("SELECT rule_id, state, kind FROM alarm ORDER BY created_at_tai, rule_id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }
}

const T: TaiNs = TaiNs(2_100_000_000_000_000_000);

fn load(db: &Database, ruleset: &str, policy: TrustPolicy) -> AlarmService {
    AlarmService::load(db, ruleset, BUNDLED_SIGNATURES, &built_in_keys(), policy, T).unwrap()
}

#[test]
fn the_bundled_ruleset_loads_in_development_builds_with_the_banner() {
    let t = TempDb::new();
    let s = load(&t.db, BUNDLED_RULESET, TrustPolicy::Development);
    assert!(
        matches!(
            s.status(),
            RulesetStatus::Verified {
                development: true,
                ..
            }
        ),
        "{:?}",
        s.status()
    );
    assert!(t.alarms().is_empty());
    assert!(t
        .db
        .latest_config_bundle("alarm-ruleset")
        .unwrap()
        .is_some());
}

/// M3 gate, SR-009: a tampered ruleset is rejected, a technical alarm raised, and the last verified
/// ruleset stays in force.
#[test]
fn sr_009_tampered_ruleset_raises_a_technical_alarm_and_falls_back() {
    let t = TempDb::new();
    load(&t.db, BUNDLED_RULESET, TrustPolicy::Development);
    let tampered = BUNDLED_RULESET.replacen("fire = 85.0", "fire = 60.0", 1);
    let s = load(&t.db, &tampered, TrustPolicy::Development);
    match s.status() {
        RulesetStatus::Fallback {
            version, reason, ..
        } => {
            assert_eq!(version, "1.0.0-draft.2");
            assert!(reason.contains("digest"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
    let alarms = t.alarms();
    assert_eq!(alarms.len(), 1);
    assert_eq!(
        (alarms[0].0.as_str(), alarms[0].2.as_str()),
        ("technical.ruleset_invalid", "technical")
    );
    let why: String =
        t.db.connection()
            .query_row("SELECT why FROM alarm", [], |r| r.get(0))
            .unwrap();
    assert!(why.contains("using the last verified ruleset"), "{why}");
}

/// SR-009: with no verified ruleset at all, no physiological rules run (safe state) and the
/// technical alarm says so.
#[test]
fn sr_009_no_verified_ruleset_means_safe_state() {
    let t = TempDb::new();
    let tampered = BUNDLED_RULESET.replacen("fire = 85.0", "fire = 60.0", 1);
    let mut s = load(&t.db, &tampered, TrustPolicy::Development);
    assert!(matches!(s.status(), RulesetStatus::SafeState { .. }));
    assert!(s.tick(&t.db, T).unwrap().is_empty());
    let why: String =
        t.db.connection()
            .query_row(
                "SELECT why FROM alarm WHERE rule_id = 'technical.ruleset_invalid'",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert!(why.contains("not running"), "{why}");
}

/// M3 gate, SR-009: a release build rejects the development-signed ruleset (no flight key yet).
#[test]
fn sr_009_release_policy_rejects_the_development_signed_ruleset() {
    let t = TempDb::new();
    let s = load(&t.db, BUNDLED_RULESET, TrustPolicy::Flight);
    assert!(
        matches!(s.status(), RulesetStatus::SafeState { .. }),
        "{:?}",
        s.status()
    );
    assert_eq!(t.alarms()[0].0, "technical.ruleset_invalid");
}

fn crew_and_device(t: &TempDb) -> (CrewMemberId, caelum_domain::ids::DeviceId) {
    let crew = CrewMemberId::from_uuid(uuid::Uuid::new_v4());
    t.db.connection()
        .execute(
            "INSERT INTO crew_member (id, name, role, mission, active) VALUES (?1, 'A', 'crew', 'M-1', 1)",
            [crew.to_string()],
        )
        .unwrap();
    let reg = Registry::new(&t.db);
    let device = reg
        .register(&DeviceIdentity {
            adapter: "ble-sig".into(),
            vendor: "V".into(),
            model: "Oximeter".into(),
            serial: "OX-1".into(),
            firmware: None,
            accuracy_class: None,
            calibration_due: None,
        })
        .unwrap();
    reg.assign(device, crew, None).unwrap();
    (crew, device)
}

/// docs/03 §3: device notification → ingest → database → engine → alarm stored, synchronously.
#[test]
fn a_desaturation_streams_from_the_oximeter_to_a_stored_alarm() {
    let t = TempDb::new();
    let (_, device) = crew_and_device(&t);
    let mut acq = Acquisition::new().unwrap();
    let mut alarms = load(&t.db, BUNDLED_RULESET, TrustPolicy::Development);
    for s in 0..=15 {
        let payload = emu::plx_continuous(emu::sfloat(82, 0), emu::sfloat(110, 0), 0);
        let out = acq
            .notification(
                &t.db,
                device,
                gatt::PLX_CONTINUOUS,
                &payload,
                TaiNs(T.0 + s * 1_000_000_000),
            )
            .unwrap();
        for obs in &out.observations {
            alarms.observe(&t.db, obs, None).unwrap();
        }
    }
    let stored = t.alarms();
    assert!(
        stored.contains(&(
            "spo2.critical".into(),
            "active".into(),
            "physiological".into()
        )),
        "{stored:?}"
    );
    let why: String =
        t.db.connection()
            .query_row(
                "SELECT why FROM alarm WHERE rule_id = 'spo2.critical'",
                [],
                |r| r.get(0),
            )
            .unwrap();
    assert!(why.contains("\"held_s\":15.0"), "{why}");
}

/// SR-005: an unusable signal raises a technical alarm, resolved when the signal recovers.
#[test]
fn sr_005_unusable_signal_raises_and_resolves_a_technical_alarm() {
    let t = TempDb::new();
    let (_, device) = crew_and_device(&t);
    let mut acq = Acquisition::new().unwrap();
    let mut alarms = load(&t.db, BUNDLED_RULESET, TrustPolicy::Development);
    let mut feed = |status: u16, s: i64| {
        let payload = emu::plx_continuous(emu::sfloat(97, 0), emu::sfloat(60, 0), status);
        let out = acq
            .notification(
                &t.db,
                device,
                gatt::PLX_CONTINUOUS,
                &payload,
                TaiNs(T.0 + s * 1_000_000_000),
            )
            .unwrap();
        for obs in &out.observations {
            alarms.observe(&t.db, obs, None).unwrap();
        }
    };
    feed(1 << 15, 0); // invalid measurement: unusable
    let stored = t.alarms();
    assert_eq!(stored.len(), 2, "SpO2 and pulse rate signals: {stored:?}");
    assert!(stored
        .iter()
        .all(|(r, s, k)| r.starts_with("technical.signal_unusable.")
            && s == "active"
            && k == "technical"));
    feed(0, 1);
    assert!(
        t.alarms().iter().all(|(_, s, _)| s == "resolved"),
        "{:?}",
        t.alarms()
    );
}
