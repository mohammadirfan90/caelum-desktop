//! Sessions: sign-in, auto-lock, re-authentication and break-glass (F-SYS-01, R-SEC-02,
//! `docs/ROLE_MATRIX.md` §4, SR-014).
//!
//! One session per station. Timing uses the monotonic clock. A locked session keeps its user and
//! is unlocked by that user's re-authentication; alarms never depend on any of this (they are
//! shown and sounded regardless of sign-in, docs/06).

use std::collections::HashMap;

use caelum_domain::ids::{CrewMemberId, UserId};
use serde_json::json;

use crate::audit::{Actor, AuditAction, AuditLog};
use crate::credential::{verify_against_dummy, verify_secret};
use crate::role::{Principal, Role, UserIdentity};
use crate::SecurityError;

const NS_PER_S: u64 = 1_000_000_000;

/// Session settings (CR-006 §2, CR-007 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionConfig {
    /// Lock after this much inactivity (default 10 minutes).
    pub idle_lock_ns: u64,
    /// A re-authentication is valid for sensitive actions this long (default 2 minutes).
    pub reauth_valid_ns: u64,
    /// Consecutive failures for one sign-in name before it is throttled (default 5).
    pub max_failures: u32,
    /// First throttle period; doubles with each further failure (default 30 s).
    pub throttle_base_ns: u64,
    /// Longest throttle period (default 15 minutes).
    pub throttle_max_ns: u64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            idle_lock_ns: 10 * 60 * NS_PER_S,
            reauth_valid_ns: 2 * 60 * NS_PER_S,
            max_failures: 5,
            throttle_base_ns: 30 * NS_PER_S,
            throttle_max_ns: 15 * 60 * NS_PER_S,
        }
    }
}

/// Sign-in and session errors. Messages never say whether a sign-in name exists.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// Wrong sign-in name or secret.
    #[error("sign-in name or secret is not correct")]
    InvalidCredentials,
    /// Too many failures; try again later.
    #[error("too many failed attempts; try again in {retry_after_s} s")]
    Throttled {
        /// Seconds until the next attempt is accepted.
        retry_after_s: u64,
    },
    /// The account is disabled (reported only after a correct secret).
    #[error("this account is disabled")]
    AccountDisabled,
    /// Machine identities cannot use the interactive sign-in.
    #[error("this account cannot sign in interactively")]
    NotInteractive,
    /// No session.
    #[error("not signed in")]
    NotSignedIn,
    /// The action needs a re-authentication within the last two minutes.
    #[error("re-authentication required")]
    ReauthRequired,
    /// Storage, audit or clock failure.
    #[error(transparent)]
    Security(#[from] SecurityError),
}

/// What the station shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    /// Nobody is signed in.
    SignedOut,
    /// Signed in and unlocked.
    Active(UserIdentity),
    /// Signed in but locked; the same user unlocks with re-authentication.
    Locked(UserIdentity),
}

