//! Bluetooth SIG profile decoders, each checked through ingest (F-ACQ-02, R-CLN-02, SR-017; M2
//! gate: no defaults, demonstration/test readings rejected, no thermometer reading reaches
//! core_temp).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use caelum_devices::raw_reading;
use caelum_devices::sig::{self, DecodeError, Measurement};
use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::observation::{MeasurementMode, Quality, TemperatureSite};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::{LeapSeconds, TaiNs};
use caelum_domain::units::Unit;
use caelum_ingest::{ingest, Observation, RawValue, Rejection, Value};

fn leap() -> LeapSeconds {
    LeapSeconds::bundled().unwrap()
}

fn decode(uuid: u16, bytes: &[u8]) -> Measurement {
    sig::decode(uuid, bytes, &leap()).expect("decodes")
}

fn through_ingest(m: &Measurement) -> Vec<Result<Observation, Rejection>> {
    let crew = Some(CrewMemberId::from_uuid(uuid::Uuid::from_u128(1)));
    m.readings
        .iter()
        .cloned()
        .map(|d| {
            let raw = raw_reading(
                d,
                DeviceId::from_uuid(uuid::Uuid::from_u128(2)),
                TaiNs(0),
                m.mode,
            );
            ingest(raw, crew, None)
        })
        .collect()
}

fn number(o: &Result<Observation, Rejection>) -> f64 {
    match &o.as_ref().expect("accepted").value {
        Value::Number(v) => *v,
        Value::State(s) => panic!("state {s}"),
    }
}

/// 2017-01-01 00:00:00 as a GATT Date Time.
const DATE_2017: [u8; 7] = [0xE1, 0x07, 1, 1, 0, 0, 0];

#[test]
fn heart_rate_with_contact_and_rr_intervals() {
    // flags: UINT8, contact supported+detected, RR present; HR 72; RR 1024/1024 s and 512/1024 s.
    let m = decode(
        sig::HEART_RATE_MEASUREMENT,
        &[0x16, 72, 0x00, 0x04, 0x00, 0x02],
    );
    assert_eq!(m.mode, MeasurementMode::Continuous);
    let p: Vec<_> = m
        .readings
        .iter()
        .map(|d| (d.parameter, d.quality))
        .collect();
    assert_eq!(
        p,
        [
            (Parameter::HeartRate, Quality::Good),
            (Parameter::RrInterval, Quality::Good),
            (Parameter::RrInterval, Quality::Good)
        ]
    );
    let obs = through_ingest(&m);
    assert_eq!(number(&obs[0]), 72.0);
    assert_eq!(number(&obs[1]), 1_000.0);
    assert_eq!(number(&obs[2]), 500.0);
}

#[test]
fn heart_rate_without_skin_contact_is_unusable_and_uint16_is_read() {
    let m = decode(sig::HEART_RATE_MEASUREMENT, &[0x05, 0x2C, 0x01]); // UINT16 300, no contact
    assert_eq!(m.readings[0].value, Some(RawValue::Number(300.0)));
    assert_eq!(m.readings[0].quality, Quality::Unusable);
    let m = decode(sig::HEART_RATE_MEASUREMENT, &[0x00, 60]);
    assert_eq!(
        m.readings[0].quality,
        Quality::Degraded,
        "no contact indicator"
    );
}

/// M2 gate, SR-017: PLXS Measurement Status bits 10 (demonstration) and 11 (testing).
#[test]
fn sr_017_plx_demonstration_and_test_bits_are_rejected() {
    for (status, expected) in [
        (1u16 << 10, Rejection::DemonstrationData),
        (1u16 << 11, Rejection::TestData),
    ] {
        let s = status.to_le_bytes();
        // flags: measurement status present; SpO2 97, PR 60 (SFLOAT exponent 0).
        let m = decode(sig::PLX_SPOT_CHECK, &[0x02, 97, 0, 60, 0, s[0], s[1]]);
        for result in through_ingest(&m) {
            assert_eq!(result, Err(expected.clone()));
        }
    }
}

