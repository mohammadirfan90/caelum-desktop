//! The parameter catalog: the single source of truth for every value CAELUM handles
//! (CLAUDE.md rule 7, docs/05_DATA_MODEL.md §1, F-SYS-07).
//!
//! ⚕ DRAFT engineering values, pending medical-lead review (CR-002 B3, CR-003):
//! - `plausible`: sensor-fault limits (outside them a reading is rejected as implausible); they
//!   are deliberately wide and are not clinical limits;
//! - `stale_after_s`: age after which the latest value is shown STALE and a technical alarm is
//!   raised (SR-004).
//!
//! LOINC codes are verified (LOINC 2.82, `data/loinc-subset.csv`); parameters without a verified
//! code are exported with the local CAELUM code system. IEEE 11073-10101 codes are added with the
//! GHS decoder in M7, verified against the nomenclature.

use serde::{Deserialize, Serialize};

use crate::units::Unit;

/// Every parameter in the v1.0 catalog (docs/05 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)] // each variant is documented by its catalog entry (label)
pub enum Parameter {
    HeartRate,
    PulseRate,
    RrInterval,
    HrvRmssd,
    HrvSdnn,
    Spo2,
    RespirationRate,
    RespirationAmplitude,
    CoreTemp,
    BodyTempMeasured,
    SkinTempProximal,
    SkinTempDistal,
    BpSystolic,
    BpDiastolic,
    BpTrendSystolic,
    Pat,
    Eda,
    ActivityLevel,
    SleepStage,
    AfProbability,
    RhythmClass,
    SqiEcg,
    SqiPpg,
    SqiResp,
    Weight,
    Glucose,
    PvtMedianRt,
    CheckinMood,
    CheckinSymptoms,
    CabinCo2,
    CabinO2,
    CabinPressure,
    CabinTemp,
    CabinHumidity,
    RadiationDoseRate,
    RadiationDoseCumulative,
    SpeState,
    ExerciseLoad,
    ExerciseDuration,
    BatteryLevel,
    DeviceClockOffset,
    Rssi,
    PacketLossPct,
    ReconnectCount,
}

/// Catalog category (docs/05 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    /// Measured vital signs.
    VitalSigns,
    /// Computed from other signals.
    Derived,
    /// Cabin and radiation environment (vehicle data).
    Environment,
    /// Context for interpreting other values (activity, check-ins, exercise).
    Context,
    /// Device and link health.
    Technical,
}

/// Value type with its admissible values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum ValueType {
    /// A number within sensor-fault limits `[min, max]`.
    Numeric {
        /// Lowest plausible value.
        min: f64,
        /// Highest plausible value.
        max: f64,
    },
    /// One of a fixed set of states.
    Enum(&'static [&'static str]),
    /// States not defined yet; every reading is rejected until they are (no guessed values).
    EnumPending(&'static str),
}

/// Downlink channels (docs/04 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    /// Mission control: status only, never physiology (R-SEC-06).
    Ops,
    /// Flight surgeon and BME: full clinical detail.
    Medical,
    /// De-identified, consented research data.
    Research,
}

/// XTCE encoding for downlinked values (F-GND-02).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum XtceType {
    /// IEEE 754 single precision.
    Float32,
    /// Unsigned 8-bit enumeration.
    UInt8Enum,
}

/// Retention class (docs/03 §4, docs/05 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Retention {
    /// Kept for the mission.
    Mission,
    /// Technical data (link and data quality): kept for the signed retention limit, except
    /// around alarms (CR-022).
    Technical,
}

/// One catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ParamSpec {
    /// The parameter.
    pub parameter: Parameter,
    /// Catalog key as in docs/05 §1.
    pub key: &'static str,
    /// Human-readable label.
    pub label: &'static str,
    /// Canonical unit; readings are converted to it.
    pub unit: Unit,
    /// Category.
    pub category: Category,
    /// Value type and plausible values.
    pub value: ValueType,
    /// ⚕ DRAFT: seconds after which the latest value is STALE (SR-004).
    pub stale_after_s: u32,
    /// Verified LOINC code, if one fits.
    pub loinc: Option<&'static str>,
    /// IEEE 11073-10101 reference ID (added and verified with the GHS decoder in M7).
    pub mdc: Option<&'static str>,
    /// XTCE encoding.
    pub xtce: XtceType,
    /// Channels the value may be downlinked on.
    pub channels: &'static [Channel],
    /// Retention class.
    pub retention: Retention,
}

