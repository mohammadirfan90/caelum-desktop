//! Decoder tests against packets from the independent test-equipment cFS source (F-ENV-01,
//! F-ENV-02, F-ENV-03, F-GND-11, R-INT-04, F-ACQ-11).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code

use std::net::IpAddr;

use caelum_domain::ids::CrewMemberId;
use caelum_domain::observation::{MeasurementMode, Quality};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::{LeapSeconds, MetEpoch, MetNs, TaiNs};
use caelum_domain::units::Unit;
use caelum_test_equipment::cfs::{self, mid, Header, Layout};
use caelum_vehicle::decode::Decoded;
use caelum_vehicle::{
    ClockState, Decoder, Rejection, Subject, TimeSource, VehicleConfig, VehicleReading,
    VehicleValue,
};

const S: i64 = 1_000_000_000;
const REFERENCE: &str = include_str!("fixtures/reference.toml");
/// 1980-01-06 TAI, the reference vehicle epoch, as TAI since 1958.
const EPOCH_S: i64 = 8_040 * 86_400;
/// Vehicle seconds used for "now" in these tests.
const NOW_V: u32 = 1_400_000_000;
/// MET epoch: ten days before NOW_V.
const MET_EPOCH_S: i64 = EPOCH_S + NOW_V as i64 - 10 * 86_400;

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("ip")
}

fn decoder_with(toml: &str) -> Decoder {
    let config = VehicleConfig::from_toml(toml).expect("valid table");
    Decoder::new(
        config,
        LeapSeconds::bundled().expect("leap table"),
        MetEpoch(TaiNs(MET_EPOCH_S * S)),
    )
}

fn decoder() -> Decoder {
    decoder_with(REFERENCE)
}

/// Station receipt time `ms` milliseconds after vehicle second `v`.
fn rx(v: u32, ms: i64) -> TaiNs {
    TaiNs((EPOCH_S + i64::from(v)) * S + ms * 1_000_000)
}

fn packet(m: u16, seq: u16, v: u32, payload: &[u8]) -> Vec<u8> {
    cfs::tlm_packet(
        Layout::Cfe7,
        Header {
            mid: m,
            sequence: seq,
            seconds: v,
            subseconds: 0,
        },
        payload,
    )
    .expect("telemetry packet")
}

fn decode(d: &mut Decoder, bytes: &[u8], received_at: TaiNs) -> Decoded {
    d.decode(bytes, loopback(), received_at)
}

fn clock_set(d: &mut Decoder, set: bool, v: u32) {
    let hk = cfs::time_hk(&cfs::TimeHk {
        clock_set: set,
        leap_seconds: 37,
    });
    let out = decode(d, &packet(mid::TIME_HK, 0, v, &hk), rx(v, 100));
    assert!(
        out.readings.is_empty() && out.rejections.is_empty(),
        "{out:?}"
    );
}

fn number(r: &VehicleReading) -> f64 {
    match r.value {
        VehicleValue::Number(v) => v,
        VehicleValue::State(s) => panic!("expected a number, got {s}"),
    }
}

fn find(out: &Decoded, p: Parameter) -> &VehicleReading {
    out.readings
        .iter()
        .find(|r| r.parameter == p)
        .unwrap_or_else(|| panic!("no {p:?} in {out:?}"))
}

fn atmosphere(co2: f32) -> cfs::CabinAtmosphere {
    cfs::CabinAtmosphere {
        co2_mmhg: Some(co2),
        o2_kpa: Some(21.2),
        pressure_kpa: Some(101.3),
        temp_c: Some(22.5),
        humidity_pct: Some(45.0),
    }
}

