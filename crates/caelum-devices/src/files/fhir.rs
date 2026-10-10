//! HL7 FHIR R4 import of Observation, Device and DiagnosticReport resources (F-ACQ-08).
//!
//! Input is a single resource or a Bundle of any type (JSON). Each resource is imported or
//! rejected on its own, with an explicit reason; one bad resource never hides the others.
//!
//! Observations are mapped to catalog parameters **only** by the LOINC codes in the parameter
//! catalog (`caelum-domain::parameters`, CLAUDE.md rule 7):
//! - codes the catalog does not list are reported and ignored, never guessed. LOINC 2708-6
//!   (oxygen saturation in arterial blood) is not the catalog's SpO2 code (59408-5) and is not
//!   mapped to `spo2`;
//! - a code the catalog uses for more than one parameter (39106-0, skin temperature, proximal and
//!   distal) is ambiguous and rejected;
//! - `core_temp` (8329-5) is derived only (docs/05 §1) and never imported;
//! - a value without a UCUM unit is rejected (R-CLN-02); comparators (`<`, `>=`, …) are rejected
//!   because the value is not a measurement;
//! - the effective time must be a full date-time with a UTC offset; partial dates and periods are
//!   rejected (no assumed time of day or time zone);
//! - only `final`, `amended` and `corrected` results are imported;
//! - components (for example the blood-pressure panel 85354-9 with 8480-6 and 8462-4) are
//!   imported one by one under the parent's time, subject and status;
//! - resources tagged with the HL7 `HTEST` (test health data) code carry the SR-017 test flag, so
//!   ingest rejects them in release builds.
//!
//! Plausibility and unit conversion to the catalog unit are left to ingest, which applies the
//! same checks to every source (docs/03 §3).

use base64::Engine as _;
use caelum_domain::ids::DeviceId;
use caelum_domain::observation::MeasurementMode;
use caelum_domain::observation::Quality;
use caelum_domain::parameters::{Parameter, CATALOG};
use caelum_domain::time::{LeapSeconds, TaiNs, Validity};
use caelum_domain::units::Unit;
use caelum_ingest::{DeviceFlags, RawReading, RawValue};
use serde_json::{Map, Value};

use super::{CivilDateTime, CivilTimeError};

/// LOINC code system URI.
pub const LOINC: &str = "http://loinc.org";
/// UCUM code system URI.
pub const UCUM: &str = "http://unitsofmeasure.org";
/// HL7 v3 ActReason code system (holds `HTEST`).
pub const ACT_REASON: &str = "http://terminology.hl7.org/CodeSystem/v3-ActReason";

/// Observation statuses that are imported.
pub const ACCEPTED_OBSERVATION_STATUS: [&str; 3] = ["final", "amended", "corrected"];
/// DiagnosticReport statuses that are imported.
pub const ACCEPTED_REPORT_STATUS: [&str; 4] = ["final", "amended", "corrected", "appended"];

/// The result of an import.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FhirImport {
    /// Observation values mapped to catalog parameters.
    pub observations: Vec<ImportedObservation>,
    /// Device resources.
    pub devices: Vec<ImportedDevice>,
    /// Diagnostic reports.
    pub reports: Vec<ImportedReport>,
    /// Everything not imported, with the reason.
    pub rejected: Vec<RejectedResource>,
}

/// One observation value mapped to a catalog parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedObservation {
    /// Where it was found (`entry[2]`, `entry[2].component[0]`, `contained[1]`, …).
    pub location: String,
    /// `Observation.id`.
    pub resource_id: Option<String>,
    /// The catalog parameter its LOINC code maps to.
    pub parameter: Parameter,
    /// The LOINC code that mapped.
    pub loinc: String,
    /// Other codes on the observation that the catalog does not map (`system|code`), for audit.
    pub unmapped_codes: Vec<String>,
    /// The value as stated.
    pub value: f64,
    /// The stated UCUM unit.
    pub unit: Unit,
    /// Effective time as TAI.
    pub effective: TaiNs,
    /// Whether the TAI conversion was inside the leap-second table.
    pub effective_validity: Validity,
    /// Effective time as written.
    pub effective_text: String,
    /// `Observation.status`.
    pub status: String,
    /// `subject.reference`, if any (the caller maps it to a crew member).
    pub subject: Option<String>,
    /// `device.reference`, if any.
    pub device: Option<String>,
    /// SR-017 flags (`HTEST` security label or tag).
    pub flags: DeviceFlags,
}

