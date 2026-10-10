//! HL7 FHIR R4 export (F-GND-09, F-SYS-08, docs/04 §5): a `collection` Bundle of Patient,
//! Observation, Device and DiagnosticReport resources for a medical record system (e.g. NASA's
//! Medical Data Architecture) or any FHIR server.
//!
//! - Codes: LOINC only from the parameter catalog (verified codes); a parameter without one uses
//!   the local code system [`LOCAL_PARAMETER_SYSTEM`] with its catalog key. Units are UCUM from
//!   the catalog.
//! - Times: TAI converted to UTC with the bundled IERS leap-second table (an inserted leap
//!   second is written as `:60`).
//! - Simulated / test-equipment data cannot be exported: [`ExportClassification`] has no such
//!   variant and the conversion from the domain classification refuses it.

use caelum_domain::ids::{AlarmId, CrewMemberId, DeviceId};
use caelum_domain::observation::{Classification, Quality};
use caelum_domain::parameters::{Category, Parameter, ValueType};
use caelum_domain::time::{LeapSeconds, TaiNs};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::ops::{OpsCategory, OpsPriority};

/// LOINC system URI.
pub const LOINC: &str = "http://loinc.org";
/// UCUM system URI.
pub const UCUM: &str = "http://unitsofmeasure.org";
/// Local code system for catalog parameters without a verified LOINC code.
pub const LOCAL_PARAMETER_SYSTEM: &str = "urn:caelum:parameter";
/// Identifier system of CAELUM record IDs.
pub const RECORD_SYSTEM: &str = "urn:caelum:record";

/// Export errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExportError {
    /// The value does not fit the parameter.
    #[error("invalid value for `{key}`: {reason}")]
    InvalidValue {
        /// Catalog key.
        key: &'static str,
        /// Why.
        reason: String,
    },
    /// Simulated or test data.
    #[error("simulated or test-equipment data cannot be exported")]
    Simulated,
    /// Time conversion failed.
    #[error("time: {0}")]
    Time(String),
}

/// How an exported value was obtained. There is deliberately no simulated variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportClassification {
    /// Measured by a device.
    Measured,
    /// Derived by a versioned algorithm.
    Derived,
    /// Entered by a person.
    Entered,
    /// Imported from a record.
    Imported,
}

impl TryFrom<Classification> for ExportClassification {
    type Error = ExportError;

    fn try_from(c: Classification) -> Result<Self, ExportError> {
        #[allow(unreachable_patterns)] // `Simulated` exists only in test-equipment builds
        match c {
            Classification::Measured => Ok(Self::Measured),
            Classification::Derived => Ok(Self::Derived),
            Classification::Entered => Ok(Self::Entered),
            Classification::Imported => Ok(Self::Imported),
            _ => Err(ExportError::Simulated),
        }
    }
}

impl ExportClassification {
    fn code(self) -> &'static str {
        match self {
            Self::Measured => "measured",
            Self::Derived => "derived",
            Self::Entered => "entered",
            Self::Imported => "imported",
        }
    }
}

/// An exported value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExportValue {
    /// Number in the catalog unit.
    Quantity(f64),
    /// Index into the catalog's states.
    State(u8),
}

/// An observation to export.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportObservation {
    /// CAELUM record ID.
    pub id: Uuid,
    /// Crew member.
    pub crew: CrewMemberId,
    /// Parameter.
    pub parameter: Parameter,
    /// Value.
    pub value: ExportValue,
    /// Measured at (TAI).
    pub at: TaiNs,
    /// Source device.
    pub device: Option<DeviceId>,
    /// Quality.
    pub quality: Quality,
    /// Classification.
    pub classification: ExportClassification,
}

/// A device to export. Unknown fields are absent, never guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportDevice {
    /// Device ID.
    pub id: DeviceId,
    /// Manufacturer.
    pub manufacturer: Option<String>,
    /// Model.
    pub model: Option<String>,
    /// Serial number.
    pub serial: Option<String>,
}

/// An alarm event, exported as a DiagnosticReport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportAlarm {
    /// Alarm ID.
    pub id: AlarmId,
    /// Crew member.
    pub crew: CrewMemberId,
    /// Priority.
    pub priority: OpsPriority,
    /// Category.
    pub category: OpsCategory,
    /// Rule ID (MEDICAL channel content).
    pub rule_id: String,
    /// Ruleset version.
    pub ruleset_version: String,
    /// Created at (TAI).
    pub created_at: TaiNs,
    /// Observations behind the alarm (record IDs in the same bundle).
    pub observations: Vec<Uuid>,
}