/// F-ENV-01: all five cabin-atmosphere values decode to catalog parameters and units, stamped
/// with vehicle time (clock set) and MET from the signed epoch.
#[test]
fn f_env_01_cabin_atmosphere_decodes_to_catalog_units() {
    let mut d = decoder();
    clock_set(&mut d, true, NOW_V);
    let bytes = packet(
        mid::CABIN_ATMOSPHERE,
        1,
        NOW_V,
        &cfs::cabin_atmosphere(&atmosphere(3.1)),
    );
    let out = decode(&mut d, &bytes, rx(NOW_V, 250));
    assert!(out.rejections.is_empty(), "{:?}", out.rejections);
    assert_eq!(out.readings.len(), 5);
    let expect = [
        (Parameter::CabinCo2, 3.1, Unit::MmHg),
        (Parameter::CabinO2, 21.2, Unit::KiloPascal),
        (Parameter::CabinPressure, 101.3, Unit::KiloPascal),
        (Parameter::CabinTemp, 22.5, Unit::Celsius),
        (Parameter::CabinHumidity, 45.0, Unit::Percent),
    ];
    for (p, v, unit) in expect {
        let r = find(&out, p);
        assert!((number(r) - v).abs() < 1e-5, "{p:?}: {}", number(r));
        assert_eq!(r.unit, unit);
        assert_eq!(r.quality, Quality::Good);
        assert_eq!(r.mode, MeasurementMode::Continuous);
        assert_eq!(r.mid, mid::CABIN_ATMOSPHERE);
        assert_eq!(r.message, "ECLSS_CABIN_ATMOS_TLM");
        assert_eq!(
            r.subject,
            Subject::Cabin {
                location: Some("cabin".into())
            }
        );
        assert_eq!(r.measured_at, TaiNs((EPOCH_S + i64::from(NOW_V)) * S));
        assert_eq!(r.received_at, rx(NOW_V, 250));
        assert_eq!(
            r.time,
            TimeSource::Vehicle {
                offset_ns: 250_000_000,
                exact: true
            }
        );
        assert_eq!(r.met, MetNs(10 * 86_400 * S));
    }
    assert_eq!(d.stats().readings, 5);
    assert_eq!(d.stats().per_mid[&mid::CABIN_ATMOSPHERE].packets, 1);
}

/// F-ENV-01, rule 2: a value whose validity bit is cleared is dropped, never filled in.
#[test]
fn f_env_01_invalid_values_are_dropped_not_defaulted() {
    let mut d = decoder();
    let only_co2 = cfs::CabinAtmosphere {
        co2_mmhg: Some(2.0),
        ..cfs::CabinAtmosphere::default()
    };
    let out = decode(
        &mut d,
        &packet(
            mid::CABIN_ATMOSPHERE,
            0,
            NOW_V,
            &cfs::cabin_atmosphere(&only_co2),
        ),
        rx(NOW_V, 10),
    );
    assert_eq!(out.readings.len(), 1);
    assert_eq!(out.readings[0].parameter, Parameter::CabinCo2);
    assert_eq!(out.rejections.len(), 4);
    assert!(out
        .rejections
        .iter()
        .all(|r| matches!(r, Rejection::SensorInvalid(_))));
    assert_eq!(d.stats().rejected["sensor_invalid"], 4);
}

/// F-ENV-02: area dose rate, cumulative dose and the SPE state (CR-015 §4).
#[test]
fn f_env_02_area_radiation_and_spe_state() {
    let mut d = decoder();
    clock_set(&mut d, true, NOW_V);
    let v = cfs::AreaRadiation {
        dose_rate_usv_h: Some(18.5),
        cumulative_msv: Some(42.25),
        spe_code: Some(3),
    };
    let out = decode(
        &mut d,
        &packet(mid::RADIATION_AREA, 0, NOW_V, &cfs::radiation_area(&v)),
        rx(NOW_V, 5),
    );
    assert!(out.rejections.is_empty(), "{:?}", out.rejections);
    assert!((number(find(&out, Parameter::RadiationDoseRate)) - 18.5).abs() < 1e-6);
    assert_eq!(
        find(&out, Parameter::RadiationDoseRate).unit,
        Unit::MicrosievertPerHour
    );
    assert!((number(find(&out, Parameter::RadiationDoseCumulative)) - 42.25).abs() < 1e-6);
    assert_eq!(
        find(&out, Parameter::SpeState).value,
        VehicleValue::State("in_progress")
    );
    assert_eq!(find(&out, Parameter::SpeState).original, None);
}