impl ImportedObservation {
    /// The reading for ingest. `device_time` carries the effective time; the caller stores
    /// imported values with classification `Imported` and the effective time as measured time
    /// (docs/integration/M8-files.md). Quality is `Degraded`: FHIR states no signal quality, and
    /// nothing is assumed `Good`.
    pub fn to_raw_reading(&self, device_id: DeviceId, received_at: TaiNs) -> RawReading {
        RawReading {
            device_id,
            parameter: self.parameter,
            value: Some(RawValue::Number(self.value)),
            unit: Some(self.unit),
            device_time: Some(self.effective),
            received_at,
            quality: Quality::Degraded,
            mode: MeasurementMode::Spot,
            flags: self.flags,
            site: None,
        }
    }
}

/// A Device resource.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImportedDevice {
    /// Where it was found.
    pub location: String,
    /// `Device.id`.
    pub resource_id: Option<String>,
    /// Identifiers (`system|value`).
    pub identifiers: Vec<String>,
    /// UDI device identifiers.
    pub udi: Vec<String>,
    /// Manufacturer.
    pub manufacturer: Option<String>,
    /// Model number.
    pub model_number: Option<String>,
    /// Serial number.
    pub serial_number: Option<String>,
    /// Device names.
    pub names: Vec<String>,
    /// Versions (`type: value`, or the value alone), e.g. firmware.
    pub versions: Vec<String>,
    /// Type codes (`system|code`).
    pub type_codes: Vec<String>,
    /// Status, if stated.
    pub status: Option<String>,
}

/// A DiagnosticReport resource.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedReport {
    /// Where it was found.
    pub location: String,
    /// `DiagnosticReport.id`.
    pub resource_id: Option<String>,
    /// Status.
    pub status: String,
    /// Report codes (`system|code`).
    pub codes: Vec<String>,
    /// Code text or first display.
    pub title: Option<String>,
    /// Effective time, if stated (a full date-time with offset).
    pub effective: Option<(TaiNs, Validity)>,
    /// `issued`, as written.
    pub issued: Option<String>,
    /// `subject.reference`.
    pub subject: Option<String>,
    /// `result[].reference` (Observations, imported separately).
    pub results: Vec<String>,
    /// Conclusion text.
    pub conclusion: Option<String>,
    /// Attached documents (`presentedForm`).
    pub attachments: Vec<ImportedAttachment>,
    /// SR-017 flags.
    pub flags: DeviceFlags,
}

/// An attachment carried inline or by URL (URLs are not fetched: offline first).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedAttachment {
    /// MIME type (required).
    pub content_type: String,
    /// Title, if any.
    pub title: Option<String>,
    /// Decoded inline data.
    pub data: Option<Vec<u8>>,
    /// URL, if the data is not inline.
    pub url: Option<String>,
}

/// A resource or value that was not imported.
#[derive(Debug, Clone, PartialEq)]
pub struct RejectedResource {
    /// Where it was found.
    pub location: String,
    /// `resourceType`, if present.
    pub resource_type: Option<String>,
    /// `id`, if present.
    pub resource_id: Option<String>,
    /// Why.
    pub reason: FhirRejection,
}

