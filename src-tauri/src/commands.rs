//! IPC commands. Commands are thin: logic and authorisation live in the core crates
//! (docs/03_ARCHITECTURE.md §1); here values are only translated for the UI.
//!
//! Access per command is `docs/ROLE_MATRIX.md` §3, enforced by `caelum_core::station::Station`
//! (CLAUDE.md rule 6). 64-bit values (TAI times, sequence numbers) are sent as decimal strings so
//! JavaScript never rounds them.

use caelum_core::security::audit::{AuditEntry, ChainStatus, ProblemKind};
use caelum_core::security::role::{DeniedReason, Role};
use caelum_core::security::session::AuthError;
use caelum_core::station::{Me, StationError};
use serde::Serialize;
use specta::Type;
use tauri::State;
use zeroize::Zeroizing;

use crate::state::AppState;

/// Product name shown in the window; must match `productName` in `tauri.conf.json`.
pub const PRODUCT_NAME: &str = "CAELUM";

/// Application identity shown in the main window (M0, F-SYS-06).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// Product name.
    pub product_name: String,
    /// Application version, taken from the Cargo workspace (single source of truth).
    pub version: String,
}

/// Returns the product name and version of this build (public, shown before sign-in).
#[tauri::command]
#[specta::specta]
pub fn app_info() -> AppInfo {
    AppInfo {
        product_name: PRODUCT_NAME.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// Why a command failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// Sign in first.
    SignInRequired,
    /// The session is locked; re-authenticate to unlock.
    Locked,
    /// Not available for this role.
    NotPermitted,
    /// Wrong sign-in name or secret.
    InvalidCredentials,
    /// Too many failures; see `retryAfterS`.
    Throttled,
    /// The account is disabled.
    AccountDisabled,
    /// The account cannot sign in interactively.
    NotInteractive,
    /// Re-authenticate first.
    ReauthRequired,
    /// CAELUM's records could not be opened.
    Unavailable,
    /// Any other failure (storage, keystore, clock).
    Failed,
}

/// A failed command, as the UI sees it. Messages never contain secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    /// Category.
    pub kind: ErrorKind,
    /// Human-readable reason.
    pub message: String,
    /// For `throttled`: seconds until the next attempt is accepted.
    pub retry_after_s: Option<u32>,
}

impl From<StationError> for CommandError {
    fn from(e: StationError) -> Self {
        let message = e.to_string();
        let mut retry_after_s = None;
        let kind = match &e {
            StationError::Denied(d) => match d.reason {
                DeniedReason::SignInRequired => ErrorKind::SignInRequired,
                DeniedReason::Locked => ErrorKind::Locked,
                DeniedReason::Role => ErrorKind::NotPermitted,
            },
            StationError::Auth(a) => match a {
                AuthError::InvalidCredentials => ErrorKind::InvalidCredentials,
                AuthError::Throttled { retry_after_s: s } => {
                    retry_after_s = Some(u32::try_from(*s).unwrap_or(u32::MAX));
                    ErrorKind::Throttled
                }
                AuthError::AccountDisabled => ErrorKind::AccountDisabled,
                AuthError::NotInteractive => ErrorKind::NotInteractive,
                AuthError::NotSignedIn => ErrorKind::SignInRequired,
                AuthError::ReauthRequired => ErrorKind::ReauthRequired,
                AuthError::Security(_) => ErrorKind::Failed,
            },
            StationError::Security(_) | StationError::Io(_) => ErrorKind::Failed,
        };
        Self {
            kind,
            message,
            retry_after_s,
        }
    }
}

/// A role (`docs/ROLE_MATRIX.md` §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum RoleName {
    /// Astronaut.
    Astronaut,
    /// Crew Medical Officer.
    Cmo,
    /// Flight Surgeon.
    FlightSurgeon,
    /// Biomedical Engineer.
    Bme,
    /// Mission Control Operator.
    Mcc,
    /// Administrator.
    Administrator,
    /// Device gateway (never signs in interactively).
    DeviceGateway,
}

impl From<Role> for RoleName {
    fn from(r: Role) -> Self {
        match r {
            Role::Astronaut => RoleName::Astronaut,
            Role::Cmo => RoleName::Cmo,
            Role::FlightSurgeon => RoleName::FlightSurgeon,
            Role::Bme => RoleName::Bme,
            Role::Mcc => RoleName::Mcc,
            Role::Administrator => RoleName::Administrator,
            Role::DeviceGateway => RoleName::DeviceGateway,
        }
    }
}

/// The signed-in user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// Account ID.
    pub user_id: String,
    /// Sign-in name.
    pub username: String,
    /// Role.
    pub role: RoleName,
    /// Crew member the account belongs to (scope of own data).
    pub crew_member_id: Option<String>,
    /// The session is locked.
    pub locked: bool,
}

impl From<Me> for SessionInfo {
    fn from(me: Me) -> Self {
        Self {
            user_id: me.identity.user_id.to_string(),
            username: me.identity.username,
            role: me.identity.role.into(),
            crew_member_id: me.identity.crew_member_id.map(|c| c.to_string()),
            locked: me.locked,
        }
    }
}

/// Result of opening the emergency view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BreakGlass {
    /// Always true: break-glass is never refused (SR-014).
    pub opened: bool,
    /// False if the opening could not be audited (shown to the crew; never blocks the view).
    pub audited: bool,
}

/// One audit entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AuditItem {
    /// Chain position (decimal).
    pub seq: String,
    /// Entry ID.
    pub id: String,
    /// TAI nanoseconds since 1958-01-01 (decimal).
    pub at_tai: String,
    /// Actor.
    pub actor: String,
    /// Actor's role.
    pub role: String,
    /// Action.
    pub action: String,
    /// Target.
    pub target: Option<String>,
    /// JSON details.
    pub details: String,
}

