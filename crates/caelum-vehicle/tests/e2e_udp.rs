//! End to end (M9 gate): test-equipment cFS source → UDP loopback → VehicleReceiver → readings,
//! with cabin CO₂ above the `co2.warning` fire threshold of the signed ruleset (docs/06 §3), so
//! the integrated station can show environment alarms firing from vehicle data (F-ENV-01,
//! F-GND-11, R-INT-04).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code

use std::time::Duration;

use caelum_domain::parameters::Parameter;
use caelum_domain::time::{LeapSeconds, MetEpoch, TaiNs};
use caelum_test_equipment::cfs::{self, mid, CfsUdpSource, Header, Layout};
use caelum_vehicle::{TimeSource, VehicleConfig, VehicleReceiver, VehicleValue};

const S: i64 = 1_000_000_000;
const EPOCH_S: i64 = 8_040 * 86_400;
const NOW_V: u32 = 1_400_000_000;

/// The `fire` value of `co2.warning` in the signed ruleset, so this test follows any change.
fn co2_warning_fire() -> f64 {
    let text = include_str!("../../caelum-alarms/rules/ruleset.toml");
    let doc: toml::Table = text.parse().expect("ruleset parses");
    doc["rule"]
        .as_array()
        .expect("rules")
        .iter()
        .find(|r| r["id"].as_str() == Some("co2.warning"))
        .and_then(|r| r["fire"].as_float())
        .expect("co2.warning fire threshold")
}

/// F-ENV-01, F-GND-11, R-INT-04: vehicle CO₂ reaches CAELUM over UDP above `co2.warning`.
#[tokio::test(flavor = "current_thread")]
async fn f_env_01_cfs_source_over_udp_gives_co2_above_warning_threshold() {
    let config =
        VehicleConfig::from_toml(include_str!("fixtures/reference.toml")).expect("reference table");
    let station_now = TaiNs((EPOCH_S + i64::from(NOW_V)) * S + 150_000_000);
    let mut receiver = VehicleReceiver::bind(
        config,
        LeapSeconds::bundled().expect("leap table"),
        MetEpoch(TaiNs((EPOCH_S + i64::from(NOW_V) - 86_400) * S)),
        move || Some(station_now),
    )
    .await
    .expect("bind loopback");
    let source = CfsUdpSource::loopback(receiver.local_addr().expect("addr")).expect("source");

    let header = |m: u16| Header {
        mid: m,
        sequence: 0,
        seconds: NOW_V,
        subseconds: 0,
    };
    let fire = co2_warning_fire();
    // A value above the threshold, supplied by the test (not invented by the source).
    let co2 = 5.2_f32;
    assert!(f64::from(co2) > fire);
    let time_hk = cfs::time_hk(&cfs::TimeHk {
        clock_set: true,
        leap_seconds: 37,
    });
    let atmosphere = cfs::cabin_atmosphere(&cfs::CabinAtmosphere {
        co2_mmhg: Some(co2),
        o2_kpa: Some(21.0),
        pressure_kpa: Some(101.0),
        temp_c: Some(23.0),
        humidity_pct: Some(40.0),
    });
    source
        .emit(&cfs::tlm_packet(Layout::Cfe7, header(mid::TIME_HK), &time_hk).expect("pkt"))
        .expect("emit time");
    source
        .emit(
            &cfs::tlm_packet(Layout::Cfe7, header(mid::CABIN_ATMOSPHERE), &atmosphere)
                .expect("pkt"),
        )
        .expect("emit atmosphere");

    let wait = Duration::from_secs(5);
    let first = tokio::time::timeout(wait, receiver.recv())
        .await
        .expect("time packet arrives")
        .expect("decoded");
    assert!(first.readings.is_empty() && first.rejections.is_empty());
    let second = tokio::time::timeout(wait, receiver.recv())
        .await
        .expect("atmosphere packet arrives")
        .expect("decoded");
    assert!(second.rejections.is_empty(), "{:?}", second.rejections);
    let reading = second
        .readings
        .iter()
        .find(|r| r.parameter == Parameter::CabinCo2)
        .expect("cabin_co2 reading");
    let VehicleValue::Number(value) = reading.value else {
        panic!("cabin_co2 is numeric");
    };
    assert!(value > fire, "{value} must exceed co2.warning {fire}");
    assert_eq!(
        reading.time,
        TimeSource::Vehicle {
            offset_ns: 150_000_000,
            exact: true
        }
    );
    assert_eq!(receiver.stats().readings, 5);
    assert_eq!(receiver.stats().datagrams, 2);
}