/// Why a resource or value was not imported.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FhirRejection {
    /// Only Observation, Device and DiagnosticReport are imported (Bundles are unpacked).
    #[error("resource type {0:?} is not imported")]
    UnsupportedResourceType(String),
    /// The resource has no `resourceType`.
    #[error("no resourceType")]
    NoResourceType,
    /// A required element is missing or has the wrong JSON type.
    #[error("malformed: {0}")]
    Malformed(&'static str),
    /// The status is not one that is imported.
    #[error("status {0:?} is not imported")]
    Status(String),
    /// No LOINC coding.
    #[error("no LOINC code (codes: {0:?})")]
    NoLoincCode(Vec<String>),
    /// LOINC codes that the parameter catalog does not map.
    #[error("LOINC code(s) {0:?} not in the parameter catalog")]
    UnknownLoincCode(Vec<String>),
    /// The codes map to more than one parameter.
    #[error("codes map to more than one parameter: {0:?}")]
    AmbiguousCode(Vec<Parameter>),
    /// `core_temp` is derived only and never imported (docs/05 §1).
    #[error("{0:?} is derived only and cannot be imported")]
    DerivedOnly(Parameter),
    /// A grouping observation without its own value (its members are imported separately).
    #[error("grouping observation without a value")]
    GroupingOnly,
    /// No value (with the dataAbsentReason code, if given).
    #[error("no value (data absent reason: {0:?})")]
    MissingValue(Option<String>),
    /// The value is not a Quantity.
    #[error("value type {0} is not supported")]
    UnsupportedValueType(String),
    /// A comparator (`<`, `<=`, `>=`, `>`, `ad`) marks the value as not exact.
    #[error("value has comparator {0:?}")]
    Comparator(String),
    /// The Quantity has no unit at all.
    #[error("no unit")]
    MissingUnit,
    /// The Quantity's unit is not a UCUM code.
    #[error("unit is not a UCUM code (system {system:?}, unit {unit:?})")]
    UnitNotUcum {
        /// `Quantity.system`.
        system: Option<String>,
        /// `Quantity.unit` or `code`.
        unit: Option<String>,
    },
    /// A UCUM code CAELUM has no unit for.
    #[error("UCUM unit {0:?} is not supported")]
    UnknownUnit(String),
    /// No effective time.
    #[error("no effective time")]
    MissingEffectiveTime,
    /// The effective time is not a full date-time with offset.
    #[error("effective time {0:?} is not a full date-time with UTC offset")]
    ImpreciseTime(String),
    /// The effective time is malformed or outside the leap-second table.
    #[error("effective time {0:?} is invalid")]
    InvalidTime(String),
    /// `effectivePeriod` / `effectiveTiming` are not imported.
    #[error("effective[x] type {0} is not supported")]
    UnsupportedEffective(String),
    /// An attachment without a content type or with invalid base64 data.
    #[error("attachment {0}: {1}")]
    Attachment(usize, &'static str),
    /// A resource that is marked entered-in-error.
    #[error("resource is entered-in-error")]
    EnteredInError,
}

/// Errors that stop the whole import.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FhirError {
    /// Not valid JSON.
    #[error("not valid JSON: {0}")]
    Json(String),
    /// The top-level JSON is not an object.
    #[error("top-level JSON is not a FHIR resource")]
    NotAResource,
}

/// Imports a FHIR R4 JSON resource or Bundle (F-ACQ-08).
pub fn import(json: &[u8], leap: &LeapSeconds) -> Result<FhirImport, FhirError> {
    let root: Value = serde_json::from_slice(json).map_err(|e| FhirError::Json(e.to_string()))?;
    let root = root.as_object().ok_or(FhirError::NotAResource)?;
    let mut out = FhirImport::default();
    if root.get("resourceType").and_then(Value::as_str) == Some("Bundle") {
        let entries = root.get("entry").and_then(Value::as_array);
        for (i, entry) in entries.into_iter().flatten().enumerate() {
            let location = format!("entry[{i}]");
            match entry.get("resource").and_then(Value::as_object) {
                Some(r) => import_resource(r, &location, leap, &mut out),
                None => out.rejected.push(RejectedResource {
                    location,
                    resource_type: None,
                    resource_id: None,
                    reason: FhirRejection::Malformed("bundle entry without a resource"),
                }),
            }
        }
    } else {
        import_resource(root, "", leap, &mut out);
    }
    Ok(out)
}

