//! MEDICAL channel parameter summaries as telemetry packets (F-GND-01, F-GND-02, F-GND-14).
//!
//! Summary-first (F-GND-14): one packet per crew member, parameter and summary window. Only
//! parameters whose catalog entry allows the MEDICAL channel can be encoded; parameters whose
//! states are still pending are refused (they are never downlinked).
//!
//! User data (APID `medical_summary`; packet time = window end):
//! crew UUID (16), parameter index in `CATALOG` (1), quality (1), window start CUC T-field (6),
//! then for numeric parameters: min, mean, max (IEEE 754 binary32 each), sample count (u16);
//! for enumerated parameters: the last state index (1) and sample count (u16).

use caelum_domain::ids::CrewMemberId;
use caelum_domain::observation::Quality;
use caelum_domain::parameters::{Channel, Parameter, ValueType, CATALOG};
use caelum_domain::time::TaiNs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::ApidPlan;
use crate::packet::{Cuc, PacketError, TmPacket};

/// Summary errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SummaryError {
    /// The parameter may not go on the MEDICAL channel or has no defined states.
    #[error("parameter `{0}` cannot be downlinked as a summary")]
    NotDownlinked(&'static str),
    /// The value does not fit the parameter's type or is inconsistent.
    #[error("invalid summary: {0}")]
    Invalid(String),
    /// Packet error.
    #[error(transparent)]
    Packet(#[from] PacketError),
}

/// Summary values of one window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryValue {
    /// Numeric parameter, in the catalog unit.
    Numeric {
        /// Minimum.
        min: f32,
        /// Mean.
        mean: f32,
        /// Maximum.
        max: f32,
        /// Number of readings (≥ 1).
        count: u16,
    },
    /// Enumerated parameter.
    State {
        /// Index into the catalog's states.
        last: u8,
        /// Number of readings (≥ 1).
        count: u16,
    },
}

/// A parameter summary for one crew member and window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MedicalSummary {
    /// Crew member.
    pub crew_id: CrewMemberId,
    /// Parameter.
    pub parameter: Parameter,
    /// Worst quality in the window.
    pub quality: Quality,
    /// Window start (TAI).
    pub window_start: TaiNs,
    /// Window end (TAI), the packet time.
    pub window_end: TaiNs,
    /// Values.
    pub value: SummaryValue,
}

/// Wire code of a quality.
pub fn quality_code(q: Quality) -> u8 {
    match q {
        Quality::Good => 1,
        Quality::Degraded => 2,
        Quality::Poor => 3,
        Quality::Unusable => 4,
    }
}

fn quality_from(c: u8) -> Option<Quality> {
    [
        Quality::Good,
        Quality::Degraded,
        Quality::Poor,
        Quality::Unusable,
    ]
    .into_iter()
    .find(|q| quality_code(*q) == c)
}

/// Whether a parameter can be downlinked as a MEDICAL summary.
pub fn downlinked(p: Parameter) -> bool {
    let s = p.spec();
    s.channels.contains(&Channel::Medical) && !matches!(s.value, ValueType::EnumPending(_))
}

impl MedicalSummary {
    /// Checks the summary against the catalog.
    pub fn validate(&self) -> Result<(), SummaryError> {
        let spec = self.parameter.spec();
        if !downlinked(self.parameter) {
            return Err(SummaryError::NotDownlinked(spec.key));
        }
        if self.window_end < self.window_start {
            return Err(SummaryError::Invalid("window ends before it starts".into()));
        }
        match (spec.value, self.value) {
            (
                ValueType::Numeric { .. },
                SummaryValue::Numeric {
                    min,
                    mean,
                    max,
                    count,
                },
            ) => {
                if count == 0 {
                    return Err(SummaryError::Invalid("no readings".into()));
                }
                if !(min.is_finite() && mean.is_finite() && max.is_finite())
                    || !(min <= mean && mean <= max)
                {
                    return Err(SummaryError::Invalid("min ≤ mean ≤ max must hold".into()));
                }
                Ok(())
            }
            (ValueType::Enum(states), SummaryValue::State { last, count }) => {
                if count == 0 {
                    return Err(SummaryError::Invalid("no readings".into()));
                }
                if usize::from(last) >= states.len() {
                    return Err(SummaryError::Invalid(format!("state index {last}")));
                }
                Ok(())
            }
            _ => Err(SummaryError::Invalid(format!(
                "value kind does not match `{}`",
                spec.key
            ))),
        }
    }

    /// User data.
    pub fn user_data(&self) -> Result<Vec<u8>, SummaryError> {
        self.validate()?;
        let index = u8::try_from(self.parameter as usize)
            .map_err(|_| SummaryError::Invalid("parameter index".into()))?;
        let mut b = self.crew_id.as_uuid().as_bytes().to_vec();
        b.push(index);
        b.push(quality_code(self.quality));
        b.extend(Cuc::from_tai(self.window_start)?.t_field());
        match self.value {
            SummaryValue::Numeric {
                min,
                mean,
                max,
                count,
            } => {
                for v in [min, mean, max] {
                    b.extend(v.to_be_bytes());
                }
                b.extend(count.to_be_bytes());
            }
            SummaryValue::State { last, count } => {
                b.push(last);
                b.extend(count.to_be_bytes());
            }
        }
        Ok(b)
    }

