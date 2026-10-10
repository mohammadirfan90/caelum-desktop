//! Minimal HL7 FHIR R4 model for what CAELUM writes and reads (ICD-MDS §5, docs/04 §3a,
//! F-GND-12).
//!
//! Writes: `Observation` (LOINC from the parameter catalog only; a parameter without a verified
//! LOINC code uses the local system `urn:caelum:parameter` with its catalog key; units are UCUM
//! from the catalog), `DiagnosticReport` (alarm events) and `Device`. Every written resource
//! carries the idempotent CAELUM client id as an identifier with system `urn:caelum:client-id`
//! (conditional create, ICD-MDS §4).
//!
//! Reads: [`parse_observation`] turns a read-back `Observation` into [`InboundObservation`]s. The
//! parameter is resolved only by a catalog LOINC code or the local code; the unit must be present
//! as a UCUM code and must be the catalog unit or convertible to it by
//! [`caelum_domain::units::convert`]. Anything else is rejected with a [`Rejection`], never
//! guessed (R-CLN-02, SR-018). LOINC 2708-6 (arterial oxygen saturation) is not in the catalog
//! and therefore never maps to SpO2.

use caelum_domain::ids::{AlarmId, CrewMemberId, DeviceId};
use caelum_domain::parameters::{Category, Parameter, ValueType, CATALOG};
use caelum_domain::time::{LeapSeconds, TaiNs, Validity};
use caelum_domain::units::{convert, Substance, Unit};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::ids::ClientId;
use crate::instant::{format_instant, parse_instant, InstantError};

/// LOINC code system.
pub const LOINC_SYSTEM: &str = "http://loinc.org";
/// UCUM code system.
pub const UCUM_SYSTEM: &str = "http://unitsofmeasure.org";
/// Identifier system of the CAELUM client id (ICD-MDS §4).
pub const CLIENT_ID_SYSTEM: &str = "urn:caelum:client-id";
/// Local code system for parameters without a verified LOINC code (value: catalog key).
pub const PARAMETER_SYSTEM: &str = "urn:caelum:parameter";
/// Identifier system for crew members (Observation.subject).
pub const CREW_SYSTEM: &str = "urn:caelum:crew-member";
/// Identifier system for CAELUM device ids.
pub const DEVICE_SYSTEM: &str = "urn:caelum:device";
/// Identifier system for CAELUM alarm ids.
pub const ALARM_SYSTEM: &str = "urn:caelum:alarm";
/// Local code system for CAELUM report types.
pub const REPORT_SYSTEM: &str = "urn:caelum:report";
/// Local code system for alarm rule ids (version = ruleset version).
pub const ALARM_RULE_SYSTEM: &str = "urn:caelum:alarm-rule";
/// Local code system for alarm priorities.
pub const ALARM_PRIORITY_SYSTEM: &str = "urn:caelum:alarm-priority";
/// Local code system for alarm states.
pub const ALARM_STATE_SYSTEM: &str = "urn:caelum:alarm-state";
const CATEGORY_SYSTEM: &str = "http://terminology.hl7.org/CodeSystem/observation-category";

/// Errors building an outbound resource. No resource is produced with an invented value.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FhirError {
    /// The numeric value is NaN or infinite.
    #[error("value of {0} is not finite")]
    NotFinite(&'static str),
    /// A quantity was given for an enumerated parameter or a state for a numeric one.
    #[error("value type does not match parameter {0}")]
    ValueTypeMismatch(&'static str),
    /// The state is not one of the parameter's catalog states (or they are still pending).
    #[error("state {state:?} is not defined for {parameter}")]
    UnknownState {
        /// Catalog key.
        parameter: &'static str,
        /// The offending state.
        state: String,
    },
    /// The period ends before it starts.
    #[error("effective period ends before it starts")]
    ReversedPeriod,
    /// A timestamp could not be rendered.
    #[error(transparent)]
    Time(#[from] InstantError),
}

/// When an observation applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effective {
    /// A single reading.
    At(TaiNs),
    /// A summary over an interval.
    Period {
        /// Start of the interval.
        start: TaiNs,
        /// End of the interval.
        end: TaiNs,
    },
}

/// An observation value in the catalog's canonical unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ObservedValue {
    /// A number in the parameter's catalog unit.
    Quantity(f64),
    /// One of the parameter's catalog states.
    State(String),
}

/// A CAELUM observation to write (summary or alarm-related value).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservationRecord {
    /// The parameter.
    pub parameter: Parameter,
    /// The value, in the catalog unit.
    pub value: ObservedValue,
    /// When it applies.
    pub effective: Effective,
    /// The crew member.
    pub crew: CrewMemberId,
    /// The device it came from, if any.
    pub device: Option<DeviceId>,
}