fn import_resource(
    r: &Map<String, Value>,
    location: &str,
    leap: &LeapSeconds,
    out: &mut FhirImport,
) {
    let rtype = r.get("resourceType").and_then(Value::as_str);
    let id = str_at(r, "id");
    let reject = |out: &mut FhirImport, reason| {
        out.rejected.push(RejectedResource {
            location: location.to_owned(),
            resource_type: rtype.map(str::to_owned),
            resource_id: id.clone(),
            reason,
        })
    };
    // Contained Observations and Devices are imported like any other resource.
    if let Some(contained) = r.get("contained").and_then(Value::as_array) {
        for (j, c) in contained.iter().enumerate() {
            let loc = join(location, &format!("contained[{j}]"));
            match c.as_object() {
                Some(c) => import_resource(c, &loc, leap, out),
                None => out.rejected.push(RejectedResource {
                    location: loc,
                    resource_type: None,
                    resource_id: None,
                    reason: FhirRejection::Malformed("contained resource is not an object"),
                }),
            }
        }
    }
    match rtype {
        Some("Observation") => observation(r, location, id.clone(), leap, out),
        Some("Device") => match device(r, location) {
            Ok(d) => out.devices.push(d),
            Err(reason) => reject(out, reason),
        },
        Some("DiagnosticReport") => match report(r, location, leap) {
            Ok(d) => out.reports.push(d),
            Err(reason) => reject(out, reason),
        },
        Some(other) => reject(
            out,
            FhirRejection::UnsupportedResourceType(other.to_owned()),
        ),
        None => reject(out, FhirRejection::NoResourceType),
    }
}

fn join(base: &str, part: &str) -> String {
    if base.is_empty() {
        part.to_owned()
    } else {
        format!("{base}.{part}")
    }
}

fn str_at(o: &Map<String, Value>, key: &str) -> Option<String> {
    o.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn reference(o: &Map<String, Value>, key: &str) -> Option<String> {
    o.get(key)
        .and_then(Value::as_object)
        .and_then(|r| str_at(r, "reference"))
}

/// `system|code` of every coding in a CodeableConcept.
fn codings(concept: Option<&Value>) -> Vec<(Option<String>, String)> {
    concept
        .and_then(|c| c.get("coding"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let code = c.get("code").and_then(Value::as_str)?;
            Some((
                c.get("system").and_then(Value::as_str).map(str::to_owned),
                code.to_owned(),
            ))
        })
        .collect()
}

fn coding_text((system, code): &(Option<String>, String)) -> String {
    format!("{}|{code}", system.as_deref().unwrap_or(""))
}

/// SR-017: the HL7 `HTEST` code in `meta.security` or `meta.tag`.
fn test_flags(r: &Map<String, Value>) -> DeviceFlags {
    let meta = r.get("meta");
    let htest = ["security", "tag"].iter().any(|k| {
        meta.and_then(|m| m.get(*k))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|c| {
                c.get("code").and_then(Value::as_str) == Some("HTEST")
                    && c.get("system").and_then(Value::as_str) == Some(ACT_REASON)
            })
    });
    DeviceFlags {
        demonstration: false,
        test_data: htest,
    }
}

struct Context {
    status: String,
    effective: (TaiNs, Validity),
    effective_text: String,
    subject: Option<String>,
    device: Option<String>,
    flags: DeviceFlags,
}

