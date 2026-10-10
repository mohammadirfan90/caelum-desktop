//! M8 gate "retention removes only eligible data" (F-SYS-08; CR-022): with signed limits,
//! technical observations and raw waveforms older than their limit are removed, except within the
//! alarm guard of the same crew member; mission data is never removed; without a signed
//! configuration nothing is removed; every purge is audited.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::sync::atomic::Ordering;

use caelum_core::station::Station;
use caelum_core::store::{ObservationRecord, WaveformChunkRecord};
use caelum_domain::time::TaiNs;
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN: &str = "ground station passphrase";
const CREW_A: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a41";
const T0: i64 = 2_100_000_000_000_000_000;
const DAY: i64 = 86_400_000_000_000;
const MIN: i64 = 60_000_000_000;

fn station() -> (DataDir, TestClock, Station) {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    clock.0.tai.store(T0, Ordering::SeqCst);
    let mut s = open(&dir, &keys, &clock).unwrap();
    s.create_first_administrator("admin", ADMIN).unwrap();
    s.login("admin", ADMIN.as_bytes()).unwrap();
    s.reauth(ADMIN.as_bytes()).unwrap();
    let fx = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/roster/");
    s.config_apply_crew(
        &std::fs::read(format!("{fx}crew-v1.toml")).unwrap(),
        &std::fs::read_to_string(format!("{fx}crew-v1.sig.toml")).unwrap(),
    )
    .unwrap();
    (dir, clock, s)
}

fn obs(s: &Station, parameter: &str, unit: &str, at: i64) {
    s.db()
        .insert_observation(&ObservationRecord {
            id: uuid::Uuid::new_v4().to_string(),
            crew_member_id: CREW_A.into(),
            device_id: None,
            parameter: parameter.into(),
            value: Some(1.0),
            value_text: None,
            unit: unit.into(),
            measured_at_tai: at,
            received_at_tai: at,
            quality: "good".into(),
            classification: "measured".into(),
            measurement_mode: "continuous".into(),
            algorithm_version: None,
            context: None,
        })
        .unwrap();
}

fn chunk(s: &Station, at: i64) {
    s.db()
        .insert_waveform_chunk(
            CREW_A,
            &WaveformChunkRecord {
                device_id: None,
                signal: "ecg".into(),
                start_tai: at,
                sample_rate: 250.0,
                encoding: "i16le".into(),
                data: vec![0, 0],
                quality: "good".into(),
            },
        )
        .unwrap();
}

fn alarm(s: &Station, at: i64) {
    s.db()
        .connection()
        .execute(
            "INSERT INTO alarm (id, crew_member_id, kind, category, rule_id, ruleset_version, priority,
             state, latched, why, created_at_tai) VALUES (?1, ?2, 'physiological', 'CARDIO',
             'hr.critical.high', '1', 'critical', 'resolved', 1, '{}', ?3)",
            (uuid::Uuid::new_v4().to_string(), CREW_A, at),
        )
        .unwrap();
}

fn count(s: &Station, table: &str, filter: &str) -> i64 {
    s.db()
        .connection()
        .query_row(
            &format!("SELECT count(*) FROM {table} WHERE {filter}"),
            [],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn f_sys_08_retention_removes_only_eligible_data_and_is_audited() {
    let (_d, clock, mut s) = station();
    let old = T0 - 10 * DAY;
    obs(&s, "rssi", "dBm", old); // technical, old: removed
    obs(&s, "rssi", "dBm", old + 60 * MIN); // technical, old, but 10 min from an alarm: kept
    obs(&s, "rssi", "dBm", T0 - DAY); // technical, recent: kept
    obs(&s, "spo2", "%", old); // mission: never removed
    chunk(&s, T0 - 5 * DAY); // waveform, older than 3 days: removed
    chunk(&s, old + 50 * MIN); // waveform, old, 20 min from the alarm: kept
    chunk(&s, T0 - DAY); // waveform, recent: kept
    alarm(&s, old + 70 * MIN);

    // Without a signed configuration nothing goes.
    assert_eq!(s.retention_purge(TaiNs(T0)).unwrap().observations, 0);
    assert_eq!(count(&s, "observation", "1=1"), 4);

    let d = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/retention/");
    s.config_apply_retention(
        &std::fs::read(format!("{d}retention-v1.toml")).unwrap(),
        &std::fs::read_to_string(format!("{d}retention-v1.sig.toml")).unwrap(),
    )
    .unwrap();
    let purged = s.retention_purge(TaiNs(T0)).unwrap();
    assert_eq!((purged.observations, purged.waveforms), (1, 1));
    assert_eq!(count(&s, "observation", "parameter = 'rssi'"), 2);
    assert_eq!(
        count(&s, "observation", "parameter = 'spo2'"),
        1,
        "mission data stays"
    );
    assert_eq!(count(&s, "waveform_chunk", "1=1"), 2);
    assert_eq!(count(&s, "alarm", "1=1"), 1);
    assert_eq!(count(&s, "audit_entry", "action = 'retention.purge'"), 1);
    // Nothing more is eligible: a second run removes nothing and writes no entry.
    clock.0.tai.store(T0 + 1, Ordering::SeqCst);
    assert_eq!(
        s.retention_purge(TaiNs(T0 + 1)).unwrap(),
        Default::default()
    );
    assert_eq!(count(&s, "audit_entry", "action = 'retention.purge'"), 1);
}