/// F-ENV-02: an SPE code the vehicle database does not map is rejected, never guessed.
#[test]
fn f_env_02_unmapped_spe_code_is_rejected() {
    let mut d = decoder();
    let v = cfs::AreaRadiation {
        dose_rate_usv_h: Some(1.0),
        cumulative_msv: Some(1.0),
        spe_code: Some(9),
    };
    let out = decode(
        &mut d,
        &packet(mid::RADIATION_AREA, 0, NOW_V, &cfs::radiation_area(&v)),
        rx(NOW_V, 5),
    );
    assert_eq!(out.readings.len(), 2);
    assert_eq!(
        out.rejections,
        [Rejection::UnknownState {
            parameter: Parameter::SpeState,
            code: 9
        }]
    );
}

fn crew(n: u8) -> CrewMemberId {
    format!("6f1c2f52-6a3e-4a8e-9a43-0d6c1f3e7a0{n}")
        .parse()
        .expect("uuid")
}

/// F-ENV-02: personal dosimetry is attributed to the crew member the table maps; an unmapped
/// vehicle crew ID drops the packet.
#[test]
fn f_env_02_personal_dose_is_attributed_by_crew_map() {
    let mut d = decoder();
    let v = cfs::PersonalDose {
        crew: 2,
        dose_rate_usv_h: Some(12.0),
        cumulative_msv: Some(30.5),
    };
    let out = decode(
        &mut d,
        &packet(
            mid::RADIATION_PERSONAL,
            0,
            NOW_V,
            &cfs::radiation_personal(&v),
        ),
        rx(NOW_V, 5),
    );
    assert_eq!(out.readings.len(), 2);
    assert!(out
        .readings
        .iter()
        .all(|r| r.subject == Subject::Crew(crew(2))));

    let stranger = cfs::PersonalDose { crew: 7, ..v };
    let out = decode(
        &mut d,
        &packet(
            mid::RADIATION_PERSONAL,
            1,
            NOW_V,
            &cfs::radiation_personal(&stranger),
        ),
        rx(NOW_V, 5),
    );
    assert!(out.readings.is_empty());
    assert_eq!(out.rejections, [Rejection::UnmappedCrew(7)]);
}

/// F-ENV-03: an exercise session gives load (W) and duration converted to minutes, as a spot
/// reading; with no validity bit in the message the quality is DEGRADED (CR-015 §5).
#[test]
fn f_env_03_exercise_session_load_and_duration() {
    let mut d = decoder();
    clock_set(&mut d, true, NOW_V);
    let v = cfs::ExerciseSession {
        crew: 1,
        mean_load_w: 175.0,
        duration_s: 2_700,
    };
    let out = decode(
        &mut d,
        &packet(mid::EXERCISE_SESSION, 0, NOW_V, &cfs::exercise_session(&v)),
        rx(NOW_V, 5),
    );
    assert!(out.rejections.is_empty(), "{:?}", out.rejections);
    let load = find(&out, Parameter::ExerciseLoad);
    assert!((number(load) - 175.0).abs() < 1e-9);
    assert_eq!(load.unit, Unit::Watt);
    let duration = find(&out, Parameter::ExerciseDuration);
    assert!((number(duration) - 45.0).abs() < 1e-9);
    assert_eq!(duration.unit, Unit::Minute);
    let original = duration.original.expect("original kept");
    assert_eq!(original.unit, "s");
    assert!((original.value - 2_700.0).abs() < 1e-9);
    for r in &out.readings {
        assert_eq!(r.mode, MeasurementMode::Spot);
        assert_eq!(r.quality, Quality::Degraded);
        assert_eq!(r.subject, Subject::Crew(crew(1)));
    }
}

