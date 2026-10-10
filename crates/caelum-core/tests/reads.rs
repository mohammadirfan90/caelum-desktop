//! Crew-screen reads and device assignment through the station (CR-012 §4; F-ACQ-09, F-ACQ-10,
//! F-ALM-05, F-ALM-09, SR-004; ROLE_MATRIX §2): real decode → ingest → store → engine path,
//! scoped by role and crew, MCC limited to the OPS fields (docs/06 §3a).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::sync::atomic::Ordering;

use caelum_core::acquisition::Acquisition;
use caelum_core::security::role::{DeniedReason, Role};
use caelum_core::security::session::AuthError;
use caelum_core::station::reads::ValueStatus;
use caelum_core::station::{Station, StationError};
use caelum_devices::registry::{DeviceIdentity, Registry};
use caelum_domain::ids::{AlarmId, CrewMemberId, DeviceId};
use caelum_domain::time::TaiNs;
use caelum_test_equipment::sig::{self as emu, uuid as gatt};
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN: &str = "ground station passphrase";
const CREW_A: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a41";
const CREW_B: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a42";
const T0: i64 = 2_100_000_000_000_000_000;

fn crew(id: &str) -> CrewMemberId {
    CrewMemberId::from_uuid(uuid::Uuid::parse_str(id).unwrap())
}

struct World {
    _dir: DataDir,
    clock: TestClock,
    station: Station,
    device: DeviceId,
    acq: Acquisition,
}

impl World {
    fn now(&self) -> TaiNs {
        TaiNs(self.clock.0.tai.load(Ordering::SeqCst))
    }

    fn sign_in(&mut self, name: &str, secret: &str) {
        let _ = self.station.logout();
        self.station.login(name, secret.as_bytes()).unwrap();
    }

    /// One continuous oximeter reading, observed by the engine, then a station tick.
    fn spo2(&mut self, pct: i16) {
        let payload = emu::plx_continuous(emu::sfloat(pct, 0), emu::sfloat(80, 0), 0);
        let now = self.now();
        let out = self
            .acq
            .notification(
                self.station.db(),
                self.device,
                gatt::PLX_CONTINUOUS,
                &payload,
                now,
            )
            .unwrap();
        for obs in &out.observations {
            self.station.observe(obs).unwrap();
        }
        self.station.tick().unwrap();
    }
}

/// Roster A and B, accounts for every role, an oximeter assigned to A by the CMO.
fn world() -> World {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    clock.0.tai.store(T0, Ordering::SeqCst);
    let mut station = open(&dir, &keys, &clock).unwrap();
    station.create_first_administrator("admin", ADMIN).unwrap();
    station.login("admin", ADMIN.as_bytes()).unwrap();
    station.reauth(ADMIN.as_bytes()).unwrap();
    let dir_fx = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/roster/");
    station
        .config_apply_crew(
            &std::fs::read(format!("{dir_fx}crew-v1.toml")).unwrap(),
            &std::fs::read_to_string(format!("{dir_fx}crew-v1.sig.toml")).unwrap(),
        )
        .unwrap();
    station
        .user_create("astro.a", Role::Astronaut, Some(crew(CREW_A)), "246813")
        .unwrap();
    station
        .user_create("cmo.b", Role::Cmo, Some(crew(CREW_B)), "135792")
        .unwrap();
    station
        .user_create("mcc.one", Role::Mcc, None, ADMIN)
        .unwrap();
    station
        .user_create("bme.one", Role::Bme, None, ADMIN)
        .unwrap();
    let device = Registry::new(station.db())
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
    let mut w = World {
        _dir: dir,
        clock,
        station,
        device,
        acq: Acquisition::new().unwrap(),
    };
    w.sign_in("cmo.b", "135792");
    w.station.reauth(b"135792").unwrap();
    w.station.device_assign(device, crew(CREW_A), None).unwrap();
    w
}

fn denied(r: Result<impl std::fmt::Debug, StationError>) -> Option<DeniedReason> {
    match r {
        Err(StationError::Denied(d)) => Some(d.reason),
        _ => None,
    }
}

/// SR-004, R-CLN-01: each current value comes with its source device, time and status.
#[test]
fn sr_004_vitals_show_source_and_freshness_and_stay_in_scope() {
    let mut w = world();
    w.spo2(97);
    w.sign_in("astro.a", "246813");
    let vitals = w.station.vitals_latest(crew(CREW_A)).unwrap();
    let spo2 = vitals
        .iter()
        .find(|v| v.observation.parameter == "spo2")
        .unwrap();
    assert_eq!(spo2.observation.value, Some(97.0));
    assert_eq!(spo2.status, ValueStatus::Live);
    assert_eq!(spo2.device_model.as_deref(), Some("Oximeter X"));
    assert!(
        !vitals
            .iter()
            .any(|v| v.observation.parameter == "heart_rate"),
        "never reported: absent"
    );
    // Own data only for an astronaut; nothing for MCC (ROLE_MATRIX §2).
    assert_eq!(
        denied(w.station.vitals_latest(crew(CREW_B))),
        Some(DeniedReason::Role)
    );
    w.sign_in("mcc.one", ADMIN);
    assert_eq!(
        denied(w.station.vitals_latest(crew(CREW_A))),
        Some(DeniedReason::Role)
    );

    // Older than the catalog's stale_after_s: STALE, never shown as live.
    w.clock.advance_s(3600);
    w.sign_in("bme.one", ADMIN);
    let spo2 = w
        .station
        .vitals_latest(crew(CREW_A))
        .unwrap()
        .into_iter()
        .find(|v| v.observation.parameter == "spo2")
        .unwrap();
    assert_eq!(spo2.status, ValueStatus::Stale);
    assert!(spo2.age_s >= 3600);
}

