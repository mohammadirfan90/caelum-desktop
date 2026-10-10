//! M9 gate through the station (F-ENV-01, F-GND-11 subscribe side, R-INT-04; CR-015): cabin
//! readings from the vehicle bridge are stored for every active crew member and evaluated, so the
//! environment alarms fire from vehicle data. Readings are built here from known values (test
//! fixtures, never product data).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::sync::atomic::Ordering;

use caelum_core::station::Station;
use caelum_core::vehicle::{Subject, TimeSource, VehicleReading, VehicleValue};
use caelum_domain::observation::{MeasurementMode, Quality};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::{MetNs, TaiNs};
use caelum_domain::units::Unit;
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN: &str = "ground station passphrase";
const T0: i64 = 2_100_000_000_000_000_000;
const S: i64 = 1_000_000_000;

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

fn cabin(parameter: Parameter, value: f64, unit: Unit, at: i64) -> VehicleReading {
    VehicleReading {
        mid: 0x0901,
        message: "CABIN_ATMOSPHERE".into(),
        parameter,
        subject: Subject::Cabin { location: None },
        value: VehicleValue::Number(value),
        unit,
        original: None,
        measured_at: TaiNs(at),
        met: MetNs(at - T0),
        received_at: TaiNs(at),
        time: TimeSource::Vehicle {
            offset_ns: 0,
            exact: true,
        },
        quality: Quality::Good,
        mode: MeasurementMode::Continuous,
    }
}

fn raised(s: &Station, rule: &str) -> usize {
    s.db()
        .connection()
        .query_row(
            "SELECT count(*) FROM alarm WHERE rule_id = ?1",
            [rule],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| usize::try_from(n).unwrap())
        .unwrap()
}

/// M9 gate, F-ENV-01: low cabin O2 from the vehicle raises `o2.critical` for every active crew
/// member once it has held for the rule's persistence; normal O2 raises nothing.
#[test]
fn f_env_01_low_cabin_o2_from_the_vehicle_raises_the_environment_alarm_for_the_crew() {
    let (_d, clock, mut s) = station();
    for k in 0..=6 {
        let at = T0 + k * 10 * S;
        clock.0.tai.store(at, Ordering::SeqCst);
        let r = s
            .ingest_vehicle(&[cabin(Parameter::CabinO2, 21.0, Unit::KiloPascal, at)])
            .unwrap();
        assert_eq!(r.stored, 2, "one observation per active crew member");
        s.tick().unwrap();
    }
    assert_eq!(raised(&s, "o2.critical"), 0);
    let start = T0 + 100 * S;
    for k in 0..=4 {
        let at = start + k * 10 * S;
        clock.0.tai.store(at, Ordering::SeqCst);
        s.ingest_vehicle(&[cabin(Parameter::CabinO2, 15.0, Unit::KiloPascal, at)])
            .unwrap();
        s.tick().unwrap();
    }
    assert_eq!(raised(&s, "o2.critical"), 2, "one alarm per crew member");
}

/// M9 gate, F-ENV-01: CO2 above the warning limit for its 15-minute persistence raises
/// `co2.warning`; a shorter excursion does not.
#[test]
fn f_env_01_elevated_cabin_co2_from_the_vehicle_raises_the_warning_after_its_persistence() {
    let (_d, clock, mut s) = station();
    for k in 0..=95 {
        let at = T0 + k * 10 * S;
        clock.0.tai.store(at, Ordering::SeqCst);
        s.ingest_vehicle(&[cabin(Parameter::CabinCo2, 5.0, Unit::MmHg, at)])
            .unwrap();
        s.tick().unwrap();
        if k == 80 {
            assert_eq!(raised(&s, "co2.warning"), 0, "not before 15 min");
        }
    }
    assert_eq!(raised(&s, "co2.warning"), 2);
}