const PHYSIOLOGY: &[Channel] = &[Channel::Medical, Channel::Research];
const MEDICAL_ONLY: &[Channel] = &[Channel::Medical];
const TECHNICAL: &[Channel] = &[Channel::Ops, Channel::Medical];

const fn num(min: f64, max: f64) -> ValueType {
    ValueType::Numeric { min, max }
}

macro_rules! spec {
    ($p:ident, $key:literal, $label:literal, $unit:ident, $cat:ident, $value:expr, $stale:literal, $loinc:expr, $channels:expr) => {
        ParamSpec {
            parameter: Parameter::$p,
            key: $key,
            label: $label,
            unit: Unit::$unit,
            category: Category::$cat,
            value: $value,
            stale_after_s: $stale,
            loinc: $loinc,
            mdc: None,
            xtce: match $value {
                ValueType::Numeric { .. } => XtceType::Float32,
                ValueType::Enum(_) | ValueType::EnumPending(_) => XtceType::UInt8Enum,
            },
            channels: $channels,
            retention: if matches!(Category::$cat, Category::Technical) {
                Retention::Technical
            } else {
                Retention::Mission
            },
        }
    };
}

/// The v1.0 catalog, in docs/05 §1 order.
pub const CATALOG: [ParamSpec; 44] = [
    spec!(
        HeartRate,
        "heart_rate",
        "Heart rate",
        PerMinute,
        VitalSigns,
        num(20.0, 300.0),
        30,
        Some("8867-4"),
        PHYSIOLOGY
    ),
    spec!(
        PulseRate,
        "pulse_rate",
        "Pulse rate (PPG)",
        PerMinute,
        VitalSigns,
        num(20.0, 300.0),
        30,
        Some("8889-8"),
        PHYSIOLOGY
    ),
    spec!(
        RrInterval,
        "rr_interval",
        "R-R interval",
        Millisecond,
        Derived,
        num(200.0, 3_000.0),
        30,
        None,
        PHYSIOLOGY
    ),
    spec!(
        HrvRmssd,
        "hrv_rmssd",
        "Heart-rate variability (RMSSD)",
        Millisecond,
        Derived,
        num(1.0, 300.0),
        600,
        None,
        PHYSIOLOGY
    ),
    spec!(
        HrvSdnn,
        "hrv_sdnn",
        "Heart-rate variability (SDNN)",
        Millisecond,
        Derived,
        num(1.0, 300.0),
        600,
        Some("80404-7"),
        PHYSIOLOGY
    ),
    spec!(
        Spo2,
        "spo2",
        "Oxygen saturation (SpO2)",
        Percent,
        VitalSigns,
        num(40.0, 100.0),
        30,
        Some("59408-5"),
        PHYSIOLOGY
    ),
    spec!(
        RespirationRate,
        "respiration_rate",
        "Respiratory rate",
        PerMinute,
        VitalSigns,
        num(2.0, 80.0),
        60,
        Some("9279-1"),
        PHYSIOLOGY
    ),
    spec!(
        RespirationAmplitude,
        "respiration_amplitude",
        "Respiration amplitude (relative, uncalibrated)",
        One,
        Derived,
        num(0.0, 10.0),
        60,
        None,
        PHYSIOLOGY
    ),
    spec!(
        CoreTemp,
        "core_temp",
        "Core body temperature (estimated)",
        Celsius,
        Derived,
        num(30.0, 44.0),
        300,
        Some("8329-5"),
        PHYSIOLOGY
    ),
    spec!(
        BodyTempMeasured,
        "body_temp_measured",
        "Body temperature (thermometer)",
        Celsius,
        VitalSigns,
        num(30.0, 44.0),
        86_400,
        Some("8310-5"),
        PHYSIOLOGY
    ),
    spec!(
        SkinTempProximal,
        "skin_temp_proximal",
        "Skin temperature (proximal)",
        Celsius,
        VitalSigns,
        num(20.0, 42.0),
        120,
        Some("39106-0"),
        PHYSIOLOGY
    ),
    spec!(
        SkinTempDistal,
        "skin_temp_distal",
        "Skin temperature (distal)",
        Celsius,
        VitalSigns,
        num(15.0, 42.0),
        120,
        Some("39106-0"),
        PHYSIOLOGY
    ),
    spec!(
        BpSystolic,
        "bp_systolic",
        "Systolic blood pressure",
        MmHg,
        VitalSigns,
        num(50.0, 260.0),
        86_400,
        Some("8480-6"),
        PHYSIOLOGY
    ),
    spec!(
        BpDiastolic,
        "bp_diastolic",
        "Diastolic blood pressure",
        MmHg,
        VitalSigns,
        num(20.0, 160.0),
        86_400,
        Some("8462-4"),
        PHYSIOLOGY
    ),
    spec!(
        BpTrendSystolic,
        "bp_trend_systolic",
        "Systolic blood pressure trend (estimated)",
        MmHg,
        Derived,
        num(50.0, 260.0),
        300,
        None,
        PHYSIOLOGY
    ),
    spec!(
        Pat,
        "pat",
        "Pulse arrival time",
        Millisecond,
        Derived,
        num(50.0, 600.0),
        300,
        None,
        PHYSIOLOGY
    ),
    spec!(
        Eda,
        "eda",
        "Electrodermal activity",
        Microsiemens,
        VitalSigns,
        num(0.0, 100.0),
        120,
        None,
        PHYSIOLOGY
    ),
    spec!(
        ActivityLevel,
        "activity_level",
        "Activity level",
        One,
        Context,
        num(0.0, 1.0),
        120,
        None,
        PHYSIOLOGY
    ),
    spec!(
        SleepStage,
        "sleep_stage",
        "Sleep stage",
        One,
        Derived,
        ValueType::Enum(&["wake", "light", "deep", "rem", "sleep"]),
        600,
        None,
        PHYSIOLOGY
    ),
    spec!(
        AfProbability,
        "af_probability",
        "Atrial-fibrillation probability (model, advisory)",
        One,
        Derived,
        num(0.0, 1.0),
        600,
        None,
        PHYSIOLOGY
    ),
    spec!(
        RhythmClass,
        "rhythm_class",
        "Rhythm class (model, advisory)",
        One,
        Derived,
        ValueType::Enum(&["sinus", "af", "other", "noise"]),
        600,
        None,
        PHYSIOLOGY
    ),
    spec!(
        SqiEcg,
        "sqi_ecg",
        "ECG signal quality index",
        One,
        Technical,
        num(0.0, 1.0),
        30,
        None,
        TECHNICAL
    ),
    spec!(
        SqiPpg,
        "sqi_ppg",
        "PPG signal quality index",
        One,
        Technical,
        num(0.0, 1.0),
        30,
        None,
        TECHNICAL
    ),
    spec!(
        SqiResp,
        "sqi_resp",
        "Respiration signal quality index",
        One,
        Technical,
        num(0.0, 1.0),
        60,
        None,
        TECHNICAL
    ),
    spec!(
        Weight,
        "weight",
        "Body weight",
        Kilogram,
        VitalSigns,
        num(20.0, 250.0),
        604_800,
        Some("29463-7"),
        PHYSIOLOGY
    ),
    spec!(
        Glucose,
        "glucose",
        "Glucose (capillary, glucometer)",
        MgPerDl,
        VitalSigns,
        num(18.0, 720.0),
        86_400,
        Some("41653-7"),
        PHYSIOLOGY
    ),
    spec!(
        PvtMedianRt,
        "pvt_median_rt",
        "PVT median reaction time",
        Millisecond,
        Derived,
        num(100.0, 3_000.0),
        604_800,
        None,
        PHYSIOLOGY
    ),
    spec!(
        CheckinMood,
        "checkin_mood",
        "Check-in mood (1–5)",
        One,
        Context,
        num(1.0, 5.0),
        172_800,
        None,
        MEDICAL_ONLY
    ),
    spec!(
        CheckinSymptoms,
        "checkin_symptoms",
        "Check-in symptom score (0–10)",
        One,
        Context,
        num(0.0, 10.0),
        172_800,
        None,
        MEDICAL_ONLY
    ),
    spec!(
        CabinCo2,
        "cabin_co2",
        "Cabin CO2 partial pressure",
        MmHg,
        Environment,
        num(0.0, 60.0),
        60,
        None,
        MEDICAL_ONLY
    ),
    spec!(
        CabinO2,
        "cabin_o2",
        "Cabin O2 partial pressure",
        KiloPascal,
        Environment,
        num(0.0, 60.0),
        60,
        None,
        MEDICAL_ONLY
    ),
    spec!(
        CabinPressure,
        "cabin_pressure",
        "Cabin total pressure",
        KiloPascal,
        Environment,
        num(0.0, 120.0),
        60,
        None,
        MEDICAL_ONLY
    ),
    spec!(
        CabinTemp,
        "cabin_temp",
        "Cabin temperature",
        Celsius,
        Environment,
        num(0.0, 50.0),
        120,
        None,
        MEDICAL_ONLY
    ),
    spec!(
        CabinHumidity,
        "cabin_humidity",
        "Cabin relative humidity",
        Percent,
        Environment,
        num(0.0, 100.0),
        120,
        None,
        MEDICAL_ONLY
    ),
    spec!(
        RadiationDoseRate,
        "radiation_dose_rate",
        "Radiation dose rate",
        MicrosievertPerHour,
        Environment,
        num(0.0, 100_000.0),
        300,
        None,
        MEDICAL_ONLY
    ),
    spec!(
        RadiationDoseCumulative,
        "radiation_dose_cumulative",
        "Cumulative radiation dose",
        Millisievert,
        Environment,
        num(0.0, 10_000.0),
        3_600,
        None,
        MEDICAL_ONLY
    ),
    spec!(
        SpeState,
        "spe_state",
        "Solar-particle-event state",
        One,
        Environment,
        ValueType::EnumPending("states defined by ICD-VEH OI-2"),
        300,
        None,
        MEDICAL_ONLY
    ),
    spec!(
        ExerciseLoad,
        "exercise_load",
        "Exercise load",
        Watt,
        Context,
        num(0.0, 2_000.0),
        300,
        None,
        PHYSIOLOGY
    ),
    spec!(
        ExerciseDuration,
        "exercise_duration",
        "Exercise duration",
        Minute,
        Context,
        num(0.0, 480.0),
        86_400,
        None,
        PHYSIOLOGY
    ),
    spec!(
        BatteryLevel,
        "battery_level",
        "Device battery level",
        Percent,
        Technical,
        num(0.0, 100.0),
        600,
        None,
        TECHNICAL
    ),
    spec!(
        DeviceClockOffset,
        "device_clock_offset",
        "Device clock offset",
        Millisecond,
        Technical,
        num(-86_400_000.0, 86_400_000.0),
        300,
        None,
        TECHNICAL
    ),
    spec!(
        Rssi,
        "rssi",
        "Link signal strength (RSSI)",
        DbMilliwatt,
        Technical,
        num(-127.0, 20.0),
        60,
        None,
        TECHNICAL
    ),
    spec!(
        PacketLossPct,
        "packet_loss_pct",
        "Link packet loss",
        Percent,
        Technical,
        num(0.0, 100.0),
        120,
        None,
        TECHNICAL
    ),
    spec!(
        ReconnectCount,
        "reconnect_count",
        "Link reconnect count",
        One,
        Technical,
        num(0.0, 1_000_000.0),
        600,
        None,
        TECHNICAL
    ),
];