#[test]
fn plx_spot_check_timestamp_and_unset_clock() {
    let mut bytes = vec![0x01, 97, 0, 60, 0];
    bytes.extend(DATE_2017);
    let m = decode(sig::PLX_SPOT_CHECK, &bytes);
    assert_eq!(m.mode, MeasurementMode::Spot);
    assert!(m.readings[0].device_time.is_some());
    bytes[0] = 0x11; // timestamp + "device clock is not set"
    let m = decode(sig::PLX_SPOT_CHECK, &bytes);
    assert_eq!(
        m.readings[0].device_time, None,
        "an unset clock gives no time"
    );
}

#[test]
fn plx_status_sets_quality_and_unavailable_gives_no_value() {
    let unavailable = (1u16 << 13).to_le_bytes();
    let m = decode(
        sig::PLX_CONTINUOUS,
        &[0x04, 97, 0, 60, 0, unavailable[0], unavailable[1]],
    );
    assert!(through_ingest(&m)
        .iter()
        .all(|r| *r == Err(Rejection::MissingValue)));
    // Device and Sensor Status bit 15: sensor disconnected.
    let m = decode(sig::PLX_CONTINUOUS, &[0x08, 97, 0, 60, 0, 0x00, 0x80, 0x00]);
    assert_eq!(m.readings[0].quality, Quality::Unusable);
    // bit 5: low perfusion.
    let m = decode(sig::PLX_CONTINUOUS, &[0x08, 97, 0, 60, 0, 0x20, 0x00, 0x00]);
    assert_eq!(m.readings[0].quality, Quality::Poor);
    // SpO2 NaN: no value for SpO2 only.
    let m = decode(sig::PLX_CONTINUOUS, &[0x00, 0xFF, 0x07, 60, 0]);
    let obs = through_ingest(&m);
    assert_eq!(obs[0], Err(Rejection::MissingValue));
    assert_eq!(number(&obs[1]), 60.0);
}

#[test]
fn blood_pressure_in_kpa_is_converted_and_status_lowers_quality() {
    // flags: kPa, pulse rate, measurement status; 16.0 / 10.7 kPa (SFLOAT 160e-1, 107e-1).
    let sys = 0xF0A0u16.to_le_bytes();
    let dia = 0xF06Bu16.to_le_bytes();
    let map = 0xF082u16.to_le_bytes();
    let bytes = [
        0x15, sys[0], sys[1], dia[0], dia[1], map[0], map[1], 70, 0, 0x02,
        0x00, // cuff too loose
    ];
    let m = decode(sig::BLOOD_PRESSURE_MEASUREMENT, &bytes);
    assert_eq!(m.readings[0].unit, Some(Unit::KiloPascal));
    assert_eq!(m.readings[0].quality, Quality::Poor);
    let obs = through_ingest(&m);
    assert!((number(&obs[0]) - 120.0).abs() < 0.1, "{}", number(&obs[0]));
    assert!((number(&obs[1]) - 80.3).abs() < 0.1);
    assert_eq!(number(&obs[2]), 70.0);
}

#[test]
fn blood_pressure_user_id_is_reported() {
    let bytes = [0x08, 120, 0, 80, 0, 93, 0, 3];
    assert_eq!(
        decode(sig::BLOOD_PRESSURE_MEASUREMENT, &bytes).device_user,
        Some(3)
    );
    let bytes = [0x08, 120, 0, 80, 0, 93, 0, 0xFF];
    assert_eq!(
        decode(sig::BLOOD_PRESSURE_MEASUREMENT, &bytes).device_user,
        None
    );
}