fn coding(system: &str, code: &str) -> Value {
    json!({ "system": system, "code": code })
}

fn identifier(system: &str, value: &str) -> Value {
    json!({ "system": system, "value": value })
}

fn client_identifier(id: ClientId) -> Value {
    identifier(CLIENT_ID_SYSTEM, &id.to_string())
}

fn logical_ref(resource_type: &str, system: &str, value: &str) -> Value {
    json!({ "type": resource_type, "identifier": identifier(system, value) })
}

/// The FHIR `Observation.code` of a parameter: catalog LOINC, else the local catalog key.
pub fn parameter_code(parameter: Parameter) -> Value {
    let spec = parameter.spec();
    let c = match spec.loinc {
        Some(loinc) => coding(LOINC_SYSTEM, loinc),
        None => coding(PARAMETER_SYSTEM, spec.key),
    };
    json!({ "coding": [c], "text": spec.label })
}

/// Builds an `Observation` with the CAELUM client id (F-GND-12).
pub fn observation(
    rec: &ObservationRecord,
    id: ClientId,
    leap: &LeapSeconds,
) -> Result<Value, FhirError> {
    let spec = rec.parameter.spec();
    let mut o = Map::new();
    o.insert("resourceType".into(), "Observation".into());
    o.insert("identifier".into(), json!([client_identifier(id)]));
    o.insert("status".into(), "final".into());
    if spec.category == Category::VitalSigns {
        o.insert(
            "category".into(),
            json!([{ "coding": [coding(CATEGORY_SYSTEM, "vital-signs")] }]),
        );
    }
    o.insert("code".into(), parameter_code(rec.parameter));
    o.insert(
        "subject".into(),
        logical_ref("Patient", CREW_SYSTEM, &rec.crew.to_string()),
    );
    match rec.effective {
        Effective::At(t) => {
            o.insert("effectiveDateTime".into(), format_instant(t, leap)?.into());
        }
        Effective::Period { start, end } => {
            if end < start {
                return Err(FhirError::ReversedPeriod);
            }
            o.insert(
                "effectivePeriod".into(),
                json!({ "start": format_instant(start, leap)?, "end": format_instant(end, leap)? }),
            );
        }
    }
    match (&rec.value, spec.value) {
        (ObservedValue::Quantity(v), ValueType::Numeric { .. }) => {
            if !v.is_finite() {
                return Err(FhirError::NotFinite(spec.key));
            }
            let ucum = spec.unit.ucum();
            o.insert(
                "valueQuantity".into(),
                json!({ "value": v, "unit": ucum, "system": UCUM_SYSTEM, "code": ucum }),
            );
        }
        (ObservedValue::State(s), ValueType::Enum(states)) => {
            if !states.contains(&s.as_str()) {
                return Err(FhirError::UnknownState {
                    parameter: spec.key,
                    state: s.clone(),
                });
            }
            let system = format!("{PARAMETER_SYSTEM}:{}", spec.key);
            o.insert(
                "valueCodeableConcept".into(),
                json!({ "coding": [coding(&system, s)], "text": s }),
            );
        }
        (ObservedValue::State(s), ValueType::EnumPending(_)) => {
            return Err(FhirError::UnknownState {
                parameter: spec.key,
                state: s.clone(),
            })
        }
        _ => return Err(FhirError::ValueTypeMismatch(spec.key)),
    }
    if let Some(d) = rec.device {
        o.insert(
            "device".into(),
            logical_ref("Device", DEVICE_SYSTEM, &d.to_string()),
        );
    }
    Ok(Value::Object(o))
}

/// An alarm event to write as a `DiagnosticReport` (full detail; MEDICAL channel only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlarmEventRecord {
    /// The alarm.
    pub alarm: AlarmId,
    /// The crew member.
    pub crew: CrewMemberId,
    /// When the alarm condition began.
    pub raised_at: TaiNs,
    /// When this event record was issued (raise, escalation, acknowledgement, clearance).
    pub issued_at: TaiNs,
    /// Rule id from the signed ruleset.
    pub rule_id: String,
    /// `RULESET_VERSION` of the rule.
    pub ruleset_version: String,
    /// Alarm priority as named by the alarm engine.
    pub priority: String,
    /// Alarm state as named by the alarm engine.
    pub state: String,
    /// Human-readable conclusion.
    pub conclusion: String,
    /// Client ids of the alarm-related observations.
    pub results: Vec<ClientId>,
}

