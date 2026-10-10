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
            assert_eq!(version, "1.0.0-draft.3");
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

fn observation(
    crew: CrewMemberId,
    device: caelum_domain::ids::DeviceId,
    parameter: caelum_domain::parameters::Parameter,
    value: f64,
    s: i64,
    mode: caelum_domain::observation::MeasurementMode,
) -> caelum_ingest::Observation {
    caelum_ingest::Observation {
        crew_member_id: crew,
        device_id: device,
        parameter,
        value: caelum_ingest::Value::Number(value),
        unit: parameter.spec().unit,
        measured_at: TaiNs(T.0 + s * 1_000_000_000),
        received_at: TaiNs(T.0 + s * 1_000_000_000),
        quality: caelum_domain::observation::Quality::Good,
        classification: caelum_domain::observation::Classification::Measured,
        mode,
        provenance: caelum_ingest::Provenance {
            time: caelum_ingest::TimeSource::Receipt,
            original: None,
            site: None,
        },
    }
}

fn state_of(t: &TempDb, rule: &str) -> Vec<String> {
    let mut stmt =
        t.db.connection()
            .prepare("SELECT state FROM alarm WHERE rule_id = ?1 ORDER BY created_at_tai")
            .unwrap();
    stmt.query_map([rule], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

/// Every alarm transition is persisted: confirming → active (recheck timeout on the tick),
/// cleared awaiting ack, not confirmed, superseded and resolved.
#[test]
fn every_alarm_transition_is_persisted() {
    use caelum_domain::observation::MeasurementMode::{Continuous, Spot};
    use caelum_domain::parameters::Parameter;
    let t = TempDb::new();
    let (crew, device) = crew_and_device(&t);
    let mut s = load(&t.db, BUNDLED_RULESET, TrustPolicy::Development);

    // Spot breach → confirming; no repeat within 120 s → active on the tick.
    s.observe(
        &t.db,
        &observation(crew, device, Parameter::Spo2, 80.0, 0, Spot),
        None,
    )
    .unwrap();
    assert_eq!(state_of(&t, "spo2.critical"), ["confirming"]);
    s.tick(&t.db, TaiNs(T.0 + 120 * 1_000_000_000)).unwrap();
    assert_eq!(state_of(&t, "spo2.critical"), ["active"]);
    // One clear spot reading → cleared awaiting acknowledgement.
    s.observe(
        &t.db,
        &observation(crew, device, Parameter::Spo2, 95.0, 130, Spot),
        None,
    )
    .unwrap();
    assert_eq!(state_of(&t, "spo2.critical"), ["cleared_awaiting_ack"]);

    // Thermometer: a breach then a normal repeat → not confirmed.
    s.observe(
        &t.db,
        &observation(crew, device, Parameter::BodyTempMeasured, 38.4, 200, Spot),
        None,
    )
    .unwrap();
    s.observe(
        &t.db,
        &observation(crew, device, Parameter::BodyTempMeasured, 36.8, 220, Spot),
        None,
    )
    .unwrap();
    assert_eq!(state_of(&t, "bodytemp.warning"), ["not_confirmed"]);

    // Heart rate 140 → warning; 190 → critical supersedes it (SR-006).
    for k in 0..=120 {
        s.observe(
            &t.db,
            &observation(
                crew,
                device,
                Parameter::HeartRate,
                140.0,
                300 + k,
                Continuous,
            ),
            None,
        )
        .unwrap();
    }
    assert_eq!(state_of(&t, "hr.warning.high"), ["active"]);
    for k in 121..=151 {
        s.observe(
            &t.db,
            &observation(
                crew,
                device,
                Parameter::HeartRate,
                190.0,
                300 + k,
                Continuous,
            ),
            None,
        )
        .unwrap();
    }
    assert_eq!(state_of(&t, "hr.warning.high"), ["superseded"]);
    assert_eq!(state_of(&t, "hr.critical.high"), ["active"]);

    // Observe-level AF notice: fires after 300 s and resolves itself after 600 s below 0.5.
    for k in 0..=300 {
        s.observe(
            &t.db,
            &observation(
                crew,
                device,
                Parameter::AfProbability,
                0.9,
                1_000 + k,
                Continuous,
            ),
            None,
        )
        .unwrap();
    }
    assert_eq!(state_of(&t, "af.observe"), ["active"]);
    for k in 301..=901 {
        s.observe(
            &t.db,
            &observation(
                crew,
                device,
                Parameter::AfProbability,
                0.2,
                1_000 + k,
                Continuous,
            ),
            None,
        )
        .unwrap();
    }
    assert_eq!(state_of(&t, "af.observe"), ["resolved"]);

    // Absence of QRS with good ECG SQI, via events and the gate.
    s.observe(
        &t.db,
        &observation(crew, device, Parameter::SqiEcg, 0.9, 3_000, Continuous),
        None,
    )
    .unwrap();
    s.event(&t.db, crew, "qrs", TaiNs(T.0 + 3_000 * 1_000_000_000))
        .unwrap();
    s.observe(
        &t.db,
        &observation(crew, device, Parameter::SqiEcg, 0.9, 3_007, Continuous),
        None,
    )
    .unwrap();
    assert_eq!(state_of(&t, "asystole.critical"), ["active"]);
}

/// Enumerated values do not reach the engine, and in safe state nothing is evaluated.
#[test]
fn states_are_not_evaluated_and_safe_state_evaluates_nothing() {
    use caelum_domain::observation::MeasurementMode::Continuous;
    use caelum_domain::parameters::Parameter;
    let t = TempDb::new();
    let (crew, device) = crew_and_device(&t);
    let mut s = load(&t.db, BUNDLED_RULESET, TrustPolicy::Development);
    let mut obs = observation(crew, device, Parameter::SleepStage, 0.0, 0, Continuous);
    obs.value = caelum_ingest::Value::State("rem");
    assert!(s.observe(&t.db, &obs, None).unwrap().is_empty());
    let mut safe = load(&t.db, BUNDLED_RULESET, TrustPolicy::Flight);
    let low = observation(crew, device, Parameter::Spo2, 70.0, 0, Continuous);
    assert!(safe.observe(&t.db, &low, None).unwrap().is_empty());
    assert!(safe.event(&t.db, crew, "qrs", T).unwrap().is_empty());
}

fn stages(t: &TempDb) -> Vec<(String, i64)> {
    let mut stmt =
        t.db.connection()
            .prepare("SELECT audience, stage FROM alarm_notification ORDER BY stage")
            .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

/// M4 gate, SR-007: stop CAELUM mid-escalation and start it again on the same records: the alarm
/// is not raised twice, sound resumes, and escalation continues from the stored stage.
#[test]
fn sr_007_restart_mid_escalation_resumes_from_the_records() {
    use caelum_alarms::escalation::{Notice, SoundScope};
    use caelum_domain::observation::MeasurementMode::Continuous;
    use caelum_domain::parameters::Parameter;
    let t = TempDb::new();
    let (crew, device) = crew_and_device(&t);
    {
        let mut s = load(&t.db, BUNDLED_RULESET, TrustPolicy::Development);
        for k in 0..=15 {
            s.observe(
                &t.db,
                &observation(crew, device, Parameter::Spo2, 80.0, k, Continuous),
                None,
            )
            .unwrap();
        }
        // Created at 15 s (persistence); sound stage due 15 s later.
        s.tick(&t.db, TaiNs(T.0 + 31 * 1_000_000_000)).unwrap();
        assert_eq!(
            stages(&t)
                .iter()
                .map(|(a, _)| a.as_str())
                .collect::<Vec<_>>(),
            ["astronaut", "cmo", "sound"]
        );
    } // CAELUM stops.
    let mut s = load(&t.db, BUNDLED_RULESET, TrustPolicy::Development);
    let resumed = s.take_restored();
    assert!(
        matches!(
            &resumed[..],
            [Notice::SoundOn {
                scope: SoundScope::AllCrewStations,
                wristband: true,
                ..
            }]
        ),
        "{resumed:?}"
    );
    // Still desaturated: no second alarm.
    for k in 32..=40 {
        let out = s
            .observe(
                &t.db,
                &observation(crew, device, Parameter::Spo2, 80.0, k, Continuous),
                None,
            )
            .unwrap();
        assert!(out.events.is_empty(), "{:?}", out.events);
    }
    assert_eq!(state_of(&t, "spo2.critical"), ["active"]);
    let crew_stage = s.tick(&t.db, TaiNs(T.0 + 45 * 1_000_000_000)).unwrap();
    assert!(
        crew_stage
            .notices
            .iter()
            .any(|n| matches!(n, Notice::Stage { stage: 3, .. })),
        "{crew_stage:?}"
    );
    let ground = s.tick(&t.db, TaiNs(T.0 + 75 * 1_000_000_000)).unwrap();
    assert!(ground.notices.iter().any(|n| matches!(n, Notice::Ops(_))));
    assert_eq!(
        stages(&t).len(),
        5,
        "each stage delivered once: {:?}",
        stages(&t)
    );
}

/// F-ALM-05, SR-008, R-ALM-05: acknowledgement and silence are recorded (silence clamped to
/// its limit) and survive a restart.
#[test]
fn acknowledgement_and_silence_are_recorded_and_survive_a_restart() {
    use caelum_alarms::escalation::Notice;
    use caelum_domain::observation::MeasurementMode::Continuous;
    use caelum_domain::parameters::Parameter;
    let t = TempDb::new();
    let (crew, device) = crew_and_device(&t);
    let user = uuid::Uuid::new_v4().to_string();
    t.db.connection()
        .execute(
            "INSERT INTO user (id, username, role, crew_member_id, credential) VALUES (?1, 'cmo', 'cmo', ?2, 'x')",
            (&user, crew.to_string()),
        )
        .unwrap();
    let mut s = load(&t.db, BUNDLED_RULESET, TrustPolicy::Development);
    for k in 0..=15 {
        s.observe(
            &t.db,
            &observation(crew, device, Parameter::Spo2, 80.0, k, Continuous),
            None,
        )
        .unwrap();
    }
    let id: caelum_domain::ids::AlarmId =
        t.db.connection()
            .query_row(
                "SELECT id FROM alarm WHERE rule_id = 'spo2.critical'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap()
            .parse()
            .unwrap();
    assert_eq!(s.crew_of(id), Some(crew));
    let (_, applied) = s
        .silence(&t.db, id, 999, TaiNs(T.0 + 16 * 1_000_000_000))
        .unwrap();
    assert_eq!(applied, 120);
    let out = s
        .acknowledge(&t.db, id, &user, TaiNs(T.0 + 20 * 1_000_000_000))
        .unwrap();
    assert!(matches!(out.notices.first(), Some(Notice::SoundOff { .. })));
    s.acknowledge_ground(&t.db, id, &user, TaiNs(T.0 + 21 * 1_000_000_000))
        .unwrap();
    let (by, ground): (Option<String>, Option<String>) =
        t.db.connection()
            .query_row(
                "SELECT acknowledged_by, ground_acknowledged_by FROM alarm WHERE id = ?1",
                [id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
    assert_eq!(
        (by.as_deref(), ground.as_deref()),
        (Some(user.as_str()), Some(user.as_str()))
    );
    drop(s);
    let mut again = load(&t.db, BUNDLED_RULESET, TrustPolicy::Development);
    assert!(
        again.take_restored().is_empty(),
        "acknowledged: no sound after the restart"
    );
    assert!(
        again
            .tick(&t.db, TaiNs(T.0 + 600 * 1_000_000_000))
            .unwrap()
            .notices
            .is_empty(),
        "no escalation after acknowledgement"
    );
    let esc = again.escalate(&t.db, id, TaiNs(T.0 + 601 * 1_000_000_000));
    assert!(esc.is_ok());
    let ghost = caelum_domain::ids::AlarmId::from_uuid(uuid::Uuid::new_v4());
    assert!(again.acknowledge_ground(&t.db, ghost, &user, T).is_err());
}
