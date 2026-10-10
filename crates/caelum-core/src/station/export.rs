//! Export (F-SYS-08, F-GND-09): a FHIR R4 bundle of a crew member's observations and devices
//! over a period, and a per-parameter CSV (RFC 4180). Simulated data can never be exported
//! (docs/05 §3): the export type has no simulated variant. Every export is audited.

use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::observation::Quality;
use caelum_domain::parameters::{Parameter, ValueType};
use caelum_domain::time::{LeapSeconds, TaiNs};
use caelum_ground::fhir::{
    self, ExportClassification, ExportDevice, ExportObservation, ExportValue,
};
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::{Command, Role};
use caelum_store::ObservationRecord;
use serde_json::json;

use super::{Station, StationError};

/// Longest export period (30 days).
pub const EXPORT_MAX_S: u32 = 30 * 24 * 3600;
const NS: i64 = 1_000_000_000;

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

fn quality(q: &str) -> Option<Quality> {
    match q {
        "good" => Some(Quality::Good),
        "degraded" => Some(Quality::Degraded),
        "poor" => Some(Quality::Poor),
        "unusable" => Some(Quality::Unusable),
        _ => None,
    }
}

fn classification(c: &str) -> Option<ExportClassification> {
    match c {
        "measured" => Some(ExportClassification::Measured),
        "derived" => Some(ExportClassification::Derived),
        "entered" => Some(ExportClassification::Entered),
        "imported" => Some(ExportClassification::Imported),
        // `simulated` (test equipment) and anything unknown are never exported.
        _ => None,
    }
}

/// One CSV field (RFC 4180): quoted when it holds a comma, quote or line break.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

impl Station {
    fn export_scope(
        &mut self,
        command: Command,
        crew: CrewMemberId,
        window_s: u32,
    ) -> Result<(Actor, i64, i64), StationError> {
        let user = self.user_for(command)?;
        if user.role == Role::Astronaut && user.crew_member_id != Some(crew) {
            return Err(StationError::Denied(caelum_security::role::Denied {
                command,
                reason: caelum_security::role::DeniedReason::Role,
            }));
        }
        if window_s == 0 || window_s > EXPORT_MAX_S {
            return Err(rejected("the export period is 1 s to 30 days"));
        }
        let now = self.now()?;
        Ok((Actor::user(&user), now.0 - i64::from(window_s) * NS, now.0))
    }

    /// `export_fhir`: a FHIR R4 bundle (JSON text) of the crew member's observations and devices
    /// over the last `window_s`. Audited with the counts.
    pub fn export_fhir(
        &mut self,
        crew: CrewMemberId,
        window_s: u32,
    ) -> Result<String, StationError> {
        let (actor, from, to) = self.export_scope(Command::ExportFhir, crew, window_s)?;
        let leaps = LeapSeconds::bundled().map_err(|e| rejected(e.to_string()))?;
        let rows = self
            .db
            .observations_for_crew_between(&crew.to_string(), from, to)?;
        let mut skipped = 0usize;
        let mut observations = Vec::new();
        let mut device_ids: Vec<String> = Vec::new();
        for r in rows {
            match export_observation(&r, crew) {
                Some(o) => {
                    if let Some(d) = &r.device_id {
                        if !device_ids.contains(d) {
                            device_ids.push(d.clone());
                        }
                    }
                    observations.push(o);
                }
                None => skipped += 1,
            }
        }
        let devices: Vec<ExportDevice> = self
            .db
            .devices()?
            .into_iter()
            .filter(|d| device_ids.contains(&d.id))
            .filter_map(|d| {
                Some(ExportDevice {
                    id: d.id.parse::<DeviceId>().ok()?,
                    manufacturer: (d.vendor != "unknown").then_some(d.vendor),
                    model: (d.model != "unknown").then_some(d.model),
                    serial: Some(d.serial),
                })
            })
            .collect();
        let bundle = fhir::bundle(
            uuid::Uuid::new_v4(),
            &observations,
            &devices,
            &[],
            TaiNs(to),
            &leaps,
        )
        .map_err(|e| rejected(e.to_string()))?;
        self.log().append(
            &actor,
            AuditAction::ExportCreated,
            Some(&crew.to_string()),
            &json!({ "format": "fhir-r4", "window_s": window_s, "observations": observations.len(), "not_exported": skipped }),
        )?;
        Ok(bundle.to_string())
    }

    /// `export_csv`: one parameter over the last `window_s` as CSV (UTC times). Audited.
    pub fn export_csv(
        &mut self,
        crew: CrewMemberId,
        parameter: &str,
        window_s: u32,
    ) -> Result<String, StationError> {
        let (actor, from, to) = self.export_scope(Command::ExportCsv, crew, window_s)?;
        let p = Parameter::from_key(parameter)
            .ok_or_else(|| rejected(format!("unknown parameter {parameter}")))?;
        let leaps = LeapSeconds::bundled().map_err(|e| rejected(e.to_string()))?;
        let mut out =
            String::from("measured_at_utc,value,unit,quality,classification,device_id\r\n");
        let mut n = 0usize;
        for r in self
            .db
            .observations_between(&crew.to_string(), p.spec().key, from, to)?
        {
            if classification(&r.classification).is_none() {
                continue;
            }
            let at = fhir::utc_string(TaiNs(r.measured_at_tai), &leaps)
                .map_err(|e| rejected(e.to_string()))?;
            let value = r
                .value
                .map(|v| v.to_string())
                .or(r.value_text.clone())
                .unwrap_or_default();
            let fields = [
                at,
                value,
                r.unit,
                r.quality,
                r.classification,
                r.device_id.unwrap_or_default(),
            ];
            out.push_str(
                &fields
                    .iter()
                    .map(|f| csv_field(f))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            out.push_str("\r\n");
            n += 1;
        }
        self.log().append(
            &actor,
            AuditAction::ExportCreated,
            Some(&crew.to_string()),
            &json!({ "format": "csv", "parameter": parameter, "window_s": window_s, "rows": n }),
        )?;
        Ok(out)
    }
}

fn export_observation(r: &ObservationRecord, crew: CrewMemberId) -> Option<ExportObservation> {
    let parameter = Parameter::from_key(&r.parameter)?;
    let value = match (r.value, r.value_text.as_deref(), parameter.spec().value) {
        (Some(v), _, _) => ExportValue::Quantity(v),
        (None, Some(t), ValueType::Enum(states)) => {
            ExportValue::State(u8::try_from(states.iter().position(|s| *s == t)?).ok()?)
        }
        _ => return None,
    };
    Some(ExportObservation {
        id: uuid::Uuid::parse_str(&r.id).ok()?,
        crew,
        parameter,
        value,
        at: TaiNs(r.measured_at_tai),
        device: r.device_id.as_deref().and_then(|d| d.parse().ok()),
        quality: quality(&r.quality)?,
        classification: classification(&r.classification)?,
    })
}

#[cfg(test)]
mod tests {
    use super::csv_field;

    #[test]
    fn f_sys_08_csv_fields_are_quoted_per_rfc_4180() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
