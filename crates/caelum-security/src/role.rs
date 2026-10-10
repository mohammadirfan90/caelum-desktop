//! Roles, principals and command access (`docs/ROLE_MATRIX.md`, F-SYS-01, R-SEC-02).
//!
//! Every IPC command is checked here before it runs (CLAUDE.md rule 6). The table in
//! [`Command::access`] is tested against `docs/ROLE_MATRIX.md` §3, so the code and the approved
//! matrix cannot drift apart.

use std::fmt;

use caelum_domain::ids::{CrewMemberId, UserId};
use serde::{Deserialize, Serialize};

use crate::audit::AuditCategory;

/// A role (`docs/ROLE_MATRIX.md` §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Astronaut: own data only.
    Astronaut,
    /// Crew Medical Officer: all crew onboard.
    Cmo,
    /// Flight Surgeon (ground): all crew.
    FlightSurgeon,
    /// Biomedical Engineer (ground): devices, technical alarms, data quality.
    Bme,
    /// Mission Control Operator (ground): operational status only.
    Mcc,
    /// Administrator: configuration only, no clinical content.
    Administrator,
    /// Device gateway machine identity: ingest only.
    DeviceGateway,
}

impl Role {
    /// Every role.
    pub const ALL: [Role; 7] = [
        Role::Astronaut,
        Role::Cmo,
        Role::FlightSurgeon,
        Role::Bme,
        Role::Mcc,
        Role::Administrator,
        Role::DeviceGateway,
    ];

    /// The stored name (database `user.role`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Astronaut => "astronaut",
            Role::Cmo => "cmo",
            Role::FlightSurgeon => "flight_surgeon",
            Role::Bme => "bme",
            Role::Mcc => "mcc",
            Role::Administrator => "administrator",
            Role::DeviceGateway => "device_gateway",
        }
    }

    /// Parses a stored name.
    pub fn parse(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.as_str() == name)
    }

    /// Crew roles sign in onboard with a PIN (`docs/ROLE_MATRIX.md` §4) and must belong to a
    /// crew member (their data scope).
    pub const fn is_onboard_crew(self) -> bool {
        matches!(self, Role::Astronaut | Role::Cmo)
    }

    /// Machine identities authenticate with keys, never through the interactive sign-in.
    pub const fn is_machine(self) -> bool {
        matches!(self, Role::DeviceGateway)
    }

    /// Audit entries this role may read (`docs/ROLE_MATRIX.md` §2, "Audit log").
    pub const fn audit_categories(self) -> &'static [AuditCategory] {
        match self {
            Role::Cmo | Role::FlightSurgeon => &AuditCategory::ALL,
            Role::Bme => &[AuditCategory::Technical],
            Role::Administrator => &[AuditCategory::Configuration, AuditCategory::Security],
            Role::Astronaut | Role::Mcc | Role::DeviceGateway => &[],
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A signed-in user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserIdentity {
    /// Account ID.
    pub user_id: UserId,
    /// Sign-in name.
    pub username: String,
    /// Role.
    pub role: Role,
    /// The crew member the account belongs to (scope of "own data").
    pub crew_member_id: Option<CrewMemberId>,
}

/// Who is calling a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// Nobody is signed in.
    Anonymous,
    /// The emergency view was opened without sign-in (SR-014).
    BreakGlass,
    /// A signed-in user whose session is locked: only the session commands (identity, unlock by
    /// re-authentication, sign out) are available (`docs/ROLE_MATRIX.md` §4).
    Locked(UserIdentity),
    /// A signed-in, unlocked user.
    User(UserIdentity),
}

/// The M1 IPC commands (`docs/ROLE_MATRIX.md` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    /// Product name and version.
    AppInfo,
    /// Sign in.
    AuthLogin,
    /// Sign out.
    AuthLogout,
    /// Own identity, role and scope.
    AuthMe,
    /// Re-authentication before sensitive actions; also unlocks a locked session.
    AuthReauth,
    /// Opens the emergency view without sign-in.
    BreakGlassOpen,
    /// Audit entries, filtered by role.
    AuditList,
    /// Audit chain and signature status.
    AuditVerify,
}

/// Who may call a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Anyone, without sign-in.
    Public,
    /// Any signed-in user, also while the session is locked (session commands only).
    AnySignedIn,
    /// Signed-in users with one of these roles.
    Roles(&'static [Role]),
}

impl Command {
    /// Every command.
    pub const ALL: [Command; 8] = [
        Command::AppInfo,
        Command::AuthLogin,
        Command::AuthLogout,
        Command::AuthMe,
        Command::AuthReauth,
        Command::BreakGlassOpen,
        Command::AuditList,
        Command::AuditVerify,
    ];

    /// The IPC command name.
    pub const fn name(self) -> &'static str {
        match self {
            Command::AppInfo => "app_info",
            Command::AuthLogin => "auth_login",
            Command::AuthLogout => "auth_logout",
            Command::AuthMe => "auth_me",
            Command::AuthReauth => "auth_reauth",
            Command::BreakGlassOpen => "break_glass_open",
            Command::AuditList => "audit_list",
            Command::AuditVerify => "audit_verify",
        }
    }

    /// Looks a command up by its IPC name.
    pub fn from_name(name: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The access rule (`docs/ROLE_MATRIX.md` §3).
    pub const fn access(self) -> Access {
        match self {
            Command::AppInfo | Command::AuthLogin | Command::BreakGlassOpen => Access::Public,
            Command::AuthLogout | Command::AuthMe | Command::AuthReauth => Access::AnySignedIn,
            Command::AuditList => Access::Roles(&[
                Role::Cmo,
                Role::FlightSurgeon,
                Role::Bme,
                Role::Administrator,
            ]),
            Command::AuditVerify => Access::Roles(&[
                Role::Cmo,
                Role::FlightSurgeon,
                Role::Bme,
                Role::Administrator,
            ]),
        }
    }

    /// Checks that `principal` may call this command.
    pub fn authorize(self, principal: &Principal) -> Result<(), Denied> {
        let allowed = match (self.access(), principal) {
            (Access::Public, _) => true,
            (Access::AnySignedIn, Principal::User(_) | Principal::Locked(_)) => true,
            (Access::Roles(roles), Principal::User(u)) => roles.contains(&u.role),
            (Access::AnySignedIn | Access::Roles(_), _) => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(Denied {
                command: self,
                reason: match principal {
                    Principal::User(_) => DeniedReason::Role,
                    Principal::Locked(_) => DeniedReason::Locked,
                    Principal::Anonymous | Principal::BreakGlass => DeniedReason::SignInRequired,
                },
            })
        }
    }
}

/// A refused command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{} is not permitted: {}", command.name(), reason.text())]
pub struct Denied {
    /// The command.
    pub command: Command,
    /// Why.
    pub reason: DeniedReason,
}

/// Why a command was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeniedReason {
    /// Nobody is signed in.
    SignInRequired,
    /// The session is locked; unlock first.
    Locked,
    /// The signed-in role may not use this command.
    Role,
}

impl DeniedReason {
    const fn text(self) -> &'static str {
        match self {
            DeniedReason::SignInRequired => "sign-in required",
            DeniedReason::Locked => "the session is locked",
            DeniedReason::Role => "not available for this role",
        }
    }
}
