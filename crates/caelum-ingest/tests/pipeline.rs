//! Ingest pipeline tests (R-CLN-02, F-ACQ-11, R-OPS-04, SR-017, SR-018; M2 gate: no default
//! values, demonstration/test readings rejected, no thermometer reading reaches core_temp).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::observation::{MeasurementMode, Quality, TemperatureSite};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_domain::units::Unit;
use caelum_ingest::{
    ingest, ClockOffset, DeviceFlags, RawReading, RawValue, Rejection, TimeSource, Value,
};

const RECEIVED: i64 = 2_000_000_000_000_000_000;

fn crew() -> Option<CrewMemberId> {
    Some(CrewMemberId::from_uuid(uuid::Uuid::from_u128(1)))
}

fn reading(parameter: Parameter, value: Option<RawValue>, unit: Option<Unit>) -> RawReading {
    RawReading {
        device_id: DeviceId::from_uuid(uuid::Uuid::from_u128(2)),
        parameter,
        value,
        unit,
        device_time: None,
        received_at: TaiNs(RECEIVED),
        quality: Quality::Good,
        mode: MeasurementMode::Continuous,
        flags: DeviceFlags::default(),
        site: None,
    }
}

fn hr(v: f64) -> RawReading {
    reading(
        Parameter::HeartRate,
        Some(RawValue::Number(v)),
        Some(Unit::PerMinute),
    )
}

#[test]
fn a_valid_reading_becomes_one_observation() {
    let obs = ingest(hr(72.0), crew(), None).expect("accepted");
    assert_eq!(obs.value, Value::Number(72.0));
    assert_eq!(obs.unit, Unit::PerMinute);
    assert_eq!(obs.measured_at, TaiNs(RECEIVED));
    assert_eq!(obs.provenance.time, TimeSource::Receipt);
    assert_eq!(obs.provenance.site, None);
}

/// M2 gate, R-CLN-02, CLAUDE.md rule 2: missing fields produce no observation, never a default.
#[test]
fn r_cln_02_missing_fields_produce_no_observation() {
    let no_value = reading(Parameter::HeartRate, None, Some(Unit::PerMinute));
    assert_eq!(ingest(no_value, crew(), None), Err(Rejection::MissingValue));
    let no_unit = reading(Parameter::HeartRate, Some(RawValue::Number(72.0)), None);
    assert_eq!(ingest(no_unit, crew(), None), Err(Rejection::MissingUnit));
    assert_eq!(ingest(hr(72.0), None, None), Err(Rejection::Unassigned));
    let no_state = reading(Parameter::SleepStage, None, None);
    assert_eq!(ingest(no_state, crew(), None), Err(Rejection::MissingValue));
}

/// M2 gate, SR-017: demonstration and test readings are rejected in release builds.
#[cfg(not(feature = "test-equipment"))]
#[test]
fn sr_017_demonstration_and_test_readings_are_rejected() {
    let mut demo = hr(72.0);
    demo.flags.demonstration = true;
    assert_eq!(
        ingest(demo, crew(), None),
        Err(Rejection::DemonstrationData)
    );
    let mut test = hr(72.0);
    test.flags.test_data = true;
    assert_eq!(ingest(test, crew(), None), Err(Rejection::TestData));
}

/// M2 gate, docs/05 §1: no thermometer reading reaches core_temp.
#[test]
fn no_thermometer_reading_reaches_core_temp() {
    let mut r = reading(
        Parameter::CoreTemp,
        Some(RawValue::Number(37.0)),
        Some(Unit::Celsius),
    );
    assert_eq!(
        ingest(r.clone(), crew(), None),
        Err(Rejection::CoreTempFromDevice)
    );
    r.site = Some(TemperatureSite::Ear);
    assert_eq!(ingest(r, crew(), None), Err(Rejection::CoreTempFromDevice));
}

/// SR-018, CR-002 B6: a thermometer reading keeps its site; no site is `unknown`, not a guess.
#[test]
fn sr_018_thermometer_site_is_kept_or_unknown() {
    let mut r = reading(
        Parameter::BodyTempMeasured,
        Some(RawValue::Number(98.6)),
        Some(Unit::Fahrenheit),
    );
    r.mode = MeasurementMode::Spot;
    let obs = ingest(r.clone(), crew(), None).unwrap();
    assert_eq!(obs.provenance.site, Some(TemperatureSite::Unknown));
    let Value::Number(c) = obs.value else {
        panic!("number")
    };
    assert!((c - 37.0).abs() < 1e-9, "{c}");
    let original = obs.provenance.original.unwrap();
    assert_eq!((original.value, original.unit), (98.6, "[degF]"));

    r.site = Some(TemperatureSite::Tympanum);
    assert_eq!(
        ingest(r, crew(), None).unwrap().provenance.site,
        Some(TemperatureSite::Tympanum)
    );

    let mut wrong = hr(72.0);
    wrong.site = Some(TemperatureSite::Ear);
    assert_eq!(
        ingest(wrong, crew(), None),
        Err(Rejection::UnexpectedSite(Parameter::HeartRate))
    );
}

#[test]
fn values_outside_the_plausible_range_or_not_finite_are_rejected() {
    assert!(matches!(
        ingest(hr(400.0), crew(), None),
        Err(Rejection::Implausible { .. })
    ));
    assert!(matches!(
        ingest(hr(-1.0), crew(), None),
        Err(Rejection::Implausible { .. })
    ));
    assert_eq!(
        ingest(hr(f64::NAN), crew(), None),
        Err(Rejection::NotFinite)
    );
    assert_eq!(
        ingest(hr(f64::INFINITY), crew(), None),
        Err(Rejection::NotFinite)
    );
}

