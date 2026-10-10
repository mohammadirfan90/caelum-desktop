//! De-identified research export (F-SYS-08, docs/04 §3 RESEARCH channel, R-SEC-06).
//!
//! - Only consented crew members are exported; others are left out and counted.
//! - Only parameters whose catalog entry allows the RESEARCH channel.
//! - Subjects are pseudonymised with HMAC-SHA256 under a secret export key: stable within one
//!   key, unlinkable without it. Record and device IDs, device details and free text are
//!   dropped.
//! - Times are shifted by a per-subject whole number of days derived from the key (intervals
//!   within a subject are preserved; calendar dates are not real).
//!
//! Output is CSV: `subject,parameter,unit,value,time,quality`.

use std::collections::BTreeSet;

use caelum_domain::ids::CrewMemberId;
use caelum_domain::parameters::{Channel, ValueType};
use caelum_domain::time::{LeapSeconds, TaiNs};
use sha2::{Digest, Sha256};

use crate::fhir::{utc_string, ExportError, ExportObservation, ExportValue};
use crate::uplink::hex;

const DAY_NS: i64 = 86_400 * 1_000_000_000;

/// HMAC-SHA256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Research export errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResearchError {
    /// The export key or shift range is unusable.
    #[error("invalid research export settings: {0}")]
    Settings(String),
    /// A value could not be exported.
    #[error(transparent)]
    Export(#[from] ExportError),
}

/// The result of an export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchExport {
    /// CSV text.
    pub csv: String,
    /// Rows written.
    pub rows: usize,
    /// Observations left out: no consent.
    pub excluded_no_consent: usize,
    /// Observations left out: parameter not allowed on RESEARCH.
    pub excluded_parameter: usize,
}

/// The exporter.
pub struct ResearchExporter {
    key: [u8; 32],
    max_shift_days: u16,
}

impl ResearchExporter {
    /// An exporter with a secret key (kept by the export operator, not in the export) and the
    /// date-shift range ±`max_shift_days` (at least 1).
    pub fn new(key: [u8; 32], max_shift_days: u16) -> Result<Self, ResearchError> {
        if key.iter().all(|b| *b == 0) {
            return Err(ResearchError::Settings(
                "export key must be secret, not zero".into(),
            ));
        }
        if max_shift_days == 0 {
            return Err(ResearchError::Settings(
                "date shift range must be at least one day".into(),
            ));
        }
        Ok(Self {
            key,
            max_shift_days,
        })
    }

    /// The pseudonym of a crew member.
    pub fn pseudonym(&self, crew: CrewMemberId) -> String {
        let mut msg = b"subject".to_vec();
        msg.extend(crew.as_uuid().as_bytes());
        format!("S-{}", &hex(&hmac_sha256(&self.key, &msg))[..16])
    }

    /// The date shift of a crew member, in days (non-zero, within the range).
    pub fn shift_days(&self, crew: CrewMemberId) -> i64 {
        let mut msg = b"shift".to_vec();
        msg.extend(crew.as_uuid().as_bytes());
        let h = hmac_sha256(&self.key, &msg);
        let max = i64::from(self.max_shift_days);
        let r = i64::from(u32::from_be_bytes([h[0], h[1], h[2], h[3]])) % (2 * max);
        // Maps 0..2max onto −max..−1 and 1..max.
        if r < max {
            r - max
        } else {
            r - max + 1
        }
    }

