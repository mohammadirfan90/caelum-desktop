//! Ground console profile (F-GND-08, F-GND-04 gateway side; CR-025): downlinked OPS records and
//! MEDICAL summaries are stored once (resent packets ignored) and served by role; OPS history
//! carries OPS fields only; a private medical conference record is drafted by its author,
//! signed once after re-authentication, then immutable.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::sync::atomic::Ordering;

use caelum_core::security::role::{DeniedReason, Role};
use caelum_core::station::{Station, StationError};
use caelum_domain::ids::CrewMemberId;
use caelum_domain::observation::Quality;
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_ground::config::ApidPlan;
use caelum_ground::medical::{MedicalSummary, SummaryValue};
use caelum_ground::ops::{OpsAlarmRecord, OpsAlarmState, OpsCategory, OpsPriority, OpsRecord};
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN: &str = "ground station passphrase";
const CREW_A: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a41";
const T0: i64 = 2_100_000_000_000_000_000;
const S: i64 = 1_000_000_000;

fn crew() -> CrewMemberId {
    CrewMemberId::from_uuid(uuid::Uuid::parse_str(CREW_A).unwrap())
}

fn plan() -> ApidPlan {
    toml::from_str(
        "ops_instance_status = 0x300\nops_crew_status = 0x301\nops_alarm = 0x302\nmedical_summary = 0x310\ncfdp = 0x320\n",
    )
    .unwrap()
}

fn console() -> (DataDir, Station) {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    clock.0.tai.store(T0, Ordering::SeqCst);
    let mut s = open(&dir, &keys, &clock).unwrap();
    s.create_first_administrator("admin", ADMIN).unwrap();
    s.login("admin", ADMIN.as_bytes()).unwrap();
    s.reauth(ADMIN.as_bytes()).unwrap();
    let d = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ground/");
    s.config_apply_console(
        &std::fs::read(format!("{d}console-v1.toml")).unwrap(),
        &std::fs::read_to_string(format!("{d}console-v1.sig.toml")).unwrap(),
    )
    .unwrap();
    s.user_create("fs.one", Role::FlightSurgeon, None, ADMIN)
        .unwrap();
    s.user_create("mcc.one", Role::Mcc, None, ADMIN).unwrap();
    (dir, s)
}

#[test]
fn f_gnd_08_downlinked_ops_and_summaries_are_stored_once_and_served_by_role() {
    let (_d, mut s) = console();
    let alarm = OpsRecord::Alarm(OpsAlarmRecord {
        crew_id: crew(),
        priority: OpsPriority::Critical,
        category: OpsCategory::Cardio,
        state: OpsAlarmState::Active,
        escalation_stage: 4,
        created_at: TaiNs(T0 - 60 * S),
        at: TaiNs(T0 - 30 * S),
    });
    let packet = alarm.to_packet(&plan(), 7).unwrap().into_bytes();
    let new = s.console_ingest(&packet).unwrap();
    assert_eq!(new.len(), 1);
    assert_eq!(new[0]["type"], "ops_alarm");
    assert_eq!(new[0]["priority"], "critical");
    assert_eq!(new[0]["category"], "CARDIO");
    assert!(
        s.console_ingest(&packet).unwrap().is_empty(),
        "a resent packet is stored once"
    );

    let summary = MedicalSummary {
        crew_id: crew(),
        parameter: Parameter::HeartRate,
        quality: Quality::Good,
        window_start: TaiNs(T0 - 120 * S),
        window_end: TaiNs(T0 - 60 * S),
        value: SummaryValue::Numeric {
            min: 58.0,
            mean: 64.5,
            max: 71.0,
            count: 60,
        },
    };
    s.console_ingest(&summary.to_packet(&plan(), 8).unwrap().into_bytes())
        .unwrap();
    // A telecommand or garbage is refused, never stored.
    assert!(s.console_ingest(b"not a packet").unwrap().is_empty());

    s.logout().unwrap();
    s.login("fs.one", ADMIN.as_bytes()).unwrap();
    let rows = s.console_summaries(crew(), 86_400).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].parameter.as_str(), rows[0].count),
        ("heart_rate", 60)
    );
    assert!((rows[0].mean_value.unwrap() - 64.5).abs() < 1e-6);
    assert_eq!(s.console_ops("ops_alarm", 86_400).unwrap().len(), 1);

    // MCC sees OPS records, never summaries or PMC records.
    s.logout().unwrap();
    s.login("mcc.one", ADMIN.as_bytes()).unwrap();
    assert_eq!(s.console_ops("ops_alarm", 86_400).unwrap().len(), 1);
    assert!(matches!(
        s.console_summaries(crew(), 86_400),
        Err(StationError::Denied(d)) if d.reason == DeniedReason::Role
    ));
    assert!(matches!(s.pmc_list(crew()), Err(StationError::Denied(_))));
    // History for Open MCT has OPS fields only.
    let h = s
        .console_history("ops_alarm", TaiNs(T0 - 3_600 * S), TaiNs(T0))
        .unwrap();
    let keys: Vec<&str> = h[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    for forbidden in ["value", "parameter", "rule_id", "ruleId", "why", "title"] {
        assert!(!keys.contains(&forbidden), "{keys:?}");
    }
}

#[test]
fn f_gnd_08_a_private_medical_conference_record_is_signed_once_then_immutable() {
    let (_d, mut s) = console();
    s.logout().unwrap();
    s.login("fs.one", ADMIN.as_bytes()).unwrap();
    let id = s
        .pmc_save(
            None,
            crew(),
            TaiNs(T0),
            "Discussed sleep and headache; plan: hydration.",
        )
        .unwrap();
    s.pmc_save(
        Some(&id),
        crew(),
        TaiNs(T0),
        "Discussed sleep and headache; plan: hydration, recheck in 2 days.",
    )
    .unwrap();
    s.reauth(ADMIN.as_bytes()).unwrap();
    s.pmc_sign(&id).unwrap();
    assert!(s.pmc_save(Some(&id), crew(), TaiNs(T0), "changed").is_err());
    assert!(s.pmc_sign(&id).is_err(), "signed once");
    let list = s.pmc_list(crew()).unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].signed_at.is_some());
    assert!(list[0].notes.ends_with("recheck in 2 days."));
    assert!(
        s.pmc_save(None, crew(), TaiNs(T0), "   ").is_err(),
        "an empty record is refused"
    );
}
