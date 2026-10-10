//! IPC commands for the crew roster and user administration (F-SYS-07, F-SYS-01; CR-012 §2–3,
//! CR-007 §4). Thin: authorisation, re-authentication and audit happen in the core.

use caelum_core::security::role::Role;
use caelum_core::station::{RosterApplied, UserSummary};
use caelum_core::store::CrewMemberRecord;
use caelum_domain::ids::{CrewMemberId, UserId};
use serde::Serialize;
use specta::Type;
use tauri::State;
use zeroize::Zeroizing;

use crate::commands::{CommandError, ErrorKind, RoleName};
use crate::state::AppState;

/// A crew member on the roster (no health data).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CrewMember {
    /// Crew member ID.
    pub id: String,
    /// Name as written in the roster.
    pub name: String,
    /// Crew position.
    pub position: String,
    /// Mission.
    pub mission: String,
    /// On the roster in force.
    pub active: bool,
}

impl From<CrewMemberRecord> for CrewMember {
    fn from(c: CrewMemberRecord) -> Self {
        Self {
            id: c.id,
            name: c.name,
            position: c.position,
            mission: c.mission,
            active: c.active,
        }
    }
}

/// A roster that was applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RosterResult {
    /// Roster version.
    pub version: String,
    /// Active crew members.
    pub members: u32,
    /// Signed with development keys only.
    pub development: bool,
}

impl From<RosterApplied> for RosterResult {
    fn from(r: RosterApplied) -> Self {
        Self {
            version: r.version,
            members: u32::try_from(r.members).unwrap_or(u32::MAX),
            development: r.development,
        }
    }
}

/// An account (never its credential).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UserItem {
    /// Account ID.
    pub id: String,
    /// Sign-in name.
    pub username: String,
    /// Role.
    pub role: RoleName,
    /// Crew member (crew roles).
    pub crew_member_id: Option<String>,
    /// Cannot sign in.
    pub disabled: bool,
}

impl From<UserSummary> for UserItem {
    fn from(u: UserSummary) -> Self {
        Self {
            id: u.id,
            username: u.username,
            role: u.role.into(),
            crew_member_id: u.crew_member_id,
            disabled: u.disabled,
        }
    }
}

fn invalid(what: &str) -> CommandError {
    CommandError {
        kind: ErrorKind::Rejected,
        message: format!("not a valid {what}"),
        retry_after_s: None,
    }
}

/// The crew roster in scope (astronaut: own entry).
#[tauri::command]
#[specta::specta]
pub async fn crew_list(state: State<'_, AppState>) -> Result<Vec<CrewMember>, CommandError> {
    state.with(|s| Ok(s.crew_list()?.into_iter().map(Into::into).collect()))
}

/// Applies a signed crew roster: the bundle text and its signature file (administrator,
/// re-authentication required).
#[tauri::command]
#[specta::specta]
pub async fn config_apply_crew(
    state: State<'_, AppState>,
    bundle: String,
    signature: String,
) -> Result<RosterResult, CommandError> {
    state.with(|s| Ok(s.config_apply_crew(bundle.as_bytes(), &signature)?.into()))
}

/// Accounts in scope.
#[tauri::command]
#[specta::specta]
pub async fn user_list(state: State<'_, AppState>) -> Result<Vec<UserItem>, CommandError> {
    state.with(|s| Ok(s.user_list()?.into_iter().map(Into::into).collect()))
}

/// Creates an account with an initial secret (administrator, re-authentication required).
#[tauri::command]
#[specta::specta]
pub async fn user_create(
    state: State<'_, AppState>,
    username: String,
    role: RoleName,
    crew_member_id: Option<String>,
    secret: String,
) -> Result<String, CommandError> {
    let secret = Zeroizing::new(secret);
    let crew = crew_member_id
        .map(|c| {
            c.parse::<CrewMemberId>()
                .map_err(|_| invalid("crew member id"))
        })
        .transpose()?;
    let role = Role::from(role);
    state.with(|s| Ok(s.user_create(&username, role, crew, &secret)?.to_string()))
}

/// Disables an account (administrator, re-authentication required).
#[tauri::command]
#[specta::specta]
pub async fn user_disable(state: State<'_, AppState>, user_id: String) -> Result<(), CommandError> {
    let id: UserId = user_id.parse().map_err(|_| invalid("account id"))?;
    state.with(|s| Ok(s.user_disable(id)?))
}
