//! The station: this installation's encrypted records, keys and sign-in session, and the M1
//! commands on them (F-SYS-01, F-SYS-02, F-SYS-03). Every command is authorised against
//! `docs/ROLE_MATRIX.md` before it runs (CLAUDE.md rule 6); the IPC layer only translates.

use std::path::Path;

use caelum_alarms::signing::{built_in_keys, TrustPolicy};
use caelum_alarms::{BUNDLED_RULESET, BUNDLED_SIGNATURES};
use caelum_security::audit::{Actor, AuditAction, AuditEntry, AuditLog, ChainStatus};
use caelum_security::clock::Clock;
use caelum_security::keys::{
    database_key, instance_key, IfMissing, InstanceKey, DB_KEY_ENTRY, INSTANCE_KEY_ENTRY,
};
use caelum_security::keystore::KeyStore;
use caelum_security::role::{Command, Denied, Principal, UserIdentity};
use caelum_security::session::{AuthError, BreakGlassGrant, SessionConfig, SessionManager};
use caelum_security::SecurityError;
use caelum_store::Database;
use serde_json::json;

use crate::alarms::{AlarmService, AlarmServiceError, RulesetStatus};

/// File name of the encrypted database inside the data directory.
pub const DATABASE_FILE: &str = "caelum.db";

/// Station errors.
#[derive(Debug, thiserror::Error)]
pub enum StationError {
    /// The caller may not use this command.
    #[error(transparent)]
    Denied(#[from] Denied),
    /// Sign-in or session failure.
    #[error(transparent)]
    Auth(#[from] AuthError),
    /// Storage, keystore, clock or audit failure.
    #[error(transparent)]
    Security(#[from] SecurityError),
    /// The data directory could not be created.
    #[error("data directory: {0}")]
    Io(#[from] std::io::Error),
    /// The alarm service could not record its state.
    #[error(transparent)]
    Alarms(#[from] AlarmServiceError),
}

impl From<caelum_store::StoreError> for StationError {
    fn from(e: caelum_store::StoreError) -> Self {
        StationError::Security(e.into())
    }
}

/// The signed-in user as shown to them (`auth_me`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Me {
    /// Identity, role and scope.
    pub identity: UserIdentity,
    /// The session is locked (unlock with re-authentication).
    pub locked: bool,
}

/// This installation.
pub struct Station {
    db: Database,
    key: InstanceKey,
    clock: Box<dyn Clock>,
    sessions: SessionManager,
    name: Option<String>,
    alarms: AlarmService,
}

impl Station {
    /// Opens the station's records in `data_dir`. A new installation (no database yet) creates
    /// its keys in `keystore` and audits that; an existing one needs its keys and never replaces
    /// them (CR-006 §2).
    pub fn open(
        data_dir: &Path,
        keystore: &dyn KeyStore,
        clock: Box<dyn Clock>,
        name: Option<String>,
        config: SessionConfig,
    ) -> Result<Self, StationError> {
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join(DATABASE_FILE);
        let if_missing = if path.exists() {
            IfMissing::Fail
        } else {
            IfMissing::Create
        };
        let (db_key, db_key_created) = database_key(keystore, if_missing)?;
        let (key, instance_key_created) = instance_key(keystore, if_missing)?;
        let db = Database::open(&path, &db_key)?;
        let now = clock.now_tai().map_err(SecurityError::from)?;
        // SR-009: verified for this build's trust policy, with the safe fallback.
        let alarms = AlarmService::load(
            &db,
            BUNDLED_RULESET,
            BUNDLED_SIGNATURES,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
            now,
        )?;
        let station = Self {
            db,
            key,
            clock,
            sessions: SessionManager::new(config),
            name,
            alarms,
        };
        let key_id = station.key.verifying_key().key_id();
        for (created, entry) in [
            (db_key_created, DB_KEY_ENTRY),
            (instance_key_created, INSTANCE_KEY_ENTRY),
        ] {
            if created {
                station.log().append(
                    &Actor::system(),
                    AuditAction::KeyCreated,
                    Some(entry),
                    &json!({"instance_key_id": key_id}),
                )?;
            }
        }
        Ok(station)
    }

    /// The audit log (for wiring and tests).
    pub fn log(&self) -> AuditLog<'_> {
        AuditLog::new(&self.db, &self.key, &*self.clock)
    }

    /// `ruleset_status` (public): the ruleset in force, for the SR-009 banner.
    pub fn ruleset_status(&self) -> &RulesetStatus {
        self.alarms.status()
    }

    /// The database (for wiring and tests).
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// The session manager and the audit log, borrowed together.
    fn parts(&mut self) -> (&mut SessionManager, AuditLog<'_>) {
        (
            &mut self.sessions,
            AuditLog::new(&self.db, &self.key, &*self.clock),
        )
    }

    /// The caller's principal, after checking that it may use `command`.
    fn authorize(&mut self, command: Command) -> Result<Principal, StationError> {
        let (sessions, log) = self.parts();
        let principal = sessions.principal(&log)?;
        command.authorize(&principal)?;
        Ok(principal)
    }

    /// `auth_login`.
    pub fn login(&mut self, username: &str, secret: &[u8]) -> Result<Me, StationError> {
        self.authorize(Command::AuthLogin)?;
        let (sessions, log) = self.parts();
        let identity = sessions.login(&log, username, secret)?;
        Ok(Me {
            identity,
            locked: false,
        })
    }

    /// `auth_logout`.
    pub fn logout(&mut self) -> Result<(), StationError> {
        self.authorize(Command::AuthLogout)?;
        let (sessions, log) = self.parts();
        sessions.logout(&log)?;
        Ok(())
    }

    /// `auth_me`.
    pub fn me(&mut self) -> Result<Me, StationError> {
        match self.authorize(Command::AuthMe)? {
            Principal::User(identity) => Ok(Me {
                identity,
                locked: false,
            }),
            Principal::Locked(identity) => Ok(Me {
                identity,
                locked: true,
            }),
            Principal::Anonymous | Principal::BreakGlass => {
                Err(StationError::Auth(AuthError::NotSignedIn))
            }
        }
    }

    /// `auth_reauth`: re-authenticates, and unlocks a locked session.
    pub fn reauth(&mut self, secret: &[u8]) -> Result<(), StationError> {
        self.authorize(Command::AuthReauth)?;
        let (sessions, log) = self.parts();
        sessions.reauth(&log, secret)?;
        Ok(())
    }

    /// `break_glass_open` (SR-014): public, so it skips the session check entirely and can never
    /// be refused; an audit failure is reported in the grant.
    pub fn break_glass_open(&mut self) -> BreakGlassGrant {
        let name = self.name.clone();
        let (sessions, log) = self.parts();
        sessions.break_glass_open(&log, name.as_deref())
    }

    /// `audit_list`: the entries the caller's role may read.
    pub fn audit_list(&mut self) -> Result<Vec<AuditEntry>, StationError> {
        let Principal::User(user) = self.authorize(Command::AuditList)? else {
            return Err(StationError::Auth(AuthError::NotSignedIn));
        };
        Ok(self.log().list(user.role.audit_categories())?)
    }

    /// `audit_verify`: chain and signature status only.
    pub fn audit_verify(&mut self) -> Result<ChainStatus, StationError> {
        self.authorize(Command::AuditVerify)?;
        Ok(self.log().verify()?)
    }
}