#[test]
fn cr_012_trends_cover_the_window_and_say_when_there_is_no_baseline() {
    let mut w = world();
    for pct in [96, 97, 98] {
        w.spo2(pct);
        w.clock.advance_s(10);
    }
    w.sign_in("astro.a", "246813");
    let t = w.station.vitals_trend(crew(CREW_A), "spo2", 3600).unwrap();
    assert_eq!(
        t.points
            .iter()
            .map(|p| p.value.unwrap())
            .collect::<Vec<_>>(),
        [96.0, 97.0, 98.0]
    );
    assert!(
        t.baseline.is_none(),
        "no baseline is estimated before M7 (CR-012 §6)"
    );
    let short = w.station.vitals_trend(crew(CREW_A), "spo2", 15).unwrap();
    assert_eq!(
        short.points.len(),
        1,
        "only the reading 10 s ago is within 15 s"
    );
    assert!(matches!(
        w.station.vitals_trend(crew(CREW_A), "nonsense", 60),
        Err(StationError::Rejected(_))
    ));
    assert!(w
        .station
        .waveform_recent(crew(CREW_A), "ecg", 30)
        .unwrap()
        .is_empty());
}

#[test]
fn f_alm_05_alarm_list_detail_and_timeline_with_mcc_ops_fields_only() {
    let mut w = world();
    for _ in 0..=60 {
        w.spo2(82);
        w.clock.advance_s(1);
    }
    w.sign_in("astro.a", "246813");
    let list = w.station.alarm_list().unwrap();
    let a = list
        .iter()
        .find(|a| a.alarm.rule_id == "spo2.critical")
        .unwrap();
    assert_eq!(a.alarm.title.as_deref().map(str::is_empty), Some(false));
    assert!(!a.ops_only);
    let id: AlarmId = a.alarm.id.parse().unwrap();
    let detail = w.station.alarm_detail(id).unwrap();
    assert!(detail.alarm.why.contains("held_s"));
    let audiences: Vec<_> = detail
        .notifications
        .iter()
        .map(|n| n.audience.as_str())
        .collect();
    assert!(
        audiences.starts_with(&["astronaut", "cmo", "sound", "crew"]),
        "{audiences:?}"
    );

    // MCC: status only, no physiology (docs/06 §3a, CLAUDE.md rule 6).
    w.sign_in("mcc.one", ADMIN);
    let ops = w.station.alarm_list().unwrap();
    let o = ops.iter().find(|x| x.alarm.id == a.alarm.id).unwrap();
    assert!(o.ops_only);
    assert!(o.alarm.title.is_none() && o.alarm.rule_id.is_empty() && o.alarm.why.is_empty());
    assert!(o.alarm.trigger_value.is_none() && o.alarm.action.is_none());
    assert_eq!(o.alarm.priority, "critical");
    assert_eq!(denied(w.station.alarm_detail(id)), Some(DeniedReason::Role));

    // The CMO acknowledges; the report counts it (F-ALM-09).
    w.sign_in("cmo.b", "135792");
    w.station.alarm_acknowledge(id).unwrap();
    let report = w.station.alarm_report(24 * 3600).unwrap();
    let r = report
        .rules
        .iter()
        .find(|r| r.rule_id == "spo2.critical")
        .unwrap();
    assert_eq!(r.count, 1);
    assert!(r.median_ack_s.is_some());
    assert!(report
        .per_crew_per_day
        .iter()
        .any(|(c, n)| c == CREW_A && *n > 0.0));
    w.sign_in("astro.a", "246813");
    assert_eq!(
        denied(w.station.alarm_report(3600)),
        Some(DeniedReason::Role)
    );
    // Another crew member's alarm is out of an astronaut's scope.
    w.sign_in("cmo.b", "135792");
}

#[test]
fn f_acq_09_devices_are_listed_in_scope_and_assignment_needs_reauthentication() {
    let mut w = world();
    w.sign_in("astro.a", "246813");
    assert_eq!(w.station.device_list().unwrap().len(), 1, "own device");
    w.sign_in("bme.one", ADMIN);
    // Signing in counts as authentication; 2 minutes later a fresh confirmation is needed.
    w.clock.advance_s(121);
    let all = w.station.device_list().unwrap();
    assert_eq!(all[0].device.crew_member_id.as_deref(), Some(CREW_A));
    assert!(all[0].battery.is_none(), "no battery reported: none shown");
    assert!(matches!(
        w.station.device_unassign(w.device),
        Err(StationError::Auth(AuthError::ReauthRequired))
    ));
    w.station.reauth(ADMIN.as_bytes()).unwrap();
    w.station.device_unassign(w.device).unwrap();
    assert!(matches!(
        w.station.device_assign(
            w.device,
            CrewMemberId::from_uuid(uuid::Uuid::new_v4()),
            None
        ),
        Err(StationError::Rejected(_))
    ));
    w.station
        .device_assign(w.device, crew(CREW_B), None)
        .unwrap();
    w.sign_in("astro.a", "246813");
    assert!(
        w.station.device_list().unwrap().is_empty(),
        "no longer astro.a's device"
    );
    assert_eq!(
        denied(w.station.device_assign(w.device, crew(CREW_A), None)),
        Some(DeniedReason::Role)
    );
}