fn observation(
    r: &Map<String, Value>,
    location: &str,
    id: Option<String>,
    leap: &LeapSeconds,
    out: &mut FhirImport,
) {
    let reject = |out: &mut FhirImport, location: String, reason| {
        out.rejected.push(RejectedResource {
            location,
            resource_type: Some("Observation".to_owned()),
            resource_id: id.clone(),
            reason,
        })
    };
    let status = match r.get("status").and_then(Value::as_str) {
        Some(s) if ACCEPTED_OBSERVATION_STATUS.contains(&s) => s.to_owned(),
        Some("entered-in-error") => {
            return reject(out, location.to_owned(), FhirRejection::EnteredInError)
        }
        Some(s) => {
            return reject(
                out,
                location.to_owned(),
                FhirRejection::Status(s.to_owned()),
            )
        }
        None => {
            return reject(
                out,
                location.to_owned(),
                FhirRejection::Malformed("no status"),
            )
        }
    };
    let (effective, effective_text) = match effective_time(r, leap) {
        Ok(Some(t)) => t,
        Ok(None) => {
            return reject(
                out,
                location.to_owned(),
                FhirRejection::MissingEffectiveTime,
            )
        }
        Err(reason) => return reject(out, location.to_owned(), reason),
    };
    let ctx = Context {
        status,
        effective,
        effective_text,
        subject: reference(r, "subject"),
        device: reference(r, "device"),
        flags: test_flags(r),
    };

    let components = r.get("component").and_then(Value::as_array);
    let has_value = r.keys().any(|k| k.starts_with("value"));
    let has_components = components.is_some_and(|c| !c.is_empty());
    if has_value || !has_components {
        if !has_value && !has_components && r.get("hasMember").is_some() {
            return reject(out, location.to_owned(), FhirRejection::GroupingOnly);
        }
        match value(r, &ctx) {
            Ok(o) => out
                .observations
                .push(finish(o, location.to_owned(), id.clone())),
            Err(reason) => reject(out, location.to_owned(), reason),
        }
    }
    for (j, c) in components.into_iter().flatten().enumerate() {
        let loc = join(location, &format!("component[{j}]"));
        match c
            .as_object()
            .ok_or(FhirRejection::Malformed("component"))
            .and_then(|c| value(c, &ctx))
        {
            Ok(o) => out.observations.push(finish(o, loc, id.clone())),
            Err(reason) => reject(out, loc, reason),
        }
    }
}

fn finish(mut o: ImportedObservation, location: String, id: Option<String>) -> ImportedObservation {
    o.location = location;
    o.resource_id = id;
    o
}

/// Maps one code + value (an Observation or one of its components).
fn value(o: &Map<String, Value>, ctx: &Context) -> Result<ImportedObservation, FhirRejection> {
    let codes = codings(o.get("code"));
    let loinc: Vec<&str> = codes
        .iter()
        .filter(|(s, _)| s.as_deref() == Some(LOINC))
        .map(|(_, c)| c.as_str())
        .collect();
    if loinc.is_empty() {
        return Err(FhirRejection::NoLoincCode(
            codes.iter().map(coding_text).collect(),
        ));
    }
    let mut mapped: Vec<(Parameter, &str)> = Vec::new();
    for code in &loinc {
        for spec in CATALOG.iter().filter(|s| s.loinc == Some(*code)) {
            if !mapped.iter().any(|(p, _)| *p == spec.parameter) {
                mapped.push((spec.parameter, code));
            }
        }
    }
    let (parameter, code) = match mapped.as_slice() {
        [] => {
            return Err(FhirRejection::UnknownLoincCode(
                loinc.iter().map(|c| (*c).to_owned()).collect(),
            ))
        }
        [one] => *one,
        many => {
            return Err(FhirRejection::AmbiguousCode(
                many.iter().map(|(p, _)| *p).collect(),
            ))
        }
    };
    if parameter == Parameter::CoreTemp {
        return Err(FhirRejection::DerivedOnly(parameter));
    }

    let (value, unit) = quantity(o)?;
    let unmapped_codes = codes
        .iter()
        .filter(|(s, c)| !(s.as_deref() == Some(LOINC) && c == code))
        .map(coding_text)
        .collect();
    Ok(ImportedObservation {
        location: String::new(),
        resource_id: None,
        parameter,
        loinc: code.to_owned(),
        unmapped_codes,
        value,
        unit,
        effective: ctx.effective.0,
        effective_validity: ctx.effective.1,
        effective_text: ctx.effective_text.clone(),
        status: ctx.status.clone(),
        subject: ctx.subject.clone(),
        device: ctx.device.clone(),
        flags: ctx.flags,
    })
}

