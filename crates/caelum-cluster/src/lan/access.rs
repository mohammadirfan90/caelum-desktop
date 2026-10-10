//! Who may call each LAN endpoint, and what each viewer may see (CLAUDE.md rule 6,
//! `docs/ROLE_MATRIX.md`, F-SYS-05).
//!
//! User endpoints reuse the IPC command rules of `caelum-security` (one source of truth): the
//! alarm list and stream are exactly `alarm_list`. What each viewer receives is decided by
//! [`project`]: the astronaut their own alarms with detail, the CMO, flight surgeon and BME every
//! alarm with detail, MCC the OPS fields only (docs/06 §3a), a kiosk or break-glass viewer the
//! crew-wide emergency view (OPS fields of critical and warning alarms; SR-014).

use caelum_domain::ids::{AlarmId, CrewMemberId};
use caelum_security::role::{Command, Principal, Role, UserIdentity};
use serde::{Deserialize, Serialize};

use super::{StationIdentity, StationKind};

/// The LAN endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// `GET /v1/status`: instance role and fence, for the status footer and primary discovery.
    Status,
    /// `POST /v1/sessions`: a user signs in from a secondary station.
    SessionOpen,
    /// `DELETE /v1/sessions`: sign out.
    SessionClose,
    /// `GET /v1/alarms`: alarm snapshot, projected per viewer.
    AlarmList,
    /// `GET /v1/alarms/stream`: the alarm bus as a WebSocket, projected per viewer.
    AlarmStream,
    /// `GET /v1/emergency/stream`: the crew-wide emergency view without sign-in (SR-014).
    EmergencyStream,
    /// `GET /v1/cluster/replicate`: replication from the primary (peer only).
    Replicate,
}

/// Who may call an endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointAccess {
    /// Any provisioned station of these kinds; no user session needed.
    Stations(&'static [StationKind]),
    /// A provisioned station of these kinds with a valid session of a user the command allows.
    User {
        /// Station kinds.
        stations: &'static [StationKind],
        /// The IPC command whose role rule applies; `None`: any signed-in user.
        command: Option<Command>,
    },
}

impl Endpoint {
    /// Every endpoint.
    pub const ALL: [Endpoint; 7] = [
        Endpoint::Status,
        Endpoint::SessionOpen,
        Endpoint::SessionClose,
        Endpoint::AlarmList,
        Endpoint::AlarmStream,
        Endpoint::EmergencyStream,
        Endpoint::Replicate,
    ];

    /// HTTP method and path.
    pub const fn route(self) -> (&'static str, &'static str) {
        match self {
            Endpoint::Status => ("GET", "/v1/status"),
            Endpoint::SessionOpen => ("POST", "/v1/sessions"),
            Endpoint::SessionClose => ("DELETE", "/v1/sessions"),
            Endpoint::AlarmList => ("GET", "/v1/alarms"),
            Endpoint::AlarmStream => ("GET", "/v1/alarms/stream"),
            Endpoint::EmergencyStream => ("GET", "/v1/emergency/stream"),
            Endpoint::Replicate => ("GET", "/v1/cluster/replicate"),
        }
    }

    /// The access rule.
    pub const fn access(self) -> EndpointAccess {
        const ALL_STATIONS: &[StationKind] = &[
            StationKind::Peer,
            StationKind::Secondary,
            StationKind::Kiosk,
        ];
        const SECONDARY: &[StationKind] = &[StationKind::Secondary];
        match self {
            Endpoint::Status => EndpointAccess::Stations(ALL_STATIONS),
            Endpoint::SessionOpen => EndpointAccess::Stations(SECONDARY),
            Endpoint::SessionClose => EndpointAccess::User {
                stations: SECONDARY,
                command: None,
            },
            Endpoint::AlarmList | Endpoint::AlarmStream => EndpointAccess::User {
                stations: SECONDARY,
                command: Some(Command::AlarmList),
            },
            // Break-glass: any crew station, without sign-in (ROLE_MATRIX §1, SR-014).
            Endpoint::EmergencyStream => {
                EndpointAccess::Stations(&[StationKind::Secondary, StationKind::Kiosk])
            }
            Endpoint::Replicate => EndpointAccess::Stations(&[StationKind::Peer]),
        }
    }

    /// Whether the endpoint needs this node to be primary (only the primary serves crew data).
    pub const fn primary_only(self) -> bool {
        matches!(
            self,
            Endpoint::SessionOpen
                | Endpoint::SessionClose
                | Endpoint::AlarmList
                | Endpoint::AlarmStream
                | Endpoint::EmergencyStream
        )
    }
}