const NS_PER_S: i64 = 1_000_000_000;

/// Civil date from days since 1970-01-01 (proleptic Gregorian).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// FHIR `instant` / `dateTime` in UTC with milliseconds.
pub fn utc_string(t: TaiNs, leaps: &LeapSeconds) -> Result<String, ExportError> {
    let (utc, _) = leaps
        .tai_to_utc(t)
        .map_err(|e| ExportError::Time(e.to_string()))?;
    let secs = utc.unix_ns.div_euclid(NS_PER_S);
    let ms = utc.unix_ns.rem_euclid(NS_PER_S) / 1_000_000;
    let (y, mo, d) = civil(secs.div_euclid(86_400));
    let sod = secs.rem_euclid(86_400);
    let s = sod % 60 + i64::from(utc.leap_second);
    Ok(format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{s:02}.{ms:03}Z",
        sod / 3600,
        sod % 3600 / 60
    ))
}

fn quality_code(q: Quality) -> &'static str {
    match q {
        Quality::Good => "good",
        Quality::Degraded => "degraded",
        Quality::Poor => "poor",
        Quality::Unusable => "unusable",
    }
}

/// The FHIR `code` of a parameter.
pub fn parameter_code(p: Parameter) -> Value {
    let s = p.spec();
    match s.loinc {
        Some(code) => {
            json!({ "coding": [{ "system": LOINC, "code": code, "display": s.label }], "text": s.label })
        }
        None => {
            json!({ "coding": [{ "system": LOCAL_PARAMETER_SYSTEM, "code": s.key, "display": s.label }], "text": s.label })
        }
    }
}

/// An Observation resource.
pub fn observation(o: &ExportObservation, leaps: &LeapSeconds) -> Result<Value, ExportError> {
    let s = o.parameter.spec();
    let invalid = |reason: &str| ExportError::InvalidValue {
        key: s.key,
        reason: reason.to_owned(),
    };
    let value = match (s.value, o.value) {
        (ValueType::Numeric { .. }, ExportValue::Quantity(v)) => {
            if !v.is_finite() {
                return Err(invalid("not finite"));
            }
            (
                "valueQuantity",
                json!({ "value": v, "unit": s.unit.ucum(), "system": UCUM, "code": s.unit.ucum() }),
            )
        }
        (ValueType::Enum(states), ExportValue::State(i)) => {
            let state = states
                .get(usize::from(i))
                .ok_or_else(|| invalid("state index out of range"))?;
            (
                "valueCodeableConcept",
                json!({ "coding": [{ "system": format!("{LOCAL_PARAMETER_SYSTEM}:{}", s.key), "code": state }] }),
            )
        }
        (ValueType::EnumPending(_), _) => return Err(invalid("states not defined")),
        _ => return Err(invalid("value kind does not match the parameter")),
    };
    let mut r = json!({
        "resourceType": "Observation",
        "id": o.id.to_string(),
        "identifier": [{ "system": RECORD_SYSTEM, "value": o.id.to_string() }],
        "status": "final",
        "code": parameter_code(o.parameter),
        "subject": { "reference": format!("urn:uuid:{}", o.crew) },
        "effectiveDateTime": utc_string(o.at, leaps)?,
        "extension": [
            { "url": "urn:caelum:quality", "valueCode": quality_code(o.quality) },
            { "url": "urn:caelum:classification", "valueCode": o.classification.code() }
        ]
    });
    r[value.0] = value.1;
    if s.category == Category::VitalSigns {
        r["category"] = json!([{ "coding": [{
            "system": "http://terminology.hl7.org/CodeSystem/observation-category",
            "code": "vital-signs"
        }] }]);
    }
    if let Some(d) = o.device {
        r["device"] = json!({ "reference": format!("urn:uuid:{d}") });
    }
    Ok(r)
}

/// A Device resource.
pub fn device(d: &ExportDevice) -> Value {
    let mut r = json!({ "resourceType": "Device", "id": d.id.to_string() });
    if let Some(m) = &d.manufacturer {
        r["manufacturer"] = json!(m);
    }
    if let Some(m) = &d.model {
        r["deviceName"] = json!([{ "name": m, "type": "model-name" }]);
    }
    if let Some(s) = &d.serial {
        r["serialNumber"] = json!(s);
    }
    r
}