/// Builds a `DiagnosticReport` for an alarm event (F-GND-12).
pub fn alarm_report(
    rec: &AlarmEventRecord,
    id: ClientId,
    leap: &LeapSeconds,
) -> Result<Value, FhirError> {
    let results: Vec<Value> = rec
        .results
        .iter()
        .map(|r| logical_ref("Observation", CLIENT_ID_SYSTEM, &r.to_string()))
        .collect();
    let mut rule = coding(ALARM_RULE_SYSTEM, &rec.rule_id);
    if let Some(m) = rule.as_object_mut() {
        m.insert("version".into(), rec.ruleset_version.clone().into());
    }
    Ok(json!({
        "resourceType": "DiagnosticReport",
        "identifier": [client_identifier(id), identifier(ALARM_SYSTEM, &rec.alarm.to_string())],
        "status": "final",
        "code": { "coding": [coding(REPORT_SYSTEM, "alarm-event")], "text": "CAELUM alarm event" },
        "subject": logical_ref("Patient", CREW_SYSTEM, &rec.crew.to_string()),
        "effectiveDateTime": format_instant(rec.raised_at, leap)?,
        "issued": format_instant(rec.issued_at, leap)?,
        "result": results,
        "conclusion": rec.conclusion,
        "conclusionCode": [
            { "coding": [rule] },
            { "coding": [coding(ALARM_PRIORITY_SYSTEM, &rec.priority)] },
            { "coding": [coding(ALARM_STATE_SYSTEM, &rec.state)] },
        ],
    }))
}

/// A device to write as a FHIR `Device`. Unknown fields stay absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    /// CAELUM device id.
    pub device: DeviceId,
    /// Manufacturer, if known.
    pub manufacturer: Option<String>,
    /// Model, if known.
    pub model: Option<String>,
    /// Serial number, if known.
    pub serial_number: Option<String>,
}

/// Builds a `Device` (F-GND-12).
pub fn device(rec: &DeviceRecord, id: ClientId) -> Value {
    let mut o = Map::new();
    o.insert("resourceType".into(), "Device".into());
    o.insert(
        "identifier".into(),
        json!([
            client_identifier(id),
            identifier(DEVICE_SYSTEM, &rec.device.to_string())
        ]),
    );
    for (k, v) in [
        ("manufacturer", &rec.manufacturer),
        ("modelNumber", &rec.model),
        ("serialNumber", &rec.serial_number),
    ] {
        if let Some(v) = v {
            o.insert(k.into(), v.clone().into());
        }
    }
    Value::Object(o)
}

/// The CAELUM client id in a resource's identifiers, if CAELUM wrote it.
pub fn caelum_client_id(resource: &Value) -> Option<ClientId> {
    resource["identifier"]
        .as_array()?
        .iter()
        .find(|i| i["system"] == CLIENT_ID_SYSTEM)?["value"]
        .as_str()?
        .parse()
        .ok()
}

/// Stable identity of a read-back resource for de-duplication (SR-016): `Type/id` when the server
/// gave an id, else the first identifier as `system|value`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(pub String);

/// The source id of a resource, if it has one.
pub fn source_id(resource: &Value) -> Option<SourceId> {
    let rt = resource["resourceType"].as_str()?;
    if let Some(id) = resource["id"].as_str().filter(|s| !s.is_empty()) {
        return Some(SourceId(format!("{rt}/{id}")));
    }
    let first = resource["identifier"].as_array()?.first()?;
    let (sys, val) = (first["system"].as_str()?, first["value"].as_str()?);
    Some(SourceId(format!("{rt}|{sys}|{val}")))
}

/// Version marker of a read-back resource (`meta.versionId`, else `meta.lastUpdated`).
pub fn version_marker(resource: &Value) -> Option<String> {
    let meta = &resource["meta"];
    meta["versionId"]
        .as_str()
        .or_else(|| meta["lastUpdated"].as_str())
        .map(str::to_owned)
}

/// A reading from the medical data system, resolved against the catalog. The core must pass it
/// through the normal ingest validation (plausibility, staleness) and alarm evaluation exactly
/// like direct device data, with classification `Imported` (SR-016).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboundObservation {
    /// De-duplication key (`Observation/<id>`, with `#component[n]` for components).
    pub source_id: SourceId,
    /// The catalog parameter.
    pub parameter: Parameter,
    /// The value, converted to the catalog unit.
    pub value: f64,
    /// The catalog unit (`value` is in it).
    pub unit: Unit,
    /// The unit as received, before conversion.
    pub reported_unit: Unit,
    /// When the reading was taken.
    pub effective: TaiNs,
    /// Whether `effective` is covered by the leap-second table.
    pub time_validity: Validity,
    /// The crew member, when the subject carries a CAELUM crew identifier.
    pub crew: Option<CrewMemberId>,
    /// The subject reference as received (`reference` or `system|value`), for mapping by the core.
    pub subject: Option<String>,
    /// The device reference as received, for cross-path de-duplication by the core (SR-016).
    pub device: Option<String>,
}

