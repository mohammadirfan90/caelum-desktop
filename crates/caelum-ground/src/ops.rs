//! OPS channel records (path B): the only content Mission Control receives (docs/06 §3a,
//! docs/04 §3, ICD-VEH §5, R-SEC-06, CLAUDE.md rule 6).
//!
//! Enforced by type: every record is built from closed enumerations, a crew UUID and TAI
//! timestamps. There is no field that can hold a measured value, a parameter name, a rule id or
//! free text, and every encoding has a fixed length. Full detail follows on MEDICAL (path A/A').
//!
//! User data layouts (after the CUC secondary header, whose time is the record time `at`):
//! - alarm (APID `ops_alarm`, 26 octets): crew UUID (16), priority, category, state, escalation
//!   stage (1 each), alarm creation CUC T-field (6);
//! - crew status (APID `ops_crew_status`, 19 octets): crew UUID (16), highest active priority,
//!   its category, its state (1 each; 0 = no active alarm);
//! - instance status (APID `ops_instance_status`, 2 octets): instance health, MDS link state.

use caelum_alarms::escalation::OpsEscalation;
use caelum_domain::ids::CrewMemberId;
use caelum_domain::time::TaiNs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::ApidPlan;
use crate::packet::{Apid, Cuc, PacketError, TmPacket};

/// OPS record errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpsError {
    /// A name from the alarm core has no OPS code; refused, never mapped to a default.
    #[error("no OPS code for {field} `{value}`")]
    Unknown {
        /// Field.
        field: &'static str,
        /// Value.
        value: String,
    },
    /// An encoded record is malformed.
    #[error("malformed OPS record: {0}")]
    Malformed(String),
    /// Packet building failed.
    #[error(transparent)]
    Packet(#[from] PacketError),
}

macro_rules! ops_enum {
    ($(#[$doc:meta])* $name:ident { $($(#[$vdoc:meta])* $v:ident = $code:literal => $text:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$vdoc])* #[serde(rename = $text)] $v = $code),+
        }

        impl $name {
            /// Every value.
            pub const ALL: &'static [Self] = &[$(Self::$v),+];

            /// Wire code.
            pub const fn code(self) -> u8 {
                self as u8
            }

            /// Name used by the alarm core and the Open MCT plugin.
            pub const fn name(self) -> &'static str {
                match self { $(Self::$v => $text),+ }
            }

            /// From a wire code.
            pub fn from_code(code: u8) -> Option<Self> {
                Self::ALL.iter().copied().find(|v| v.code() == code)
            }

            /// From the alarm core's name; unknown names are refused.
            pub fn from_name(name: &str) -> Result<Self, OpsError> {
                Self::ALL.iter().copied().find(|v| v.name() == name).ok_or_else(|| OpsError::Unknown {
                    field: stringify!($name),
                    value: name.to_owned(),
                })
            }
        }
    };
}

ops_enum!(
    /// Alarm priority.
    OpsPriority {
        /// Critical (high priority).
        Critical = 1 => "critical",
        /// Warning (medium priority).
        Warning = 2 => "warning",
        /// Observe (low priority).
        Observe = 3 => "observe",
    }
);

ops_enum!(
    /// Alarm category (docs/04 §3).
    OpsCategory {
        /// Cardiovascular.
        Cardio = 1 => "CARDIO",
        /// Respiratory.
        Respiratory = 2 => "RESPIRATORY",
        /// Thermal.
        Thermal = 3 => "THERMAL",
        /// Cabin environment.
        Environment = 4 => "ENVIRONMENT",
        /// Technical (device, data quality, system).
        Technical = 5 => "TECHNICAL",
    }
);

ops_enum!(
    /// Alarm state.
    OpsAlarmState {
        /// Spot reading awaiting confirmation (SR-019).
        Confirming = 1 => "confirming",
        /// Condition present.
        Active = 2 => "active",
        /// Cleared, latched until acknowledged.
        ClearedAwaitingAck = 3 => "cleared_awaiting_ack",
        /// Acknowledged.
        Acknowledged = 4 => "acknowledged",
        /// Ended (resolved, superseded or not confirmed).
        Ended = 5 => "ended",
    }
);

ops_enum!(
    /// CAELUM instance technical health.
    InstanceHealth {
        /// All supervised tasks running; no technical alarm open.
        Ok = 1 => "ok",
        /// A technical alarm is open.
        Degraded = 2 => "degraded",
        /// A supervised task failed.
        Fault = 3 => "fault",
    }
);