    /// Exports observations of consented crew members.
    pub fn export(
        &self,
        observations: &[ExportObservation],
        consented: &BTreeSet<CrewMemberId>,
        leaps: &LeapSeconds,
    ) -> Result<ResearchExport, ResearchError> {
        let mut out = ResearchExport {
            csv: "subject,parameter,unit,value,time,quality\n".into(),
            rows: 0,
            excluded_no_consent: 0,
            excluded_parameter: 0,
        };
        for o in observations {
            if !consented.contains(&o.crew) {
                out.excluded_no_consent += 1;
                continue;
            }
            let spec = o.parameter.spec();
            if !spec.channels.contains(&Channel::Research) {
                out.excluded_parameter += 1;
                continue;
            }
            let value = match (spec.value, o.value) {
                (ValueType::Numeric { .. }, ExportValue::Quantity(v)) if v.is_finite() => {
                    v.to_string()
                }
                (ValueType::Enum(states), ExportValue::State(i)) => states
                    .get(usize::from(i))
                    .map(|s| (*s).to_owned())
                    .ok_or(ExportError::InvalidValue {
                        key: spec.key,
                        reason: "state index out of range".into(),
                    })?,
                _ => {
                    return Err(ExportError::InvalidValue {
                        key: spec.key,
                        reason: "value does not fit the parameter".into(),
                    }
                    .into())
                }
            };
            let shifted = TaiNs(o.at.0 + self.shift_days(o.crew) * DAY_NS);
            let quality = match o.quality {
                caelum_domain::observation::Quality::Good => "good",
                caelum_domain::observation::Quality::Degraded => "degraded",
                caelum_domain::observation::Quality::Poor => "poor",
                caelum_domain::observation::Quality::Unusable => "unusable",
            };
            out.csv.push_str(&format!(
                "{},{},{},{},{},{}\n",
                self.pseudonym(o.crew),
                spec.key,
                spec.unit.ucum(),
                value,
                utc_string(shifted, leaps)?,
                quality
            ));
            out.rows += 1;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fhir::tests::{leaps, obs};
    use caelum_domain::parameters::Parameter;
    use uuid::Uuid;

    fn exporter() -> ResearchExporter {
        ResearchExporter::new([7; 32], 180).expect("valid")
    }

    /// RFC 4231 test case 2.
    #[test]
    fn hmac_matches_rfc_4231() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    /// F-SYS-08: pseudonyms are stable, differ per subject and per key, and reveal no ID.
    #[test]
    fn f_sys_08_pseudonyms_are_keyed_and_stable() {
        let e = exporter();
        let a = obs(Parameter::HeartRate, ExportValue::Quantity(60.0)).crew;
        let b = CrewMemberId::from_uuid(Uuid::from_u128(77));
        assert_eq!(e.pseudonym(a), e.pseudonym(a));
        assert_ne!(e.pseudonym(a), e.pseudonym(b));
        let other = ResearchExporter::new([8; 32], 180).expect("valid");
        assert_ne!(e.pseudonym(a), other.pseudonym(a));
        assert!(!e.pseudonym(a).contains(&a.to_string()[..8]));
        for c in 0..200u128 {
            let s = e.shift_days(CrewMemberId::from_uuid(Uuid::from_u128(c)));
            assert!(s != 0 && s.abs() <= 180, "{s}");
        }
    }

    /// F-SYS-08: consent, channel allow-list, date shift and identifier removal.
    #[test]
    fn f_sys_08_export_is_de_identified() {
        let e = exporter();
        let l = leaps();
        let hr = obs(Parameter::HeartRate, ExportValue::Quantity(61.5));
        let crew = hr.crew;
        let stranger = ExportObservation {
            crew: CrewMemberId::from_uuid(Uuid::from_u128(99)),
            ..hr.clone()
        };
        let medical_only = caelum_domain::parameters::CATALOG
            .iter()
            .find(|s| {
                !s.channels.contains(&Channel::Research)
                    && matches!(s.value, ValueType::Numeric { .. })
            })
            .expect("a medical-only parameter")
            .parameter;
        let mo = obs(medical_only, ExportValue::Quantity(1.0));
        let consented: BTreeSet<CrewMemberId> = [crew].into();
        let out = e
            .export(&[hr.clone(), stranger, mo], &consented, &l)
            .expect("exports");
        assert_eq!(
            (out.rows, out.excluded_no_consent, out.excluded_parameter),
            (1, 1, 1)
        );
        assert!(out.csv.contains(&e.pseudonym(crew)));
        assert!(!out.csv.contains(&crew.to_string()));
        assert!(!out.csv.contains(&hr.id.to_string()));
        assert!(
            !out.csv.contains("2026-10-10"),
            "dates are shifted:\n{}",
            out.csv
        );
        assert!(out.csv.contains("heart_rate,/min,61.5,"));
    }

    #[test]
    fn unusable_settings_are_refused() {
        assert!(ResearchExporter::new([0; 32], 10).is_err());
        assert!(ResearchExporter::new([1; 32], 0).is_err());
    }
}