/// R-CLN-02: only explicit conversions; anything else is rejected, never passed through.
#[test]
fn units_without_a_defined_conversion_are_rejected() {
    let r = reading(
        Parameter::HeartRate,
        Some(RawValue::Number(72.0)),
        Some(Unit::Kilogram),
    );
    assert!(matches!(
        ingest(r, crew(), None),
        Err(Rejection::UnitNotConvertible {
            from: "kg",
            to: "/min"
        })
    ));
    // Glucose is the only parameter with a mass/molar conversion.
    let g = reading(
        Parameter::Glucose,
        Some(RawValue::Number(5.5)),
        Some(Unit::MmolPerL),
    );
    assert!(ingest(g, crew(), None).is_ok());
}

#[test]
fn enumerated_values_must_be_defined_states() {
    let ok = reading(
        Parameter::SleepStage,
        Some(RawValue::State("rem".into())),
        None,
    );
    assert_eq!(ingest(ok, crew(), None).unwrap().value, Value::State("rem"));
    let unknown = reading(
        Parameter::SleepStage,
        Some(RawValue::State("dozing".into())),
        None,
    );
    assert_eq!(
        ingest(unknown, crew(), None),
        Err(Rejection::UnknownState("dozing".into()))
    );
    let number = reading(
        Parameter::SleepStage,
        Some(RawValue::Number(1.0)),
        Some(Unit::One),
    );
    assert_eq!(ingest(number, crew(), None), Err(Rejection::WrongValueType));
    let state = reading(
        Parameter::HeartRate,
        Some(RawValue::State("high".into())),
        None,
    );
    assert_eq!(ingest(state, crew(), None), Err(Rejection::WrongValueType));
    // spe_state values are pending in the vehicle ICD: nothing is accepted until they exist.
    let spe = reading(
        Parameter::SpeState,
        Some(RawValue::State("nominal".into())),
        None,
    );
    assert!(matches!(
        ingest(spe, crew(), None),
        Err(Rejection::StatesPending(_))
    ));
}

/// F-ACQ-11, docs/05 §4: device time corrected by the measured offset, recorded in provenance.
#[test]
fn f_acq_11_device_time_is_corrected_and_recorded() {
    let offset = ClockOffset {
        offset_ns: 2_500_000_000,
        residual_ns: 4_000_000,
        measured_at: TaiNs(RECEIVED - 10),
    };
    let mut r = hr(72.0);
    r.device_time = Some(TaiNs(RECEIVED - 3_000_000_000));
    let obs = ingest(r.clone(), crew(), Some(&offset)).unwrap();
    assert_eq!(obs.measured_at, TaiNs(RECEIVED - 500_000_000));
    assert_eq!(
        obs.provenance.time,
        TimeSource::DeviceCorrected {
            offset_ns: 2_500_000_000,
            residual_ns: 4_000_000
        }
    );
    // A device time without a measured offset is not trusted: receipt time, marked as such.
    let obs = ingest(r, crew(), None).unwrap();
    assert_eq!(obs.measured_at, TaiNs(RECEIVED));
    assert_eq!(obs.provenance.time, TimeSource::ReceiptUnsynchronised);
}

#[test]
fn a_corrected_time_after_receipt_is_a_clock_fault() {
    let offset = ClockOffset {
        offset_ns: 0,
        residual_ns: 0,
        measured_at: TaiNs(RECEIVED),
    };
    let mut r = hr(72.0);
    r.device_time = Some(TaiNs(RECEIVED + 1_000_000_000));
    assert!(
        ingest(r.clone(), crew(), Some(&offset)).is_ok(),
        "1 s ahead is tolerated"
    );
    r.device_time = Some(TaiNs(RECEIVED + 1_500_000_000));
    assert_eq!(
        ingest(r, crew(), Some(&offset)),
        Err(Rejection::TimeInFuture { ahead_ms: 1_500 })
    );
}

#[test]
fn rejection_codes_are_stable_and_distinct() {
    let all = [
        Rejection::Unassigned,
        Rejection::DemonstrationData,
        Rejection::TestData,
        Rejection::CoreTempFromDevice,
        Rejection::UnexpectedSite(Parameter::HeartRate),
        Rejection::MissingValue,
        Rejection::MissingUnit,
        Rejection::WrongValueType,
        Rejection::UnknownState(String::new()),
        Rejection::StatesPending(""),
        Rejection::UnitNotConvertible { from: "", to: "" },
        Rejection::NotFinite,
        Rejection::Implausible {
            value: 0.0,
            min: 0.0,
            max: 0.0,
        },
        Rejection::TimeInFuture { ahead_ms: 0 },
    ];
    let mut codes: Vec<_> = all.iter().map(Rejection::code).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), all.len());
}

/// docs/05 §3: test-equipment builds keep flagged readings, classified as simulated only.
#[cfg(feature = "test-equipment")]
#[test]
fn sr_017_test_equipment_builds_keep_flagged_readings_as_simulated() {
    use caelum_domain::observation::Classification;
    let mut demo = hr(72.0);
    demo.flags.demonstration = true;
    assert_eq!(
        ingest(demo, crew(), None).unwrap().classification,
        Classification::Simulated
    );
    assert_eq!(
        ingest(hr(72.0), crew(), None).unwrap().classification,
        Classification::Measured
    );
}