/// F-ACQ-11, docs/05 §4: without a fresh clock-set report vehicle time is not used; the reading
/// gets the receipt time, marked unsynchronised.
#[test]
fn f_acq_11_vehicle_time_unused_until_clock_reported_set() {
    let mut d = decoder();
    let bytes = packet(
        mid::CABIN_ATMOSPHERE,
        0,
        NOW_V,
        &cfs::cabin_atmosphere(&atmosphere(3.0)),
    );
    let at = rx(NOW_V, 300);
    let out = decode(&mut d, &bytes, at);
    assert!(out
        .readings
        .iter()
        .all(|r| r.time == TimeSource::ReceiptUnsynchronised && r.measured_at == at));
    assert_eq!(d.correlation(at).clock, ClockState::Unknown);

    clock_set(&mut d, false, NOW_V);
    assert_eq!(d.correlation(at).clock, ClockState::NotSet);
    let out = decode(&mut d, &bytes, at);
    assert!(out
        .readings
        .iter()
        .all(|r| r.time == TimeSource::ReceiptUnsynchronised));

    clock_set(&mut d, true, NOW_V);
    let status = d.correlation(at);
    assert_eq!(status.clock, ClockState::Set);
    assert_eq!(status.vehicle_leap_seconds, Some(37));
    assert_eq!(status.station_leap_seconds, 37);
    assert_eq!(status.latest_met, Some(MetNs(10 * 86_400 * S)));
    // The report goes stale after clock_state_max_age_s (30 s).
    assert_eq!(d.correlation(rx(NOW_V, 31_000)).clock, ClockState::Stale);
}

/// docs/05 §4: a vehicle time more than 1 s after receipt is a clock fault; nothing is stored.
#[test]
fn f_acq_11_vehicle_time_in_the_future_is_rejected() {
    let mut d = decoder();
    clock_set(&mut d, true, NOW_V);
    let bytes = packet(
        mid::CABIN_ATMOSPHERE,
        0,
        NOW_V + 5,
        &cfs::cabin_atmosphere(&atmosphere(3.0)),
    );
    let out = decode(&mut d, &bytes, rx(NOW_V, 0));
    assert!(out.readings.is_empty());
    assert_eq!(
        out.rejections,
        [Rejection::TimeInFuture { ahead_ms: 5_000 }]
    );
}

/// R-INT-04, ICD-VEH §9: malformed, unknown and unauthorised datagrams are counted, never
/// decoded and never panic.
#[test]
fn r_int_04_malformed_unknown_and_foreign_packets_are_counted() {
    let mut d = decoder();
    let at = rx(NOW_V, 0);
    let good = cfs::cabin_atmosphere(&atmosphere(3.0));

    let cases: Vec<(Vec<u8>, &str)> = vec![
        (vec![], "too_short"),
        (vec![0x0C, 0x40, 0xC0], "too_short"),
        (packet(0x0C99, 0, NOW_V, &good), "unknown_mid"),
        (
            packet(mid::CABIN_ATMOSPHERE, 0, NOW_V, &good[..10]),
            "payload_too_short",
        ),
        (
            {
                let mut b = packet(mid::CABIN_ATMOSPHERE, 0, NOW_V, &good);
                b.push(0);
                b
            },
            "length_mismatch",
        ),
        (
            {
                let mut b = packet(mid::CABIN_ATMOSPHERE, 0, NOW_V, &good);
                b[0] |= 0x10; // telecommand bit
                b
            },
            "telecommand",
        ),
    ];
    for (bytes, code) in &cases {
        let out = decode(&mut d, bytes, at);
        assert!(out.readings.is_empty(), "{code}");
        assert_eq!(out.rejections.len(), 1, "{code}");
        assert_eq!(out.rejections[0].code(), *code);
    }

    let foreign: IpAddr = "10.0.0.9".parse().expect("ip");
    let out = d.decode(&packet(mid::CABIN_ATMOSPHERE, 0, NOW_V, &good), foreign, at);
    assert_eq!(out.rejections, [Rejection::Source(foreign)]);

    // Implausible (CO2 above the catalog sensor-fault limit) and non-finite values with the
    // validity bit set are dropped field by field.
    let mut bad = cfs::cabin_atmosphere(&atmosphere(75.0));
    bad[4..8].copy_from_slice(&f32::INFINITY.to_le_bytes());
    let out = decode(&mut d, &packet(mid::CABIN_ATMOSPHERE, 1, NOW_V, &bad), at);
    assert_eq!(out.readings.len(), 3);
    let codes: Vec<&str> = out.rejections.iter().map(Rejection::code).collect();
    assert_eq!(codes, ["implausible", "not_finite"]);

    let stats = d.stats();
    assert_eq!(stats.datagrams, 8);
    assert_eq!(stats.rejected["too_short"], 2);
    assert_eq!(stats.rejected["unknown_mid"], 1);
    assert_eq!(stats.rejected["source_not_allowed"], 1);
    assert_eq!(stats.rejected["telecommand"], 1);
}