/// M2 gate, docs/05 §1, CR-002 B6: thermometer readings go to body_temp_measured with their
/// site; no site is unknown; nothing goes to core_temp.
#[test]
fn thermometer_readings_go_to_body_temp_measured_never_core_temp() {
    let t = 0xFF00_0172u32.to_le_bytes(); // 37.0
    let m = decode(
        sig::TEMPERATURE_MEASUREMENT,
        &[0x04, t[0], t[1], t[2], t[3], 3],
    );
    let obs = through_ingest(&m);
    let o = obs[0].as_ref().unwrap();
    assert_eq!(o.parameter, Parameter::BodyTempMeasured);
    assert_eq!(o.provenance.site, Some(TemperatureSite::Ear));
    let m = decode(
        sig::TEMPERATURE_MEASUREMENT,
        &[0x00, t[0], t[1], t[2], t[3]],
    );
    let o = through_ingest(&m).remove(0).unwrap();
    assert_eq!(o.provenance.site, Some(TemperatureSite::Unknown));
    for uuid in sig::SUPPORTED {
        // No decoder ever produces core_temp.
        let probe = sig::decode(uuid, &[0x00; 32], &leap());
        if let Ok(m) = probe {
            assert!(m
                .readings
                .iter()
                .all(|d| d.parameter != Parameter::CoreTemp));
        }
    }
}

#[test]
fn thermometer_fahrenheit_and_nan() {
    let t = 0xFF00_03DAu32.to_le_bytes(); // 98.6 °F
    let m = decode(
        sig::TEMPERATURE_MEASUREMENT,
        &[0x01, t[0], t[1], t[2], t[3]],
    );
    assert!((number(&through_ingest(&m)[0]) - 37.0).abs() < 1e-9);
    let nan = 0x007F_FFFFu32.to_le_bytes();
    let m = decode(
        sig::TEMPERATURE_MEASUREMENT,
        &[0x00, nan[0], nan[1], nan[2], nan[3]],
    );
    assert_eq!(through_ingest(&m)[0], Err(Rejection::MissingValue));
}

#[test]
fn weight_si_imperial_and_unsuccessful() {
    let kg = 14_000u16.to_le_bytes(); // 70.000 kg at 0.005
    let m = decode(sig::WEIGHT_MEASUREMENT, &[0x00, kg[0], kg[1]]);
    assert!((number(&through_ingest(&m)[0]) - 70.0).abs() < 1e-9);
    let lb = 15_432u16.to_le_bytes(); // 154.32 lb
    let m = decode(sig::WEIGHT_MEASUREMENT, &[0x05, lb[0], lb[1], 2]);
    assert_eq!(m.device_user, Some(2));
    assert!((number(&through_ingest(&m)[0]) - 70.0).abs() < 0.01);
    let m = decode(sig::WEIGHT_MEASUREMENT, &[0x00, 0xFF, 0xFF]);
    assert_eq!(through_ingest(&m)[0], Err(Rejection::MissingValue));
}

fn glucose_bytes(flags: u8, concentration: u16, type_location: u8, status: Option<u16>) -> Vec<u8> {
    let mut b = vec![flags, 1, 0];
    b.extend(DATE_2017);
    if flags & 0x01 != 0 {
        b.extend(30i16.to_le_bytes()); // +30 min
    }
    b.extend(concentration.to_le_bytes());
    b.push(type_location);
    if let Some(s) = status {
        b.extend(s.to_le_bytes());
    }
    b
}

#[test]
fn glucose_units_time_offset_and_sample_type() {
    // kg/L: 95 × 10^-5 = 0.00095 kg/L = 95 mg/dL; capillary whole blood (type 1).
    let m = decode(
        sig::GLUCOSE_MEASUREMENT,
        &glucose_bytes(0x03, 0xB05F, 0x11, None),
    );
    let obs = through_ingest(&m);
    assert!((number(&obs[0]) - 95.0).abs() < 1e-6, "{}", number(&obs[0]));
    let base = decode(
        sig::GLUCOSE_MEASUREMENT,
        &glucose_bytes(0x02, 0xB05F, 0x11, None),
    );
    assert_eq!(
        m.readings[0].device_time.unwrap().0 - base.readings[0].device_time.unwrap().0,
        30 * 60 * 1_000_000_000
    );
    // mol/L: 53 × 10^-4 = 0.0053 mol/L = 5.3 mmol/L.
    let m = decode(
        sig::GLUCOSE_MEASUREMENT,
        &glucose_bytes(0x06, 0xC035, 0x11, None),
    );
    assert_eq!(m.readings[0].unit, Some(Unit::MmolPerL));
    assert!(through_ingest(&m)[0].is_ok());
}