/// A DiagnosticReport for an alarm event.
pub fn alarm_report(a: &ExportAlarm, leaps: &LeapSeconds) -> Result<Value, ExportError> {
    Ok(json!({
        "resourceType": "DiagnosticReport",
        "id": a.id.to_string(),
        "identifier": [{ "system": RECORD_SYSTEM, "value": a.id.to_string() }],
        "status": "final",
        "code": { "coding": [{ "system": "urn:caelum:alarm", "code": a.category.name() }], "text": "CAELUM alarm" },
        "subject": { "reference": format!("urn:uuid:{}", a.crew) },
        "effectiveDateTime": utc_string(a.created_at, leaps)?,
        "result": a.observations.iter().map(|o| json!({ "reference": format!("urn:uuid:{o}") })).collect::<Vec<_>>(),
        "conclusion": format!(
            "{} {} alarm, rule {} (ruleset {})",
            a.priority.name(), a.category.name(), a.rule_id, a.ruleset_version
        )
    }))
}

/// A `collection` Bundle with a Patient entry per crew member referenced.
pub fn bundle(
    bundle_id: Uuid,
    observations: &[ExportObservation],
    devices: &[ExportDevice],
    alarms: &[ExportAlarm],
    created: TaiNs,
    leaps: &LeapSeconds,
) -> Result<Value, ExportError> {
    let mut crews: Vec<CrewMemberId> = observations
        .iter()
        .map(|o| o.crew)
        .chain(alarms.iter().map(|a| a.crew))
        .collect();
    crews.sort();
    crews.dedup();
    let mut entries = Vec::new();
    for c in crews {
        entries.push(json!({
            "fullUrl": format!("urn:uuid:{c}"),
            "resource": { "resourceType": "Patient", "id": c.to_string(),
                "identifier": [{ "system": "urn:caelum:crew", "value": c.to_string() }] }
        }));
    }
    for d in devices {
        entries.push(json!({ "fullUrl": format!("urn:uuid:{}", d.id), "resource": device(d) }));
    }
    for o in observations {
        entries.push(
            json!({ "fullUrl": format!("urn:uuid:{}", o.id), "resource": observation(o, leaps)? }),
        );
    }
    for a in alarms {
        entries.push(
            json!({ "fullUrl": format!("urn:uuid:{}", a.id), "resource": alarm_report(a, leaps)? }),
        );
    }
    Ok(json!({
        "resourceType": "Bundle",
        "id": bundle_id.to_string(),
        "type": "collection",
        "timestamp": utc_string(created, leaps)?,
        "entry": entries
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ops::tests::crew;

    pub(crate) fn leaps() -> LeapSeconds {
        LeapSeconds::bundled().expect("bundled")
    }

    /// TAI for a UTC civil time (after 2017: TAI − UTC = 37 s).
    pub(crate) fn tai(unix_s: i64) -> TaiNs {
        leaps().utc_to_tai(unix_s * NS_PER_S).expect("in range").0
    }

    pub(crate) fn obs(parameter: Parameter, value: ExportValue) -> ExportObservation {
        ExportObservation {
            id: Uuid::from_u128(0x1234),
            crew: crew(),
            parameter,
            value,
            // 2026-10-10T12:00:00Z
            at: tai(1_791_633_600),
            device: Some(DeviceId::from_uuid(Uuid::from_u128(0xD1))),
            quality: Quality::Good,
            classification: ExportClassification::Measured,
        }
    }

    #[test]
    fn utc_strings_include_leap_seconds() {
        let l = leaps();
        assert_eq!(
            utc_string(tai(1_791_633_600), &l).expect("ok"),
            "2026-10-10T12:00:00.000Z"
        );
        // 2016-12-31T23:59:59Z, then the leap second.
        let before = tai(1_483_228_799);
        assert_eq!(
            utc_string(before, &l).expect("ok"),
            "2016-12-31T23:59:59.000Z"
        );
        assert_eq!(
            utc_string(TaiNs(before.0 + NS_PER_S + 250_000_000), &l).expect("ok"),
            "2016-12-31T23:59:60.250Z"
        );
    }

    /// F-GND-09: LOINC from the catalog, UCUM units, vital-signs category, device reference.
    #[test]
    fn f_gnd_09_observation_uses_catalog_codes() {
        let r = observation(
            &obs(Parameter::HeartRate, ExportValue::Quantity(64.0)),
            &leaps(),
        )
        .expect("exports");
        assert_eq!(r["code"]["coding"][0]["system"], LOINC);
        assert_eq!(r["code"]["coding"][0]["code"], "8867-4");
        assert_eq!(r["valueQuantity"]["code"], "/min");
        assert_eq!(r["valueQuantity"]["system"], UCUM);
        assert_eq!(r["category"][0]["coding"][0]["code"], "vital-signs");
        assert!(r["device"]["reference"]
            .as_str()
            .is_some_and(|s| s.starts_with("urn:uuid:")));
    }

    /// F-GND-09: a parameter without a verified LOINC code uses the local code system only.
    #[test]
    fn f_gnd_09_no_loinc_means_local_code() {
        let p = caelum_domain::parameters::CATALOG
            .iter()
            .find(|s| s.loinc.is_none() && matches!(s.value, ValueType::Numeric { .. }))
            .expect("a parameter without LOINC")
            .parameter;
        let r = observation(&obs(p, ExportValue::Quantity(1.0)), &leaps()).expect("exports");
        assert_eq!(r["code"]["coding"][0]["system"], LOCAL_PARAMETER_SYSTEM);
        assert_eq!(r["code"]["coding"][0]["code"], p.spec().key);
    }

    #[test]
    fn invalid_values_are_refused() {
        let l = leaps();
        assert!(observation(
            &obs(Parameter::HeartRate, ExportValue::Quantity(f64::NAN)),
            &l
        )
        .is_err());
        assert!(observation(&obs(Parameter::HeartRate, ExportValue::State(0)), &l).is_err());
        assert!(observation(&obs(Parameter::SleepStage, ExportValue::State(9)), &l).is_err());
        assert!(observation(&obs(Parameter::SpeState, ExportValue::State(0)), &l).is_err());
        let ok = observation(&obs(Parameter::SleepStage, ExportValue::State(3)), &l).expect("ok");
        assert_eq!(ok["valueCodeableConcept"]["coding"][0]["code"], "rem");
    }

    /// F-SYS-08: simulated data cannot become an export classification.
    #[test]
    fn f_sys_08_only_real_classifications_export() {
        assert_eq!(
            ExportClassification::try_from(Classification::Measured),
            Ok(ExportClassification::Measured)
        );
        assert_eq!(
            ExportClassification::try_from(Classification::Imported),
            Ok(ExportClassification::Imported)
        );
    }

    /// F-GND-09: the bundle holds one Patient per crew member and every resource once.
    #[test]
    fn f_gnd_09_bundle_collects_resources() {
        let l = leaps();
        let o = obs(Parameter::Spo2, ExportValue::Quantity(88.0));
        let a = ExportAlarm {
            id: AlarmId::from_uuid(Uuid::from_u128(0xA1)),
            crew: crew(),
            priority: OpsPriority::Critical,
            category: OpsCategory::Respiratory,
            rule_id: "spo2.critical".into(),
            ruleset_version: "0.3.0".into(),
            created_at: o.at,
            observations: vec![o.id],
        };
        let d = ExportDevice {
            id: DeviceId::from_uuid(Uuid::from_u128(0xD1)),
            manufacturer: None,
            model: Some("PO-1".into()),
            serial: None,
        };
        let b = bundle(
            Uuid::from_u128(9),
            &[o.clone(), o],
            &[d],
            &[a],
            tai(1_791_633_600),
            &l,
        )
        .expect("bundle");
        let types: Vec<&str> = b["entry"]
            .as_array()
            .expect("entries")
            .iter()
            .filter_map(|e| e["resource"]["resourceType"].as_str())
            .collect();
        assert_eq!(
            types,
            [
                "Patient",
                "Device",
                "Observation",
                "Observation",
                "DiagnosticReport"
            ]
        );
        assert_eq!(b["type"], "collection");
        let dev = &b["entry"][1]["resource"];
        assert!(
            dev.get("manufacturer").is_none(),
            "unknown fields stay absent"
        );
        assert_eq!(
            b["entry"][4]["resource"]["result"][0]["reference"],
            "urn:uuid:00000000-0000-0000-0000-000000001234"
        );
    }
}