#[derive(Debug, Clone)]
struct Session {
    identity: UserIdentity,
    last_activity_ns: u64,
    reauth_at_ns: Option<u64>,
    locked: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Failures {
    count: u32,
    last_ns: u64,
}

/// Bound on remembered failure counters (oldest dropped first).
const MAX_TRACKED_NAMES: usize = 1024;

/// The station's session.
#[derive(Debug, Default)]
pub struct SessionManager {
    config: SessionConfig,
    session: Option<Session>,
    break_glass: bool,
    failures: HashMap<String, Failures>,
}

/// A break-glass opening (SR-014): always granted; `audit_error` reports an audit failure, which
/// never blocks the emergency view.
#[derive(Debug)]
pub struct BreakGlassGrant {
    /// Set if the opening could not be written to the audit log.
    pub audit_error: Option<SecurityError>,
}

impl SessionManager {
    /// A manager with these settings and nobody signed in.
    pub fn new(config: SessionConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    /// Signs in with a sign-in name and secret (PIN or password). Any existing session ends.
    pub fn login(
        &mut self,
        log: &AuditLog<'_>,
        username: &str,
        secret: &[u8],
    ) -> Result<UserIdentity, AuthError> {
        let now = log.clock().monotonic_ns();
        let key = username.to_lowercase();
        if let Some(retry_after_s) = self.throttled(&key, now) {
            log.append(
                &Actor::claimed(username, None),
                AuditAction::LoginFailed,
                None,
                &json!({"reason": "throttled"}),
            )?;
            return Err(AuthError::Throttled { retry_after_s });
        }
        let record = log
            .db()
            .user_by_username(&key)
            .map_err(SecurityError::from)?;
        let Some(record) = record else {
            verify_against_dummy(secret);
            self.fail(log, &key, now, username, None, "invalid_credentials")?;
            return Err(AuthError::InvalidCredentials);
        };
        let role = Role::parse(&record.role);
        if !verify_secret(secret, &record.credential) {
            self.fail(log, &key, now, username, role, "invalid_credentials")?;
            return Err(AuthError::InvalidCredentials);
        }
        let identity = identity_of(&record)?;
        if record.disabled {
            self.fail(log, &key, now, username, role, "disabled")?;
            return Err(AuthError::AccountDisabled);
        }
        if identity.role.is_machine() {
            self.fail(log, &key, now, username, role, "not_interactive")?;
            return Err(AuthError::NotInteractive);
        }
        self.failures.remove(&key);
        if let Some(previous) = self.session.take() {
            log.append(
                &Actor::user(&previous.identity),
                AuditAction::Logout,
                None,
                &json!({"reason": "replaced"}),
            )?;
        }
        log.append(
            &Actor::user(&identity),
            AuditAction::Login,
            None,
            &json!({}),
        )?;
        self.session = Some(Session {
            identity: identity.clone(),
            last_activity_ns: now,
            reauth_at_ns: Some(now),
            locked: false,
        });
        self.break_glass = false;
        Ok(identity)
    }

    /// Signs out (also from a locked session).
    pub fn logout(&mut self, log: &AuditLog<'_>) -> Result<(), AuthError> {
        let session = self.session.take().ok_or(AuthError::NotSignedIn)?;
        log.append(
            &Actor::user(&session.identity),
            AuditAction::Logout,
            None,
            &json!({"reason": "user"}),
        )?;
        self.break_glass = false;
        Ok(())
    }

    /// The current state, after applying the idle lock.
    pub fn status(&mut self, log: &AuditLog<'_>) -> Result<SessionStatus, AuthError> {
        self.apply_idle_lock(log)?;
        Ok(match &self.session {
            None => SessionStatus::SignedOut,
            Some(s) if s.locked => SessionStatus::Locked(s.identity.clone()),
            Some(s) => SessionStatus::Active(s.identity.clone()),
        })
    }

    /// Who is calling, after applying the idle lock. A call by an active user counts as activity.
    pub fn principal(&mut self, log: &AuditLog<'_>) -> Result<Principal, AuthError> {
        self.apply_idle_lock(log)?;
        let now = log.clock().monotonic_ns();
        Ok(match &mut self.session {
            Some(s) if s.locked => Principal::Locked(s.identity.clone()),
            Some(s) => {
                s.last_activity_ns = now;
                Principal::User(s.identity.clone())
            }
            None if self.break_glass => Principal::BreakGlass,
            None => Principal::Anonymous,
        })
    }

    /// Re-authenticates the session's user; unlocks a locked session.
    pub fn reauth(&mut self, log: &AuditLog<'_>, secret: &[u8]) -> Result<(), AuthError> {
        self.apply_idle_lock(log)?;
        let now = log.clock().monotonic_ns();
        let identity = self
            .session
            .as_ref()
            .map(|s| s.identity.clone())
            .ok_or(AuthError::NotSignedIn)?;
        let key = identity.username.to_lowercase();
        if let Some(retry_after_s) = self.throttled(&key, now) {
            log.append(
                &Actor::user(&identity),
                AuditAction::ReauthFailed,
                None,
                &json!({"reason": "throttled"}),
            )?;
            return Err(AuthError::Throttled { retry_after_s });
        }
        let record = log
            .db()
            .user_by_username(&key)
            .map_err(SecurityError::from)?;
        let valid = record
            .as_ref()
            .is_some_and(|r| !r.disabled && verify_secret(secret, &r.credential));
        if !valid {
            self.count_failure(&key, now);
            log.append(
                &Actor::user(&identity),
                AuditAction::ReauthFailed,
                None,
                &json!({"reason": "invalid_credentials"}),
            )?;
            return Err(AuthError::InvalidCredentials);
        }
        self.failures.remove(&key);
        let unlocked = self.session.as_ref().is_some_and(|s| s.locked);
        log.append(
            &Actor::user(&identity),
            AuditAction::Reauth,
            None,
            &json!({"unlocked": unlocked}),
        )?;
        if let Some(s) = &mut self.session {
            s.locked = false;
            s.last_activity_ns = now;
            s.reauth_at_ns = Some(now);
        }
        Ok(())
    }

    /// Succeeds if the active session re-authenticated within the configured window
    /// (`docs/ROLE_MATRIX.md` §4: signing, applying configuration, changing users, assigning
    /// devices).
    pub fn require_recent_reauth(&mut self, log: &AuditLog<'_>) -> Result<(), AuthError> {
        self.apply_idle_lock(log)?;
        let now = log.clock().monotonic_ns();
        match &self.session {
            Some(s) if !s.locked => match s.reauth_at_ns {
                Some(at) if now.saturating_sub(at) <= self.config.reauth_valid_ns => Ok(()),
                _ => Err(AuthError::ReauthRequired),
            },
            _ => Err(AuthError::NotSignedIn),
        }
    }

    /// Opens the emergency view without sign-in (SR-014). Never refused; always audited, with the
    /// station's name as target when it is known.
    pub fn break_glass_open(
        &mut self,
        log: &AuditLog<'_>,
        station: Option<&str>,
    ) -> BreakGlassGrant {
        self.break_glass = true;
        let audit_error = log
            .append(
                &Actor::break_glass(),
                AuditAction::BreakGlassOpened,
                station,
                &json!({}),
            )
            .err();
        BreakGlassGrant { audit_error }
    }

    fn apply_idle_lock(&mut self, log: &AuditLog<'_>) -> Result<(), AuthError> {
        let now = log.clock().monotonic_ns();
        if let Some(s) = &mut self.session {
            let idle = now.saturating_sub(s.last_activity_ns);
            if !s.locked && idle >= self.config.idle_lock_ns {
                s.locked = true;
                s.reauth_at_ns = None;
                log.append(
                    &Actor::user(&s.identity),
                    AuditAction::AutoLock,
                    None,
                    &json!({"idle_s": idle / NS_PER_S}),
                )?;
            }
        }
        Ok(())
    }

    /// Seconds to wait if `key` is throttled now.
    fn throttled(&self, key: &str, now: u64) -> Option<u64> {
        let f = self.failures.get(key)?;
        if f.count < self.config.max_failures {
            return None;
        }
        let doublings = (f.count - self.config.max_failures).min(32);
        let period = self
            .config
            .throttle_base_ns
            .saturating_mul(1u64 << doublings)
            .min(self.config.throttle_max_ns);
        let until = f.last_ns.saturating_add(period);
        (now < until).then(|| (until - now).div_ceil(NS_PER_S))
    }

    fn count_failure(&mut self, key: &str, now: u64) {
        if self.failures.len() >= MAX_TRACKED_NAMES && !self.failures.contains_key(key) {
            if let Some(oldest) = self
                .failures
                .iter()
                .min_by_key(|(_, f)| f.last_ns)
                .map(|(k, _)| k.clone())
            {
                self.failures.remove(&oldest);
            }
        }
        let f = self.failures.entry(key.to_owned()).or_default();
        f.count = f.count.saturating_add(1);
        f.last_ns = now;
    }

    fn fail(
        &mut self,
        log: &AuditLog<'_>,
        key: &str,
        now: u64,
        username: &str,
        role: Option<Role>,
        reason: &str,
    ) -> Result<(), AuthError> {
        self.count_failure(key, now);
        log.append(
            &Actor::claimed(username, role),
            AuditAction::LoginFailed,
            None,
            &json!({"reason": reason}),
        )?;
        Ok(())
    }
}

fn identity_of(record: &caelum_store::UserRecord) -> Result<UserIdentity, SecurityError> {
    let malformed = || SecurityError::Malformed(format!("user {}", record.id));
    Ok(UserIdentity {
        user_id: record.id.parse::<UserId>().map_err(|_| malformed())?,
        username: record.username.clone(),
        role: Role::parse(&record.role).ok_or_else(malformed)?,
        crew_member_id: record
            .crew_member_id
            .as_deref()
            .map(str::parse::<CrewMemberId>)
            .transpose()
            .map_err(|_| malformed())?,
    })
}