impl Parameter {
    /// The catalog entry for this parameter.
    pub fn spec(self) -> &'static ParamSpec {
        // CATALOG is in enum order (checked by a test), so the discriminant indexes it.
        &CATALOG[self as usize]
    }

    /// Looks a parameter up by its catalog key.
    pub fn from_key(key: &str) -> Option<Self> {
        CATALOG.iter().find(|s| s.key == key).map(|s| s.parameter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOINC_SUBSET: &str = include_str!("../data/loinc-subset.csv");
    const DATA_MODEL_DOC: &str = include_str!("../../../docs/05_DATA_MODEL.md");

    /// The keys listed in docs/05 §1 ("v1.0 set (minimum): …").
    fn documented_keys() -> Vec<String> {
        let start = DATA_MODEL_DOC
            .find("v1.0 set (minimum):")
            .expect("docs/05 lists the v1.0 set");
        let rest = &DATA_MODEL_DOC[start + "v1.0 set (minimum):".len()..];
        let end = rest.find('.').expect("the list ends with a full stop");
        rest[..end]
            .split(',')
            .map(|k| k.trim().to_owned())
            .collect()
    }

    /// F-SYS-07, CLAUDE.md rule 7: the catalog has exactly the parameters docs/05 §1 lists, in order.
    #[test]
    fn catalog_matches_the_data_model_document() {
        let keys: Vec<String> = CATALOG.iter().map(|s| s.key.to_owned()).collect();
        assert_eq!(keys, documented_keys());
    }

    #[test]
    fn catalog_is_in_enum_order_and_keys_round_trip() {
        for (i, s) in CATALOG.iter().enumerate() {
            assert_eq!(s.parameter as usize, i, "{} out of order", s.key);
            assert_eq!(s.parameter.spec().key, s.key);
            assert_eq!(Parameter::from_key(s.key), Some(s.parameter));
        }
        assert_eq!(Parameter::from_key("skin_temp"), None);
    }

    /// CR-003 §7: every LOINC code in the catalog was verified (data/loinc-subset.csv).
    #[test]
    fn every_loinc_code_is_verified() {
        let verified: Vec<&str> = LOINC_SUBSET
            .lines()
            .skip(1)
            .filter_map(|l| l.split(',').next())
            .collect();
        for s in CATALOG.iter() {
            if let Some(code) = s.loinc {
                assert!(
                    verified.contains(&code),
                    "{}: LOINC {code} not verified",
                    s.key
                );
            }
        }
    }

    /// CR-002 B6: pulse rate and heart rate have distinct LOINC codes; thermometer and core
    /// temperature estimate are distinct parameters.
    #[test]
    fn cr_002_b6_distinct_codes_and_temperatures() {
        assert_ne!(
            Parameter::HeartRate.spec().loinc,
            Parameter::PulseRate.spec().loinc
        );
        assert_eq!(Parameter::PulseRate.spec().loinc, Some("8889-8"));
        assert_eq!(Parameter::CoreTemp.spec().category, Category::Derived);
        assert_eq!(
            Parameter::BodyTempMeasured.spec().category,
            Category::VitalSigns
        );
    }

    /// R-SEC-06: no physiological, environmental or check-in value may go on the OPS channel;
    /// only technical health may.
    #[test]
    fn r_sec_06_only_technical_values_may_go_on_ops() {
        for s in CATALOG.iter() {
            let on_ops = s.channels.contains(&Channel::Ops);
            assert_eq!(on_ops, s.category == Category::Technical, "{}", s.key);
        }
    }

    #[test]
    fn plausible_ranges_and_staleness_are_well_formed() {
        for s in CATALOG.iter() {
            assert!(s.stale_after_s > 0, "{}", s.key);
            match s.value {
                ValueType::Numeric { min, max } => {
                    assert!(min.is_finite() && max.is_finite() && min < max, "{}", s.key)
                }
                ValueType::Enum(values) => assert!(!values.is_empty(), "{}", s.key),
                ValueType::EnumPending(reason) => assert!(!reason.is_empty(), "{}", s.key),
            }
        }
    }

    /// No guessed states: the SPE state is rejected until ICD-VEH defines it (CLAUDE.md rule 2).
    #[test]
    fn spe_state_values_are_pending_not_invented() {
        assert!(matches!(
            Parameter::SpeState.spec().value,
            ValueType::EnumPending(_)
        ));
    }
}