fn quantity(o: &Map<String, Value>) -> Result<(f64, Unit), FhirRejection> {
    let Some(q) = o.get("valueQuantity") else {
        if let Some(k) = o.keys().find(|k| k.starts_with("value")) {
            return Err(FhirRejection::UnsupportedValueType(k.clone()));
        }
        let reason = codings(o.get("dataAbsentReason"))
            .first()
            .map(|(_, c)| c.clone());
        return Err(FhirRejection::MissingValue(reason));
    };
    let q = q
        .as_object()
        .ok_or(FhirRejection::Malformed("valueQuantity"))?;
    if let Some(c) = q.get("comparator").and_then(Value::as_str) {
        return Err(FhirRejection::Comparator(c.to_owned()));
    }
    let v = q
        .get("value")
        .and_then(Value::as_f64)
        .ok_or(FhirRejection::MissingValue(None))?;
    let system = str_at(q, "system");
    let code = str_at(q, "code");
    let unit_text = str_at(q, "unit");
    let code = match (system.as_deref(), code) {
        (Some(UCUM), Some(code)) => code,
        (_, None) if unit_text.is_none() => return Err(FhirRejection::MissingUnit),
        (system, code) => {
            return Err(FhirRejection::UnitNotUcum {
                system: system.map(str::to_owned),
                unit: code.or(unit_text),
            })
        }
    };
    let unit = Unit::from_ucum(&code).ok_or(FhirRejection::UnknownUnit(code))?;
    Ok((v, unit))
}

/// `effectiveDateTime` / `effectiveInstant`; `Ok(None)` when absent.
#[allow(clippy::type_complexity)]
fn effective_time(
    r: &Map<String, Value>,
    leap: &LeapSeconds,
) -> Result<Option<((TaiNs, Validity), String)>, FhirRejection> {
    for key in ["effectiveDateTime", "effectiveInstant"] {
        if let Some(v) = r.get(key) {
            let text = v
                .as_str()
                .ok_or(FhirRejection::Malformed("effective time"))?;
            return parse_instant(text, leap).map(|t| Some((t, text.to_owned())));
        }
    }
    if let Some(k) = r.keys().find(|k| k.starts_with("effective")) {
        return Err(FhirRejection::UnsupportedEffective(k.clone()));
    }
    Ok(None)
}

/// A FHIR `dateTime`/`instant` with time and offset → TAI.
pub fn parse_instant(text: &str, leap: &LeapSeconds) -> Result<(TaiNs, Validity), FhirRejection> {
    let t = parse_datetime(text)?;
    t.to_tai(leap).map_err(|e| match e {
        CivilTimeError::NoUtcOffset => FhirRejection::ImpreciseTime(text.to_owned()),
        _ => FhirRejection::InvalidTime(text.to_owned()),
    })
}

/// `YYYY-MM-DDThh:mm:ss[.f+](Z|±hh:mm)`; partial dates are imprecise, anything else invalid.
pub fn parse_datetime(text: &str) -> Result<CivilDateTime, FhirRejection> {
    let invalid = || FhirRejection::InvalidTime(text.to_owned());
    let b = text.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Result<u32, FhirRejection> {
        let s = text.get(r).ok_or_else(invalid)?;
        if s.bytes().all(|c| c.is_ascii_digit()) {
            s.parse().map_err(|_| invalid())
        } else {
            Err(invalid())
        }
    };
    // Partial dates: YYYY, YYYY-MM, YYYY-MM-DD.
    if matches!(b.len(), 4 | 7 | 10) && !text.contains('T') {
        return Err(FhirRejection::ImpreciseTime(text.to_owned()));
    }
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return Err(invalid());
    }
    let year = digits(0..4)? as i32;
    let month = digits(5..7)? as u8;
    let day = digits(8..10)? as u8;
    let hour = digits(11..13)? as u8;
    let minute = digits(14..16)? as u8;
    let second = digits(17..19)? as u8;
    let mut i = 19;
    let mut nanos = 0u32;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        let mut end = start;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
        let frac = &text[start..end];
        if frac.is_empty() || frac.len() > 9 {
            return Err(invalid());
        }
        nanos = format!("{frac:0<9}").parse().map_err(|_| invalid())?;
        i = end;
    }
    let offset = match &text[i..] {
        "Z" => 0i16,
        tz if tz.len() == 6 && (tz.starts_with('+') || tz.starts_with('-')) && &tz[3..4] == ":" => {
            let h = digits(i + 1..i + 3)? as i16;
            let m = digits(i + 4..i + 6)? as i16;
            let o = h * 60 + m;
            if tz.starts_with('-') {
                -o
            } else {
                o
            }
        }
        _ => return Err(invalid()),
    };
    CivilDateTime::new(year, month, day, hour, minute, second, nanos, Some(offset))
        .map_err(|_| invalid())
}