/// SR-017: a control-solution sample is a device check, rejected as test data.
#[test]
fn sr_017_glucose_control_solution_is_test_data() {
    let m = decode(
        sig::GLUCOSE_MEASUREMENT,
        &glucose_bytes(0x02, 0xB05F, 0x1A, None),
    );
    assert_eq!(through_ingest(&m)[0], Err(Rejection::TestData));
}

#[test]
fn glucose_sensor_status_sets_quality_and_time_fault_drops_time() {
    let m = decode(
        sig::GLUCOSE_MEASUREMENT,
        &glucose_bytes(0x0A, 0xB05F, 0x11, Some(1 << 2)),
    );
    assert_eq!(
        m.readings[0].quality,
        Quality::Unusable,
        "sample size insufficient"
    );
    let m = decode(
        sig::GLUCOSE_MEASUREMENT,
        &glucose_bytes(0x0A, 0xB05F, 0x11, Some(1 << 11)),
    );
    assert_eq!(m.readings[0].device_time, None);
    assert_eq!(m.readings[0].quality, Quality::Good);
}

/// A malformed payload produces no readings at all.
#[test]
fn truncated_payloads_are_errors_not_partial_readings() {
    let cases: [(u16, &[u8]); 7] = [
        (sig::HEART_RATE_MEASUREMENT, &[0x01, 72]),
        (sig::HEART_RATE_MEASUREMENT, &[0x10, 72, 0x00]),
        (sig::PLX_SPOT_CHECK, &[0x02, 97, 0, 60, 0, 0]),
        (sig::PLX_CONTINUOUS, &[0x00, 97, 0]),
        (
            sig::BLOOD_PRESSURE_MEASUREMENT,
            &[0x02, 120, 0, 80, 0, 93, 0, 0xE1],
        ),
        (
            sig::TEMPERATURE_MEASUREMENT,
            &[0x04, 0x72, 0x01, 0x00, 0xFF],
        ),
        (sig::GLUCOSE_MEASUREMENT, &[0x02, 1, 0, 0xE1, 0x07]),
    ];
    for (uuid, bytes) in cases {
        assert!(
            matches!(
                sig::decode(uuid, bytes, &leap()),
                Err(DecodeError::Truncated { .. })
            ),
            "{uuid:#06x} {bytes:?}"
        );
    }
    assert!(matches!(
        sig::decode(sig::HEART_RATE_MEASUREMENT, &[], &leap()),
        Err(DecodeError::Truncated { .. })
    ));
    assert_eq!(
        sig::decode(0x2A19, &[50], &leap()),
        Err(DecodeError::Unsupported(0x2A19))
    );
}

/// F-ACQ-11: the Current Time characteristic gives the device clock, fractions included; an
/// unset date gives none.
#[test]
fn f_acq_11_current_time_is_read_for_clock_exchanges() {
    let mut bytes = DATE_2017.to_vec();
    bytes.extend([7, 128, 0]); // Sunday, 128/256 s, adjust reason
    let t = sig::current_time(&bytes, &leap()).unwrap().unwrap();
    let mut whole = DATE_2017.to_vec();
    whole.extend([7, 0, 0]);
    let base = sig::current_time(&whole, &leap()).unwrap().unwrap();
    assert_eq!(t.0 - base.0, 500_000_000);
    let unset = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    assert_eq!(sig::current_time(&unset, &leap()).unwrap(), None);
    assert!(sig::current_time(&[0xE1, 0x07, 1], &leap()).is_err());
}