/// SR-004 support: sequence-count gaps are counted per MID.
#[test]
fn r_int_04_sequence_gaps_are_counted() {
    let mut d = decoder();
    let p = cfs::cabin_atmosphere(&atmosphere(3.0));
    for seq in [0x3FFE, 0x3FFF, 0, 5] {
        decode(
            &mut d,
            &packet(mid::CABIN_ATMOSPHERE, seq, NOW_V, &p),
            rx(NOW_V, 0),
        );
    }
    let s = d.stats().per_mid[&mid::CABIN_ATMOSPHERE];
    assert_eq!(s.packets, 4);
    assert_eq!(s.sequence_gaps, 1);
    assert_eq!(s.last_received_at, Some(rx(NOW_V, 0)));
}

/// R-INT-04: SBN framing; control messages and peers outside the allow-list are ignored.
#[test]
fn r_int_04_sbn_framing() {
    let toml = REFERENCE.replacen(
        "framing = \"ccsds\"",
        "framing = \"sbn\"\nsbn_peers = [7]",
        1,
    );
    let mut d = decoder_with(&toml);
    let pkt = packet(
        mid::CABIN_ATMOSPHERE,
        0,
        NOW_V,
        &cfs::cabin_atmosphere(&atmosphere(3.0)),
    );
    let at = rx(NOW_V, 0);
    let out = decode(&mut d, &cfs::sbn_app(7, 66, &pkt).expect("sbn"), at);
    assert_eq!(out.readings.len(), 5);
    let out = decode(&mut d, &cfs::sbn_app(8, 66, &pkt).expect("sbn"), at);
    assert_eq!(out.rejections[0].code(), "sbn_peer");
    let out = decode(&mut d, &cfs::sbn(1, 7, 66, &[]).expect("sbn"), at);
    assert_eq!(out.rejections[0].code(), "sbn_control");
}

/// R-INT-04: a cFE 6 header layout is read when configured.
#[test]
fn r_int_04_cfe6_layout() {
    let mut d = decoder_with(&REFERENCE.replacen("header = \"cfe7\"", "header = \"cfe6\"", 1));
    let bytes = cfs::tlm_packet(
        Layout::Cfe6,
        Header {
            mid: mid::CABIN_ATMOSPHERE,
            sequence: 0,
            seconds: NOW_V,
            subseconds: 0,
        },
        &cfs::cabin_atmosphere(&atmosphere(3.0)),
    )
    .expect("packet");
    assert_eq!(decode(&mut d, &bytes, rx(NOW_V, 0)).readings.len(), 5);
}