ops_enum!(
    /// Onboard medical data system link (docs/04 §3a).
    MdsLinkState {
        /// No medical data system configured for the mission.
        NotConfigured = 4 => "not_configured",
        /// Connected after a successful exchange.
        Connected = 1 => "connected",
        /// Recent failures.
        Degraded = 2 => "degraded",
        /// Unreachable.
        Unreachable = 3 => "unreachable",
    }
);

/// An alarm state change or ground escalation (docs/06 §3a fields only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpsAlarmRecord {
    /// Crew member.
    pub crew_id: CrewMemberId,
    /// Priority.
    pub priority: OpsPriority,
    /// Category.
    pub category: OpsCategory,
    /// State.
    pub state: OpsAlarmState,
    /// Escalation stage index.
    pub escalation_stage: u8,
    /// Alarm creation (TAI).
    pub created_at: TaiNs,
    /// Time of this record (TAI).
    pub at: TaiNs,
}

impl TryFrom<&OpsEscalation> for OpsAlarmRecord {
    type Error = OpsError;

    /// The ground escalation stage from the escalator (M4) on path B.
    fn try_from(e: &OpsEscalation) -> Result<Self, OpsError> {
        Ok(Self {
            crew_id: e.crew_id,
            priority: OpsPriority::from_name(e.priority)?,
            category: OpsCategory::from_name(e.category)?,
            state: OpsAlarmState::from_name(e.state)?,
            escalation_stage: e.stage,
            created_at: TaiNs(e.created_at_tai),
            at: TaiNs(e.at_tai),
        })
    }
}

/// The highest-priority active alarm of a crew member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HighestAlarm {
    /// Priority.
    pub priority: OpsPriority,
    /// Category.
    pub category: OpsCategory,
    /// State.
    pub state: OpsAlarmState,
}

/// Per-crew status. `highest` is `None` when the crew member has no active alarm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrewStatusRecord {
    /// Crew member.
    pub crew_id: CrewMemberId,
    /// Highest active alarm.
    pub highest: Option<HighestAlarm>,
    /// Time of this record (TAI).
    pub at: TaiNs,
}

/// CAELUM instance status (technical health).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceStatusRecord {
    /// Instance health.
    pub health: InstanceHealth,
    /// Medical data system link.
    pub mds_link: MdsLinkState,
    /// Time of this record (TAI).
    pub at: TaiNs,
}

/// Any OPS record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OpsRecord {
    /// Alarm.
    Alarm(OpsAlarmRecord),
    /// Crew status.
    CrewStatus(CrewStatusRecord),
    /// Instance status.
    InstanceStatus(InstanceStatusRecord),
}

fn crew_bytes(id: CrewMemberId) -> [u8; 16] {
    *id.as_uuid().as_bytes()
}

fn crew_from(b: &[u8]) -> Result<CrewMemberId, OpsError> {
    let arr: [u8; 16] = b
        .get(..16)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| OpsError::Malformed("crew id".into()))?;
    Ok(CrewMemberId::from_uuid(Uuid::from_bytes(arr)))
}

fn code<T>(v: Option<T>, what: &str) -> Result<T, OpsError> {
    v.ok_or_else(|| OpsError::Malformed(format!("bad {what} code")))
}

impl OpsRecord {
    /// Record time.
    pub fn at(&self) -> TaiNs {
        match self {
            Self::Alarm(r) => r.at,
            Self::CrewStatus(r) => r.at,
            Self::InstanceStatus(r) => r.at,
        }
    }

    /// APID of this record in the plan.
    pub fn apid(&self, plan: &ApidPlan) -> Apid {
        match self {
            Self::Alarm(_) => plan.ops_alarm,
            Self::CrewStatus(_) => plan.ops_crew_status,
            Self::InstanceStatus(_) => plan.ops_instance_status,
        }
    }

    /// Fixed-length user data.
    pub fn user_data(&self) -> Result<Vec<u8>, OpsError> {
        Ok(match self {
            Self::Alarm(r) => {
                let mut b = crew_bytes(r.crew_id).to_vec();
                b.extend([
                    r.priority.code(),
                    r.category.code(),
                    r.state.code(),
                    r.escalation_stage,
                ]);
                b.extend(Cuc::from_tai(r.created_at)?.t_field());
                b
            }
            Self::CrewStatus(r) => {
                let mut b = crew_bytes(r.crew_id).to_vec();
                match r.highest {
                    Some(h) => b.extend([h.priority.code(), h.category.code(), h.state.code()]),
                    None => b.extend([0, 0, 0]),
                }
                b
            }
            Self::InstanceStatus(r) => vec![r.health.code(), r.mds_link.code()],
        })
    }

    /// Builds the telemetry packet.
    pub fn to_packet(&self, plan: &ApidPlan, seq_count: u16) -> Result<TmPacket, OpsError> {
        Ok(TmPacket::build(
            self.apid(plan),
            seq_count,
            self.at(),
            &self.user_data()?,
        )?)
    }