fn device(r: &Map<String, Value>, location: &str) -> Result<ImportedDevice, FhirRejection> {
    let status = str_at(r, "status");
    if status.as_deref() == Some("entered-in-error") {
        return Err(FhirRejection::EnteredInError);
    }
    let arr = |key: &str| {
        r.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let identifiers = arr("identifier")
        .iter()
        .filter_map(|i| {
            let v = i.get("value").and_then(Value::as_str)?;
            let s = i.get("system").and_then(Value::as_str).unwrap_or("");
            Some(format!("{s}|{v}"))
        })
        .collect();
    let udi = arr("udiCarrier")
        .iter()
        .filter_map(|u| {
            u.get("deviceIdentifier")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    let names = arr("deviceName")
        .iter()
        .filter_map(|n| n.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let versions = arr("version")
        .iter()
        .filter_map(|v| {
            let value = v.get("value").and_then(Value::as_str)?;
            let kind = v.get("type").and_then(|t| {
                t.get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| codings(Some(t)).first().map(|(_, c)| c.clone()))
            });
            Some(match kind {
                Some(k) => format!("{k}: {value}"),
                None => value.to_owned(),
            })
        })
        .collect();
    Ok(ImportedDevice {
        location: location.to_owned(),
        resource_id: str_at(r, "id"),
        identifiers,
        udi,
        manufacturer: str_at(r, "manufacturer"),
        model_number: str_at(r, "modelNumber"),
        serial_number: str_at(r, "serialNumber"),
        names,
        versions,
        type_codes: codings(r.get("type")).iter().map(coding_text).collect(),
        status,
    })
}

fn report(
    r: &Map<String, Value>,
    location: &str,
    leap: &LeapSeconds,
) -> Result<ImportedReport, FhirRejection> {
    let status = match r.get("status").and_then(Value::as_str) {
        Some(s) if ACCEPTED_REPORT_STATUS.contains(&s) => s.to_owned(),
        Some("entered-in-error") => return Err(FhirRejection::EnteredInError),
        Some(s) => return Err(FhirRejection::Status(s.to_owned())),
        None => return Err(FhirRejection::Malformed("no status")),
    };
    let code = r.get("code");
    let codes: Vec<String> = codings(code).iter().map(coding_text).collect();
    if codes.is_empty() {
        return Err(FhirRejection::Malformed("no report code"));
    }
    let title = code
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            code.and_then(|c| c.get("coding"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find_map(|c| c.get("display").and_then(Value::as_str).map(str::to_owned))
        });
    let effective = effective_time(r, leap)?.map(|(t, _)| t);
    let results = r
        .get("result")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|x| {
            x.get("reference")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    let mut attachments = Vec::new();
    for (i, a) in r
        .get("presentedForm")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let content_type = a
            .get("contentType")
            .and_then(Value::as_str)
            .ok_or(FhirRejection::Attachment(i, "no contentType"))?;
        let data = match a.get("data").and_then(Value::as_str) {
            Some(d) => Some(
                base64::engine::general_purpose::STANDARD
                    .decode(d)
                    .map_err(|_| FhirRejection::Attachment(i, "data is not valid base64"))?,
            ),
            None => None,
        };
        let url = a.get("url").and_then(Value::as_str).map(str::to_owned);
        if data.is_none() && url.is_none() {
            return Err(FhirRejection::Attachment(i, "neither data nor url"));
        }
        attachments.push(ImportedAttachment {
            content_type: content_type.to_owned(),
            title: a.get("title").and_then(Value::as_str).map(str::to_owned),
            data,
            url,
        });
    }
    Ok(ImportedReport {
        location: location.to_owned(),
        resource_id: str_at(r, "id"),
        status,
        codes,
        title,
        effective,
        issued: str_at(r, "issued"),
        subject: reference(r, "subject"),
        results,
        conclusion: str_at(r, "conclusion"),
        attachments,
        flags: test_flags(r),
    })
}
