//! Technical alarms (F-ALM-06; SR-004, SR-012, R-OPS-04; CR-011): each cause raises once and
//! clears when it goes away.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use caelum_alarms::technical::{Change, Limits, Monitor};
use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;

const S: i64 = 1_000_000_000;

fn crew() -> CrewMemberId {
    CrewMemberId::from_uuid(uuid::Uuid::from_u128(1))
}

fn device() -> DeviceId {
    DeviceId::from_uuid(uuid::Uuid::from_u128(2))
}

fn raised(c: &[Change]) -> Vec<&str> {
    c.iter()
        .filter(|x| x.raise.is_some())
        .map(|x| x.rule_id.as_str())
        .collect()
}

fn cleared(c: &[Change]) -> Vec<&str> {
    c.iter()
        .filter(|x| x.raise.is_none())
        .map(|x| x.rule_id.as_str())
        .collect()
}

/// SR-004: a value older than stale_after_s raises stale data once; a new value clears it; a
/// parameter never reported is not stale (it is NO DATA).
#[test]
fn sr_004_stale_data_raises_once_and_clears_on_new_data() {
    let mut m = Monitor::new(Limits::DEFAULT);
    assert!(
        m.tick(TaiNs(1_000 * S)).is_empty(),
        "nothing reported, nothing stale"
    );
    m.observation(
        crew(),
        Some(device()),
        Parameter::Spo2,
        Some(97.0),
        TaiNs(0),
    );
    let limit = i64::from(Parameter::Spo2.spec().stale_after_s);
    assert!(
        m.tick(TaiNs(limit * S)).is_empty(),
        "exactly at the limit is not stale"
    );
    let c = m.tick(TaiNs((limit + 1) * S));
    assert_eq!(raised(&c), ["technical.stale.spo2"]);
    assert!(m.tick(TaiNs((limit + 50) * S)).is_empty(), "raised once");
    let c = m.observation(
        crew(),
        Some(device()),
        Parameter::Spo2,
        Some(96.0),
        TaiNs((limit + 60) * S),
    );
    assert_eq!(cleared(&c), ["technical.stale.spo2"]);
}

/// F-ALM-06: battery below 20 % raises low battery; recharged clears it.
#[test]
fn f_alm_06_low_battery_raises_and_clears() {
    let mut m = Monitor::new(Limits::DEFAULT);
    assert!(m
        .observation(
            crew(),
            Some(device()),
            Parameter::BatteryLevel,
            Some(55.0),
            TaiNs(0)
        )
        .is_empty());
    let c = m.observation(
        crew(),
        Some(device()),
        Parameter::BatteryLevel,
        Some(19.0),
        TaiNs(S),
    );
    assert_eq!(
        raised(&c),
        [format!("technical.low_battery.{}", device()).as_str()]
    );
    assert!(m
        .observation(
            crew(),
            Some(device()),
            Parameter::BatteryLevel,
            Some(15.0),
            TaiNs(2 * S)
        )
        .is_empty());
    let c = m.observation(
        crew(),
        Some(device()),
        Parameter::BatteryLevel,
        Some(80.0),
        TaiNs(3 * S),
    );
    assert_eq!(cleared(&c).len(), 1);
}

/// F-ALM-06: a device disconnected for 30 s is lost; reconnecting clears it; a short drop does not
/// raise anything.
#[test]
fn f_alm_06_device_lost_after_thirty_seconds() {
    let mut m = Monitor::new(Limits::DEFAULT);
    m.link(device(), Some(crew()), false, TaiNs(0));
    assert!(m.tick(TaiNs(29 * S)).is_empty());
    let c = m.tick(TaiNs(30 * S));
    assert_eq!(
        raised(&c),
        [format!("technical.device_lost.{}", device()).as_str()]
    );
    assert!(m.tick(TaiNs(60 * S)).is_empty(), "raised once");
    let c = m.link(device(), Some(crew()), true, TaiNs(61 * S));
    assert_eq!(cleared(&c).len(), 1);
    m.link(device(), Some(crew()), false, TaiNs(100 * S));
    assert!(
        m.link(device(), Some(crew()), true, TaiNs(103 * S))
            .is_empty(),
        "short drop: nothing to clear"
    );
    assert!(m.tick(TaiNs(200 * S)).is_empty());
}

/// R-OPS-04, SR-012: residual over 50 ms, or time going backwards, raises clock drift.
#[test]
fn sr_012_clock_faults_raise_a_technical_alarm() {
    let mut m = Monitor::new(Limits::DEFAULT);
    assert!(m.clock(Some(device()), Some(40_000_000), false).is_empty());
    let c = m.clock(Some(device()), Some(60_000_000), false);
    assert_eq!(
        raised(&c),
        [format!("technical.clock.{}", device()).as_str()]
    );
    assert_eq!(
        cleared(&m.clock(Some(device()), Some(10_000_000), false)).len(),
        1
    );
    let c = m.clock(None, None, true);
    assert_eq!(raised(&c), ["technical.clock.station"]);
    assert!(c[0].raise.as_ref().unwrap()["time_went_backwards"]
        .as_bool()
        .unwrap());
}

/// F-ALM-06: free storage below 1 GiB raises storage low.
#[test]
fn f_alm_06_storage_low() {
    let mut m = Monitor::new(Limits::DEFAULT);
    assert!(m.storage(10 << 30).is_empty());
    assert_eq!(raised(&m.storage(512 << 20)), ["technical.storage_low"]);
    assert!(m.storage(100 << 20).is_empty());
    assert_eq!(cleared(&m.storage(2 << 30)), ["technical.storage_low"]);
}