impl From<AuditEntry> for AuditItem {
    fn from(e: AuditEntry) -> Self {
        Self {
            seq: e.seq.to_string(),
            id: e.id,
            at_tai: e.at_tai.0.to_string(),
            actor: e.actor,
            role: e.role,
            action: e.action,
            target: e.target,
            details: e.details,
        }
    }
}

/// What is wrong at one entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ChainProblemKind {
    /// An entry is missing or was inserted.
    SequenceGap,
    /// The link to the previous entry is broken.
    BrokenLink,
    /// The entry's content was changed.
    HashMismatch,
    /// The signature is not the instance key's.
    BadSignature,
}

/// A problem at one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChainProblem {
    /// Chain position (decimal).
    pub seq: String,
    /// What is wrong.
    pub kind: ChainProblemKind,
}

/// Audit chain and signature status (`audit_verify`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AuditStatus {
    /// Entries checked (decimal).
    pub entries: String,
    /// True if every link, hash and signature checked out.
    pub intact: bool,
    /// Problems found.
    pub problems: Vec<ChainProblem>,
}

impl From<ChainStatus> for AuditStatus {
    fn from(s: ChainStatus) -> Self {
        Self {
            entries: s.entries.to_string(),
            intact: s.is_intact(),
            problems: s
                .problems
                .into_iter()
                .map(|p| ChainProblem {
                    seq: p.seq.to_string(),
                    kind: match p.kind {
                        ProblemKind::SequenceGap => ChainProblemKind::SequenceGap,
                        ProblemKind::BrokenLink => ChainProblemKind::BrokenLink,
                        ProblemKind::HashMismatch => ChainProblemKind::HashMismatch,
                        ProblemKind::BadSignature => ChainProblemKind::BadSignature,
                    },
                })
                .collect(),
        }
    }
}

// Commands are `async` so the argon2 work runs off the window's main thread.

/// Signs in with a sign-in name and PIN or password (public; throttled; every attempt audited).
#[tauri::command]
#[specta::specta]
pub async fn auth_login(
    state: State<'_, AppState>,
    username: String,
    secret: String,
) -> Result<SessionInfo, CommandError> {
    let secret = Zeroizing::new(secret);
    state.with(|s| Ok(s.login(&username, secret.as_bytes())?.into()))
}

/// Signs out.
#[tauri::command]
#[specta::specta]
pub async fn auth_logout(state: State<'_, AppState>) -> Result<(), CommandError> {
    state.with(|s| Ok(s.logout()?))
}

/// The signed-in user's identity, role and scope, and whether the session is locked.
#[tauri::command]
#[specta::specta]
pub async fn auth_me(state: State<'_, AppState>) -> Result<SessionInfo, CommandError> {
    state.with(|s| Ok(s.me()?.into()))
}

/// Re-authenticates before a sensitive action; also unlocks a locked session.
#[tauri::command]
#[specta::specta]
pub async fn auth_reauth(state: State<'_, AppState>, secret: String) -> Result<(), CommandError> {
    let secret = Zeroizing::new(secret);
    state.with(|s| Ok(s.reauth(secret.as_bytes())?))
}

/// Opens the emergency view without sign-in (SR-014; audited).
#[tauri::command]
#[specta::specta]
pub async fn break_glass_open(state: State<'_, AppState>) -> Result<BreakGlass, CommandError> {
    state.with(|s| {
        let grant = s.break_glass_open();
        Ok(BreakGlass {
            opened: true,
            audited: grant.audit_error.is_none(),
        })
    })
}

/// Audit entries the caller's role may read, in chain order.
#[tauri::command]
#[specta::specta]
pub async fn audit_list(state: State<'_, AppState>) -> Result<Vec<AuditItem>, CommandError> {
    state.with(|s| Ok(s.audit_list()?.into_iter().map(Into::into).collect()))
}

/// Audit chain and signature status.
#[tauri::command]
#[specta::specta]
pub async fn audit_verify(state: State<'_, AppState>) -> Result<AuditStatus, CommandError> {
    state.with(|s| Ok(s.audit_verify()?.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F-SYS-06: the version shown comes from the Cargo workspace version.
    #[test]
    fn app_info_reports_workspace_version_and_product_name() {
        let info = app_info();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.version, "0.1.0");
        assert_eq!(info.product_name, "CAELUM");
    }

    #[test]
    fn errors_map_to_ui_kinds_without_losing_the_retry_time() {
        let e: CommandError = StationError::Auth(AuthError::Throttled { retry_after_s: 30 }).into();
        assert_eq!(e.kind, ErrorKind::Throttled);
        assert_eq!(e.retry_after_s, Some(30));
        let e: CommandError = StationError::Auth(AuthError::InvalidCredentials).into();
        assert_eq!(e.kind, ErrorKind::InvalidCredentials);
        assert_eq!(e.retry_after_s, None);
    }

    #[test]
    fn large_values_are_sent_as_exact_decimal_strings() {
        let item: AuditItem = AuditEntry {
            seq: 1,
            id: "x".to_owned(),
            at_tai: caelum_domain::time::TaiNs(2_107_382_437_123_456_789),
            actor: "system".to_owned(),
            role: "system".to_owned(),
            action: "key.create".to_owned(),
            target: None,
            details: "{}".to_owned(),
        }
        .into();
        assert_eq!(item.at_tai, "2107382437123456789");
    }
}