    /// Builds the telemetry packet.
    pub fn to_packet(&self, plan: &ApidPlan, seq_count: u16) -> Result<TmPacket, SummaryError> {
        Ok(TmPacket::build(
            plan.medical_summary,
            seq_count,
            self.window_end,
            &self.user_data()?,
        )?)
    }

    /// Decodes user data (ground side).
    pub fn decode(window_end: TaiNs, d: &[u8]) -> Result<Self, SummaryError> {
        let bad = |w: &str| SummaryError::Invalid(format!("malformed summary: {w}"));
        let head = d.get(..24).ok_or_else(|| bad("short"))?;
        let uuid: [u8; 16] = head[..16].try_into().map_err(|_| bad("crew"))?;
        let parameter = CATALOG
            .get(usize::from(head[16]))
            .map(|s| s.parameter)
            .ok_or_else(|| bad("parameter index"))?;
        let quality = quality_from(head[17]).ok_or_else(|| bad("quality"))?;
        let window_start = Cuc::from_t_field(&head[18..24])?.to_tai();
        let rest = &d[24..];
        let f = |i: usize| -> Result<f32, SummaryError> {
            let b: [u8; 4] = rest
                .get(i..i + 4)
                .and_then(|s| s.try_into().ok())
                .ok_or_else(|| bad("value"))?;
            Ok(f32::from_be_bytes(b))
        };
        let value = match parameter.spec().value {
            ValueType::Numeric { .. } => {
                if rest.len() != 14 {
                    return Err(bad("numeric length"));
                }
                SummaryValue::Numeric {
                    min: f(0)?,
                    mean: f(4)?,
                    max: f(8)?,
                    count: u16::from_be_bytes([rest[12], rest[13]]),
                }
            }
            _ => {
                if rest.len() != 3 {
                    return Err(bad("state length"));
                }
                SummaryValue::State {
                    last: rest[0],
                    count: u16::from_be_bytes([rest[1], rest[2]]),
                }
            }
        };
        let s = Self {
            crew_id: CrewMemberId::from_uuid(Uuid::from_bytes(uuid)),
            parameter,
            quality,
            window_start,
            window_end,
            value,
        };
        s.validate()?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::tests::{crew, plan};
    use crate::packet::ParsedTm;

    fn hr() -> MedicalSummary {
        MedicalSummary {
            crew_id: crew(),
            parameter: Parameter::HeartRate,
            quality: Quality::Good,
            window_start: TaiNs(2_000_000_000_000_000_000),
            window_end: TaiNs(2_000_000_060_000_000_000),
            value: SummaryValue::Numeric {
                min: 61.0,
                mean: 64.5,
                max: 70.0,
                count: 60,
            },
        }
    }

    /// F-GND-01, F-GND-14: a summary round-trips through a MEDICAL packet.
    #[test]
    fn f_gnd_14_summary_round_trips() {
        let p = plan();
        let pkt = hr().to_packet(&p, 3).expect("packet");
        let (parsed, _) = ParsedTm::parse(pkt.bytes()).expect("parses");
        assert_eq!(parsed.apid, p.medical_summary);
        let back = MedicalSummary::decode(parsed.time, &parsed.user_data).expect("decodes");
        assert_eq!(back, hr());
    }

    #[test]
    fn enumerated_summary_round_trips() {
        let s = MedicalSummary {
            parameter: Parameter::SleepStage,
            value: SummaryValue::State { last: 3, count: 4 },
            ..hr()
        };
        let back = MedicalSummary::decode(s.window_end, &s.user_data().expect("encodes"))
            .expect("decodes");
        assert_eq!(back, s);
    }

    /// No fabricated data: empty windows, inconsistent values and mismatched kinds are refused.
    #[test]
    fn invalid_summaries_are_refused() {
        let mut s = hr();
        s.value = SummaryValue::Numeric {
            min: 1.0,
            mean: 1.0,
            max: 1.0,
            count: 0,
        };
        assert!(s.validate().is_err());
        s.value = SummaryValue::Numeric {
            min: 2.0,
            mean: 1.0,
            max: 3.0,
            count: 1,
        };
        assert!(s.validate().is_err());
        s.value = SummaryValue::Numeric {
            min: f32::NAN,
            mean: 1.0,
            max: 3.0,
            count: 1,
        };
        assert!(s.validate().is_err());
        s.value = SummaryValue::State { last: 0, count: 1 };
        assert!(s.validate().is_err());
        let sleep = MedicalSummary {
            parameter: Parameter::SleepStage,
            value: SummaryValue::State { last: 5, count: 1 },
            ..hr()
        };
        assert!(sleep.validate().is_err());
    }

    #[test]
    fn pending_state_parameters_are_never_downlinked() {
        let s = MedicalSummary {
            parameter: Parameter::SpeState,
            value: SummaryValue::State { last: 0, count: 1 },
            ..hr()
        };
        assert!(matches!(s.validate(), Err(SummaryError::NotDownlinked(_))));
        assert!(!downlinked(Parameter::SpeState));
        assert!(downlinked(Parameter::HeartRate));
    }

    #[test]
    fn malformed_summary_bytes_are_rejected() {
        let d = hr().user_data().expect("encodes");
        assert!(MedicalSummary::decode(TaiNs(0), &d[..20]).is_err());
        let mut bad_param = d.clone();
        bad_param[16] = 200;
        assert!(MedicalSummary::decode(TaiNs(0), &bad_param).is_err());
        let mut bad_q = d;
        bad_q[17] = 0;
        assert!(MedicalSummary::decode(TaiNs(0), &bad_q).is_err());
    }
}