/// Why a read-back observation was not accepted. Every rejection is reported, never dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum Rejection {
    /// Not an `Observation`.
    #[error("not an Observation")]
    NotAnObservation,
    /// No id or identifier to de-duplicate by.
    #[error("no source id")]
    NoSourceId,
    /// Status is not final, amended or corrected.
    #[error("status {0:?} not accepted")]
    Status(String),
    /// No code resolves to a catalog parameter (codes listed as `system|code`).
    #[error("no catalog parameter for codes {0:?}")]
    UnknownCode(Vec<String>),
    /// Codes resolve to more than one parameter.
    #[error("codes resolve to several parameters")]
    AmbiguousCode,
    /// The parameter is not numeric in the catalog.
    #[error("parameter {0} is not numeric")]
    NotNumeric(String),
    /// `dataAbsentReason` instead of a value.
    #[error("data absent")]
    DataAbsent,
    /// No `valueQuantity` with a number.
    #[error("no numeric value")]
    NoValue,
    /// The value is not finite.
    #[error("value not finite")]
    NotFinite,
    /// No UCUM unit code.
    #[error("no UCUM unit")]
    NoUnit,
    /// The UCUM code is not one CAELUM knows.
    #[error("unknown UCUM unit {0:?}")]
    UnknownUnit(String),
    /// No defined conversion to the catalog unit.
    #[error("unit {from} not convertible to {to}")]
    UnitNotConvertible {
        /// Received UCUM code.
        from: String,
        /// Catalog UCUM code.
        to: String,
    },
    /// No `effectiveDateTime` / `effectiveInstant` (periods are not point readings).
    #[error("no effective time")]
    NoEffectiveTime,
    /// The effective time is imprecise or malformed.
    #[error("bad time: {0}")]
    BadTime(String),
}

fn resolve_parameter(code: &Value) -> Result<Parameter, Rejection> {
    let mut found: Vec<Parameter> = Vec::new();
    let mut seen_codes = Vec::new();
    for c in code["coding"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let (Some(sys), Some(code)) = (c["system"].as_str(), c["code"].as_str()) else {
            continue;
        };
        seen_codes.push(format!("{sys}|{code}"));
        let hits: Vec<Parameter> = match sys {
            LOINC_SYSTEM => CATALOG
                .iter()
                .filter(|s| s.loinc == Some(code))
                .map(|s| s.parameter)
                .collect(),
            PARAMETER_SYSTEM => Parameter::from_key(code).into_iter().collect(),
            _ => Vec::new(),
        };
        if hits.len() > 1 {
            return Err(Rejection::AmbiguousCode);
        }
        found.extend(hits);
    }
    found.sort();
    found.dedup();
    match found.as_slice() {
        [p] => Ok(*p),
        [] => Err(Rejection::UnknownCode(seen_codes)),
        _ => Err(Rejection::AmbiguousCode),
    }
}

fn reference_text(r: &Value) -> Option<String> {
    if let Some(s) = r["reference"].as_str() {
        return Some(s.to_owned());
    }
    let i = &r["identifier"];
    Some(format!(
        "{}|{}",
        i["system"].as_str()?,
        i["value"].as_str()?
    ))
}

struct Context {
    effective: TaiNs,
    time_validity: Validity,
    crew: Option<CrewMemberId>,
    subject: Option<String>,
    device: Option<String>,
}

fn parse_value(
    source_id: SourceId,
    holder: &Value,
    ctx: &Context,
) -> Result<InboundObservation, Rejection> {
    let parameter = resolve_parameter(&holder["code"])?;
    let spec = parameter.spec();
    if !matches!(spec.value, ValueType::Numeric { .. }) {
        return Err(Rejection::NotNumeric(spec.key.to_owned()));
    }
    if !holder["dataAbsentReason"].is_null() {
        return Err(Rejection::DataAbsent);
    }
    let q = &holder["valueQuantity"];
    let value = q["value"].as_f64().ok_or(Rejection::NoValue)?;
    if !value.is_finite() {
        return Err(Rejection::NotFinite);
    }
    let code = match (q["system"].as_str(), q["code"].as_str()) {
        (Some(UCUM_SYSTEM), Some(c)) => c,
        _ => return Err(Rejection::NoUnit),
    };
    let reported_unit = Unit::from_ucum(code).ok_or_else(|| Rejection::UnknownUnit(code.into()))?;
    let substance = if parameter == Parameter::Glucose {
        Substance::Glucose
    } else {
        Substance::Other
    };
    let value = convert(value, reported_unit, spec.unit, substance).map_err(|_| {
        Rejection::UnitNotConvertible {
            from: code.into(),
            to: spec.unit.ucum().into(),
        }
    })?;
    Ok(InboundObservation {
        source_id,
        parameter,
        value,
        unit: spec.unit,
        reported_unit,
        effective: ctx.effective,
        time_validity: ctx.time_validity,
        crew: ctx.crew,
        subject: ctx.subject.clone(),
        device: ctx.device.clone(),
    })
}