/// Why a LAN request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum LanDenied {
    /// No valid session token (HTTP 401).
    #[error("sign-in required")]
    Unauthenticated,
    /// This station kind may not use the endpoint (HTTP 403).
    #[error("station not permitted")]
    Station,
    /// The user's role may not use the endpoint (HTTP 403).
    #[error("role not permitted")]
    Role,
}

/// Checks `endpoint` for `station` and the session's user (if any).
pub fn authorize(
    endpoint: Endpoint,
    station: &StationIdentity,
    user: Option<&UserIdentity>,
) -> Result<(), LanDenied> {
    match endpoint.access() {
        EndpointAccess::Stations(kinds) => {
            if kinds.contains(&station.kind) {
                Ok(())
            } else {
                Err(LanDenied::Station)
            }
        }
        EndpointAccess::User { stations, command } => {
            if !stations.contains(&station.kind) {
                return Err(LanDenied::Station);
            }
            let user = user.ok_or(LanDenied::Unauthenticated)?;
            match command {
                None => Ok(()),
                Some(command) => command
                    .authorize(&Principal::User(user.clone()))
                    .map_err(|_| LanDenied::Role),
            }
        }
    }
}

/// Alarm priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlarmPriority {
    /// Critical.
    Critical,
    /// Warning.
    Warning,
    /// Observe.
    Observe,
}

/// The OPS fields of an alarm (docs/06 §3a): no measured value, parameter name, rule ID or free
/// text. `category` and `state` are identifiers (`[a-z_]`, ≤ 32), checked on construction, so no
/// free text can travel in them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpsFields {
    /// Alarm.
    pub alarm_id: AlarmId,
    /// Crew member.
    pub crew_id: CrewMemberId,
    /// Priority.
    pub priority: AlarmPriority,
    /// Category identifier.
    pub category: String,
    /// State identifier.
    pub state: String,
    /// Escalation stage index.
    pub stage: u8,
    /// Creation (TAI ns).
    pub created_at_tai_ns: i64,
    /// Last change (TAI ns).
    pub updated_at_tai_ns: i64,
}

/// An identifier is not a short `[a-z_]` name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0} must be 1 to 32 characters of a-z or '_'")]
pub struct NotAnIdentifier(pub &'static str);

fn identifier(name: &'static str, value: &str) -> Result<(), NotAnIdentifier> {
    let ok = !value.is_empty()
        && value.len() <= 32
        && value.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(NotAnIdentifier(name))
    }
}

/// One alarm as the core publishes it on the LAN alarm bus.
#[derive(Debug, Clone, PartialEq)]
pub struct LanAlarm {
    ops: OpsFields,
    detail: Option<serde_json::Value>,
}

impl LanAlarm {
    /// An alarm: its OPS fields and, for viewers allowed physiology, its detail document (the
    /// core's `alarm_detail` content: values, parameter, rule, why).
    pub fn new(ops: OpsFields, detail: Option<serde_json::Value>) -> Result<Self, NotAnIdentifier> {
        identifier("category", &ops.category)?;
        identifier("state", &ops.state)?;
        Ok(Self { ops, detail })
    }

    /// OPS fields.
    pub fn ops(&self) -> &OpsFields {
        &self.ops
    }
}

/// What one viewer receives for one alarm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlarmView {
    /// OPS fields.
    pub ops: OpsFields,
    /// Physiological detail; absent unless the viewer may read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

/// Who is looking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Viewer {
    /// A signed-in user.
    User(UserIdentity),
    /// A kiosk or a break-glass station: the crew-wide emergency view.
    Emergency,
}

/// The view of `alarm` for `viewer`, or `None` if the viewer may not see it at all.
pub fn project(alarm: &LanAlarm, viewer: &Viewer) -> Option<AlarmView> {
    let full = || AlarmView {
        ops: alarm.ops.clone(),
        detail: alarm.detail.clone(),
    };
    let ops_only = || AlarmView {
        ops: alarm.ops.clone(),
        detail: None,
    };
    match viewer {
        Viewer::User(user) => match user.role {
            Role::Astronaut => (user.crew_member_id == Some(alarm.ops.crew_id)).then(full),
            Role::Cmo | Role::FlightSurgeon | Role::Bme => Some(full()),
            Role::Mcc => Some(ops_only()),
            Role::Administrator | Role::DeviceGateway => None,
        },
        Viewer::Emergency => matches!(
            alarm.ops.priority,
            AlarmPriority::Critical | AlarmPriority::Warning
        )
        .then(ops_only),
    }
}