    /// Decodes user data of a packet on an OPS APID (ground side).
    pub fn decode(plan: &ApidPlan, apid: Apid, at: TaiNs, d: &[u8]) -> Result<Self, OpsError> {
        let len = |n: usize| {
            if d.len() == n {
                Ok(())
            } else {
                Err(OpsError::Malformed(format!(
                    "length {} is not {n}",
                    d.len()
                )))
            }
        };
        if apid == plan.ops_alarm {
            len(26)?;
            Ok(Self::Alarm(OpsAlarmRecord {
                crew_id: crew_from(d)?,
                priority: code(OpsPriority::from_code(d[16]), "priority")?,
                category: code(OpsCategory::from_code(d[17]), "category")?,
                state: code(OpsAlarmState::from_code(d[18]), "state")?,
                escalation_stage: d[19],
                created_at: Cuc::from_t_field(&d[20..26])?.to_tai(),
                at,
            }))
        } else if apid == plan.ops_crew_status {
            len(19)?;
            let highest = if d[16..19] == [0, 0, 0] {
                None
            } else {
                Some(HighestAlarm {
                    priority: code(OpsPriority::from_code(d[16]), "priority")?,
                    category: code(OpsCategory::from_code(d[17]), "category")?,
                    state: code(OpsAlarmState::from_code(d[18]), "state")?,
                })
            };
            Ok(Self::CrewStatus(CrewStatusRecord {
                crew_id: crew_from(d)?,
                highest,
                at,
            }))
        } else if apid == plan.ops_instance_status {
            len(2)?;
            Ok(Self::InstanceStatus(InstanceStatusRecord {
                health: code(InstanceHealth::from_code(d[0]), "health")?,
                mds_link: code(MdsLinkState::from_code(d[1]), "MDS link")?,
                at,
            }))
        } else {
            Err(OpsError::Malformed(format!(
                "APID {} is not an OPS APID",
                apid.value()
            )))
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::tests::GROUND_TOML;
    use crate::config::GroundConfig;
    use crate::packet::ParsedTm;
    use std::collections::BTreeSet;

    pub(crate) fn plan() -> ApidPlan {
        GroundConfig::parse(GROUND_TOML).expect("valid").apids
    }

    pub(crate) fn crew() -> CrewMemberId {
        "67e55044-10b1-426f-9247-bb680e5fe0c8"
            .parse()
            .expect("uuid")
    }

    pub(crate) fn alarm(at: i64) -> OpsAlarmRecord {
        OpsAlarmRecord {
            crew_id: crew(),
            priority: OpsPriority::Critical,
            category: OpsCategory::Cardio,
            state: OpsAlarmState::Active,
            escalation_stage: 4,
            created_at: TaiNs(at - 60_000_000_000),
            at: TaiNs(at),
        }
    }

    fn keys(v: &serde_json::Value) -> BTreeSet<String> {
        v.as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// R-SEC-06, docs/06 §3a: an OPS alarm record carries exactly the allowed fields and nothing
    /// that could hold physiology (no measured value, parameter name, rule id or free text).
    #[test]
    fn r_sec_06_ops_alarm_has_only_the_allowed_fields() {
        let v = serde_json::to_value(OpsRecord::Alarm(alarm(1_000_000_000_000))).expect("json");
        let allowed: BTreeSet<String> = [
            "type",
            "crew_id",
            "priority",
            "category",
            "state",
            "escalation_stage",
            "created_at",
            "at",
        ]
        .map(String::from)
        .into();
        assert_eq!(keys(&v), allowed);
        // Every value is an enum name, a UUID or a number: no room for text.
        for (k, field) in v.as_object().expect("object") {
            if let Some(s) = field.as_str() {
                let enum_names: Vec<&str> = OpsPriority::ALL
                    .iter()
                    .map(|p| p.name())
                    .chain(OpsCategory::ALL.iter().map(|c| c.name()))
                    .chain(OpsAlarmState::ALL.iter().map(|s| s.name()))
                    .chain(["alarm"])
                    .collect();
                assert!(
                    enum_names.contains(&s) || s.parse::<Uuid>().is_ok(),
                    "{k} = {s} is free text"
                );
            }
        }
        // Unknown fields (e.g. a value or rule id) are refused when decoding.
        let mut with_value = v.clone();
        with_value["value"] = serde_json::json!(38.5);
        assert!(serde_json::from_value::<OpsRecord>(with_value).is_err());
        let mut with_rule = v;
        with_rule["rule_id"] = serde_json::json!("spo2.critical");
        assert!(serde_json::from_value::<OpsRecord>(with_rule).is_err());
    }

    /// R-SEC-06: encodings have a fixed length, so nothing else fits in an OPS packet.
    #[test]
    fn r_sec_06_ops_encodings_have_fixed_length() {
        let p = plan();
        let records = [
            OpsRecord::Alarm(alarm(5_000_000_000_000)),
            OpsRecord::CrewStatus(CrewStatusRecord {
                crew_id: crew(),
                highest: None,
                at: TaiNs(1),
            }),
            OpsRecord::InstanceStatus(InstanceStatusRecord {
                health: InstanceHealth::Degraded,
                mds_link: MdsLinkState::Unreachable,
                at: TaiNs(1),
            }),
        ];
        let lens: Vec<usize> = records
            .iter()
            .map(|r| r.user_data().expect("encodes").len())
            .collect();
        assert_eq!(lens, [26, 19, 2]);
        for r in records {
            let pkt = r.to_packet(&p, 0).expect("packet");
            let (parsed, _) = ParsedTm::parse(pkt.bytes()).expect("parses");
            let back = OpsRecord::decode(&p, parsed.apid, parsed.time, &parsed.user_data)
                .expect("decodes");
            // Times survive to CUC resolution; these are whole seconds or exact.
            assert_eq!(back.apid(&p), r.apid(&p));
        }
    }

    /// F-GND-11: the ground escalation of the escalator becomes an OPS record; unknown names are
    /// refused, never defaulted.
    #[test]
    fn f_gnd_11_ops_escalation_converts_strictly() {
        let e = OpsEscalation {
            crew_id: crew(),
            priority: "critical",
            category: "RESPIRATORY",
            state: "active",
            stage: 4,
            created_at_tai: 1_000,
            at_tai: 61_000,
        };
        let r = OpsAlarmRecord::try_from(&e).expect("converts");
        assert_eq!(r.category, OpsCategory::Respiratory);
        assert_eq!(r.escalation_stage, 4);
        let bad = OpsEscalation {
            category: "MYSTERY",
            ..e
        };
        assert!(matches!(
            OpsAlarmRecord::try_from(&bad),
            Err(OpsError::Unknown { .. })
        ));
    }

    #[test]
    fn alarm_round_trips_through_a_packet() {
        let p = plan();
        let r = OpsRecord::Alarm(alarm(7_000_000_000_000));
        let pkt = r.to_packet(&p, 42).expect("packet");
        let (parsed, _) = ParsedTm::parse(pkt.bytes()).expect("parses");
        assert_eq!(parsed.seq_count, 42);
        let back = OpsRecord::decode(&p, parsed.apid, parsed.time, &parsed.user_data).expect("ok");
        assert_eq!(back, r);
    }

    #[test]
    fn crew_status_none_and_some_round_trip() {
        let p = plan();
        for highest in [
            None,
            Some(HighestAlarm {
                priority: OpsPriority::Warning,
                category: OpsCategory::Thermal,
                state: OpsAlarmState::Confirming,
            }),
        ] {
            let r = OpsRecord::CrewStatus(CrewStatusRecord {
                crew_id: crew(),
                highest,
                at: TaiNs(3_000_000_000),
            });
            let d = r.user_data().expect("encodes");
            let back = OpsRecord::decode(&p, p.ops_crew_status, TaiNs(3_000_000_000), &d)
                .expect("decodes");
            assert_eq!(back, r);
        }
    }

    #[test]
    fn malformed_ops_data_is_rejected() {
        let p = plan();
        let mut d = OpsRecord::Alarm(alarm(9_000_000_000_000))
            .user_data()
            .expect("encodes");
        assert!(OpsRecord::decode(&p, p.ops_alarm, TaiNs(0), &d[..25]).is_err());
        d[16] = 9;
        assert!(OpsRecord::decode(&p, p.ops_alarm, TaiNs(0), &d).is_err());
        assert!(OpsRecord::decode(&p, p.medical_summary, TaiNs(0), &[0, 0]).is_err());
        assert!(OpsRecord::decode(&p, p.ops_instance_status, TaiNs(0), &[0, 1]).is_err());
    }

    #[test]
    fn enum_codes_are_unique_and_names_round_trip() {
        for p in OpsPriority::ALL {
            assert_eq!(OpsPriority::from_name(p.name()), Ok(*p));
            assert_eq!(OpsPriority::from_code(p.code()), Some(*p));
        }
        for s in MdsLinkState::ALL {
            assert_eq!(MdsLinkState::from_code(s.code()), Some(*s));
        }
        assert_eq!(OpsAlarmState::from_code(0), None);
    }
}