/// Parses a read-back `Observation` (SR-016). A resource with a top-level value yields one result;
/// one without a value but with `component`s (e.g. a blood-pressure panel) yields one result per
/// component, with source id `<resource>#component[n]`. A resource-level problem yields `Err`.
pub fn parse_observation(
    resource: &Value,
    leap: &LeapSeconds,
) -> Result<Vec<Result<InboundObservation, Rejection>>, Rejection> {
    if resource["resourceType"] != "Observation" {
        return Err(Rejection::NotAnObservation);
    }
    let source = source_id(resource).ok_or(Rejection::NoSourceId)?;
    let status = resource["status"].as_str().unwrap_or("");
    if !matches!(status, "final" | "amended" | "corrected") {
        return Err(Rejection::Status(status.to_owned()));
    }
    let time = resource["effectiveDateTime"]
        .as_str()
        .or_else(|| resource["effectiveInstant"].as_str())
        .ok_or(Rejection::NoEffectiveTime)?;
    let (effective, time_validity) =
        parse_instant(time, leap).map_err(|e| Rejection::BadTime(e.to_string()))?;
    let subject = &resource["subject"];
    let crew = (subject["identifier"]["system"] == CREW_SYSTEM)
        .then(|| subject["identifier"]["value"].as_str()?.parse().ok())
        .flatten();
    let ctx = Context {
        effective,
        time_validity,
        crew,
        subject: reference_text(subject),
        device: reference_text(&resource["device"]),
    };
    let components = resource["component"].as_array();
    let has_value = !resource["valueQuantity"].is_null() || !resource["dataAbsentReason"].is_null();
    match components {
        Some(cs) if !has_value && !cs.is_empty() => Ok(cs
            .iter()
            .enumerate()
            .map(|(i, c)| parse_value(SourceId(format!("{}#component[{i}]", source.0)), c, &ctx))
            .collect()),
        _ => Ok(vec![parse_value(source, resource, &ctx)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ResourceKind;
    use crate::instant::tai_from_unix_s;

    fn leap() -> LeapSeconds {
        LeapSeconds::bundled().expect("bundled table")
    }

    fn t0(l: &LeapSeconds) -> TaiNs {
        tai_from_unix_s(1_791_633_600, l).expect("2026-10-10")
    }

    fn crew() -> CrewMemberId {
        "67e55044-10b1-426f-9247-bb680e5fe0c8"
            .parse()
            .expect("uuid")
    }

    fn rec(parameter: Parameter, value: ObservedValue, l: &LeapSeconds) -> ObservationRecord {
        ObservationRecord {
            parameter,
            value,
            effective: Effective::At(t0(l)),
            crew: crew(),
            device: None,
        }
    }

    fn mds_obs(code: Value, quantity: Value) -> Value {
        json!({
            "resourceType": "Observation", "id": "42", "meta": { "versionId": "1" },
            "status": "final", "code": code, "valueQuantity": quantity,
            "effectiveDateTime": "2026-10-10T12:00:00Z",
            "subject": { "reference": "Patient/7" },
            "device": { "reference": "Device/pulse-ox-1" },
        })
    }

    fn loinc(code: &str) -> Value {
        json!({ "coding": [{ "system": LOINC_SYSTEM, "code": code }] })
    }

    fn ucum(value: f64, code: &str) -> Value {
        json!({ "value": value, "system": UCUM_SYSTEM, "code": code })
    }

    fn single(v: &Value, l: &LeapSeconds) -> Result<InboundObservation, Rejection> {
        let mut out = parse_observation(v, l)?;
        assert_eq!(out.len(), 1);
        out.remove(0)
    }

    /// F-GND-12: Observation uses catalog LOINC + UCUM and carries the CAELUM client id.
    #[test]
    fn f_gnd_12_observation_has_loinc_ucum_and_client_id() {
        let l = leap();
        let id = ClientId::derive(ResourceKind::Observation, "hr-1");
        let o = observation(
            &rec(Parameter::HeartRate, ObservedValue::Quantity(72.0), &l),
            id,
            &l,
        )
        .expect("builds");
        assert_eq!(o["code"]["coding"][0]["system"], LOINC_SYSTEM);
        assert_eq!(o["code"]["coding"][0]["code"], "8867-4");
        assert_eq!(o["valueQuantity"]["code"], "/min");
        assert_eq!(o["valueQuantity"]["system"], UCUM_SYSTEM);
        assert_eq!(o["category"][0]["coding"][0]["code"], "vital-signs");
        assert_eq!(o["effectiveDateTime"], "2026-10-10T12:00:00.000Z");
        assert_eq!(caelum_client_id(&o), Some(id));
        assert_eq!(o["subject"]["identifier"]["system"], CREW_SYSTEM);
    }

    /// A parameter without a verified LOINC code uses the local code system with the catalog key.
    #[test]
    fn parameters_without_loinc_use_the_local_code() {
        let l = leap();
        let p = CATALOG
            .iter()
            .find(|s| s.loinc.is_none() && matches!(s.value, ValueType::Numeric { .. }))
            .expect("catalog has a numeric parameter without LOINC")
            .parameter;
        let o = observation(
            &rec(p, ObservedValue::Quantity(1.0), &l),
            ClientId::derive(ResourceKind::Observation, "x"),
            &l,
        )
        .expect("builds");
        assert_eq!(o["code"]["coding"][0]["system"], PARAMETER_SYSTEM);
        assert_eq!(o["code"]["coding"][0]["code"], p.spec().key);
        assert!(o.get("category").is_none() || p.spec().category == Category::VitalSigns);
    }

    /// No fabricated data: invalid values and periods are errors, not resources.
    #[test]
    fn invalid_outbound_values_are_errors() {
        let l = leap();
        let id = ClientId::derive(ResourceKind::Observation, "bad");
        let nan = rec(Parameter::HeartRate, ObservedValue::Quantity(f64::NAN), &l);
        assert_eq!(
            observation(&nan, id, &l),
            Err(FhirError::NotFinite("heart_rate"))
        );
        let state = rec(Parameter::HeartRate, ObservedValue::State("x".into()), &l);
        assert_eq!(
            observation(&state, id, &l),
            Err(FhirError::ValueTypeMismatch("heart_rate"))
        );
        let mut rev = rec(Parameter::HeartRate, ObservedValue::Quantity(60.0), &l);
        rev.effective = Effective::Period {
            start: TaiNs(t0(&l).0 + 1),
            end: t0(&l),
        };
        assert_eq!(observation(&rev, id, &l), Err(FhirError::ReversedPeriod));
        let pending = CATALOG
            .iter()
            .find(|s| matches!(s.value, ValueType::EnumPending(_)))
            .expect("catalog has pending states")
            .parameter;
        let p = rec(pending, ObservedValue::State("any".into()), &l);
        assert!(matches!(
            observation(&p, id, &l),
            Err(FhirError::UnknownState { .. })
        ));
        let enumerated = CATALOG
            .iter()
            .find(|s| matches!(s.value, ValueType::Enum(_)))
            .expect("catalog has an enum");
        let ValueType::Enum(states) = enumerated.value else {
            unreachable!()
        };
        let ok = rec(
            enumerated.parameter,
            ObservedValue::State(states[0].into()),
            &l,
        );
        let o = observation(&ok, id, &l).expect("valid state");
        assert_eq!(o["valueCodeableConcept"]["coding"][0]["code"], states[0]);
        let bad = rec(
            enumerated.parameter,
            ObservedValue::State("invented".into()),
            &l,
        );
        assert!(matches!(
            observation(&bad, id, &l),
            Err(FhirError::UnknownState { .. })
        ));
        let mut period = rec(Parameter::HeartRate, ObservedValue::Quantity(60.0), &l);
        period.effective = Effective::Period {
            start: t0(&l),
            end: TaiNs(t0(&l).0 + 60_000_000_000),
        };
        period.device = Some(
            "67e55044-10b1-426f-9247-bb680e5fe0c9"
                .parse()
                .expect("uuid"),
        );
        let o = observation(&period, id, &l).expect("period");
        assert_eq!(o["effectivePeriod"]["end"], "2026-10-10T12:01:00.000Z");
        assert_eq!(o["device"]["identifier"]["system"], DEVICE_SYSTEM);
    }

    /// F-GND-12: alarm events become DiagnosticReports linking their observations by client id.
    #[test]
    fn f_gnd_12_alarm_report_and_device() {
        let l = leap();
        let obs = ClientId::derive(ResourceKind::Observation, "spo2-1");
        let id = ClientId::derive(ResourceKind::DiagnosticReport, "alarm-1/raised");
        let r = alarm_report(
            &AlarmEventRecord {
                alarm: "67e55044-10b1-426f-9247-bb680e5fe0c1"
                    .parse()
                    .expect("uuid"),
                crew: crew(),
                raised_at: t0(&l),
                issued_at: t0(&l),
                rule_id: "SPO2_LOW".into(),
                ruleset_version: "1.2.0".into(),
                priority: "HIGH".into(),
                state: "ACTIVE".into(),
                conclusion: "SpO2 below limit".into(),
                results: vec![obs],
            },
            id,
            &l,
        )
        .expect("builds");
        assert_eq!(r["resourceType"], "DiagnosticReport");
        assert_eq!(caelum_client_id(&r), Some(id));
        assert_eq!(r["result"][0]["identifier"]["value"], obs.to_string());
        assert_eq!(r["conclusionCode"][0]["coding"][0]["version"], "1.2.0");

        let did = ClientId::derive(ResourceKind::Device, "dev-1");
        let d = device(
            &DeviceRecord {
                device: "67e55044-10b1-426f-9247-bb680e5fe0c2"
                    .parse()
                    .expect("uuid"),
                manufacturer: Some("Acme".into()),
                model: None,
                serial_number: None,
            },
            did,
        );
        assert_eq!(caelum_client_id(&d), Some(did));
        assert_eq!(d["manufacturer"], "Acme");
        assert!(d.get("modelNumber").is_none(), "unknown fields stay absent");
    }

    /// SR-016: a read-back observation resolves by catalog LOINC and converts units.
    #[test]
    fn sr_016_inbound_resolves_and_converts() {
        let l = leap();
        let v = single(&mds_obs(loinc("8310-5"), ucum(98.6, "[degF]")), &l).expect("accepted");
        assert_eq!(v.parameter, Parameter::BodyTempMeasured);
        assert!((v.value - 37.0).abs() < 1e-9);
        assert_eq!((v.unit, v.reported_unit), (Unit::Celsius, Unit::Fahrenheit));
        assert_eq!(v.source_id, SourceId("Observation/42".into()));
        assert_eq!(v.device.as_deref(), Some("Device/pulse-ox-1"));
        assert_eq!(v.subject.as_deref(), Some("Patient/7"));
        assert_eq!(v.crew, None);
        let local = json!({ "coding": [{ "system": PARAMETER_SYSTEM, "code": "heart_rate" }] });
        assert_eq!(
            single(&mds_obs(local, ucum(60.0, "/min")), &l)
                .expect("ok")
                .parameter,
            Parameter::HeartRate
        );
        // Glucose is the only parameter with a mass/molar conversion (catalog unit mg/dL).
        let glucose = single(&mds_obs(loinc("41653-7"), ucum(5.0, "mmol/L")), &l).expect("glucose");
        assert_eq!(glucose.unit, Unit::MgPerDl);
        assert_eq!(glucose.reported_unit, Unit::MmolPerL);
        assert!((glucose.value - 90.078).abs() < 1e-9);
    }

    /// SR-016, R-CLN-02: LOINC 2708-6 (arterial SaO2) never maps to SpO2.
    #[test]
    fn sr_016_loinc_2708_6_is_not_spo2() {
        let l = leap();
        assert!(matches!(
            single(&mds_obs(loinc("2708-6"), ucum(97.0, "%")), &l),
            Err(Rejection::UnknownCode(_))
        ));
        assert_eq!(
            single(&mds_obs(loinc("59408-5"), ucum(97.0, "%")), &l)
                .expect("ok")
                .parameter,
            Parameter::Spo2
        );
    }

    /// SR-016, R-CLN-02: missing, unknown or unconvertible units and other defects are rejected.
    #[test]
    fn sr_016_inbound_rejections_never_guess() {
        let l = leap();
        let hr = || loinc("8867-4");
        let case = |v: Value| single(&v, &l).expect_err("rejected");
        assert_eq!(
            case(mds_obs(hr(), json!({ "value": 60.0, "unit": "/min" }))),
            Rejection::NoUnit
        );
        assert_eq!(
            case(mds_obs(hr(), ucum(60.0, "furlong"))),
            Rejection::UnknownUnit("furlong".into())
        );
        assert!(matches!(
            case(mds_obs(hr(), ucum(60.0, "Cel"))),
            Rejection::UnitNotConvertible { .. }
        ));
        assert_eq!(
            case(mds_obs(
                hr(),
                json!({ "system": UCUM_SYSTEM, "code": "/min" })
            )),
            Rejection::NoValue
        );
        // A LOINC code shared by two catalog parameters (skin temperature sites) is ambiguous.
        let skin = loinc("39106-0");
        assert_eq!(
            case(mds_obs(skin, ucum(33.0, "Cel"))),
            Rejection::AmbiguousCode
        );
        let both = json!({ "coding": [
            { "system": LOINC_SYSTEM, "code": "8867-4" },
            { "system": PARAMETER_SYSTEM, "code": "pulse_rate" } ] });
        assert_eq!(
            case(mds_obs(both, ucum(60.0, "/min"))),
            Rejection::AmbiguousCode
        );
        let mut absent = mds_obs(hr(), Value::Null);
        absent["dataAbsentReason"] = json!({ "text": "sensor off" });
        assert_eq!(case(absent), Rejection::DataAbsent);
        let enumerated = CATALOG
            .iter()
            .find(|s| matches!(s.value, ValueType::Enum(_)))
            .expect("enum");
        let code = json!({ "coding": [{ "system": PARAMETER_SYSTEM, "code": enumerated.key }] });
        assert!(matches!(
            case(mds_obs(code, ucum(1.0, "1"))),
            Rejection::NotNumeric(_)
        ));

        let resource_level = |v: Value| parse_observation(&v, &l).expect_err("rejected");
        let mut prelim = mds_obs(hr(), ucum(60.0, "/min"));
        prelim["status"] = "preliminary".into();
        assert_eq!(
            resource_level(prelim),
            Rejection::Status("preliminary".into())
        );
        let mut no_time = mds_obs(hr(), ucum(60.0, "/min"));
        no_time
            .as_object_mut()
            .expect("object")
            .remove("effectiveDateTime");
        assert_eq!(resource_level(no_time), Rejection::NoEffectiveTime);
        let mut date_only = mds_obs(hr(), ucum(60.0, "/min"));
        date_only["effectiveDateTime"] = "2026-10-10".into();
        assert!(matches!(resource_level(date_only), Rejection::BadTime(_)));
        let mut anon = mds_obs(hr(), ucum(60.0, "/min"));
        anon.as_object_mut().expect("object").remove("id");
        assert_eq!(resource_level(anon), Rejection::NoSourceId);
        assert_eq!(
            resource_level(json!({ "resourceType": "Patient" })),
            Rejection::NotAnObservation
        );
    }

    /// SR-016: panel components are separate readings with their own source ids.
    #[test]
    fn sr_016_components_are_parsed_individually() {
        let l = leap();
        let mut bp = mds_obs(loinc("85354-9"), Value::Null);
        bp.as_object_mut().expect("object").remove("valueQuantity");
        bp["subject"] =
            json!({ "identifier": { "system": CREW_SYSTEM, "value": crew().to_string() } });
        bp["component"] = json!([
            { "code": loinc("8480-6"), "valueQuantity": ucum(16.0, "kPa") },
            { "code": loinc("8462-4"), "valueQuantity": ucum(80.0, "mm[Hg]") },
            { "code": loinc("9999-9"), "valueQuantity": ucum(1.0, "1") },
        ]);
        let out = parse_observation(&bp, &l).expect("resource accepted");
        let sys = out[0].as_ref().expect("systolic");
        assert_eq!(sys.parameter, Parameter::BpSystolic);
        assert!((sys.value - 120.009).abs() < 1e-3);
        assert_eq!(sys.source_id.0, "Observation/42#component[0]");
        assert_eq!(sys.crew, Some(crew()));
        assert_eq!(
            out[1].as_ref().expect("diastolic").parameter,
            Parameter::BpDiastolic
        );
        assert!(matches!(out[2], Err(Rejection::UnknownCode(_))));
    }

    #[test]
    fn source_ids_and_versions() {
        let with_id =
            json!({ "resourceType": "Observation", "id": "1", "meta": { "lastUpdated": "x" } });
        assert_eq!(source_id(&with_id), Some(SourceId("Observation/1".into())));
        assert_eq!(version_marker(&with_id).as_deref(), Some("x"));
        let by_ident = json!({ "resourceType": "Observation", "identifier": [{ "system": "s", "value": "v" }] });
        assert_eq!(
            source_id(&by_ident),
            Some(SourceId("Observation|s|v".into()))
        );
        assert_eq!(version_marker(&by_ident), None);
        assert_eq!(caelum_client_id(&by_ident), None);
    }
}
