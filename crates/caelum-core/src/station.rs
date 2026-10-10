//! The station: this installation's encrypted records, keys and sign-in session, and the M1
//! commands on them (F-SYS-01, F-SYS-02, F-SYS-03). Every command is authorised against
//! `docs/ROLE_MATRIX.md` before it runs (CLAUDE.md rule 6); the IPC layer only translates.

use std::path::{Path, PathBuf};

use caelum_alarms::signing::{built_in_keys, TrustPolicy};
use caelum_alarms::{BUNDLED_RULESET, BUNDLED_SIGNATURES};
use caelum_domain::ids::AlarmId;
use caelum_security::audit::{Actor, AuditAction, AuditEntry, AuditLog, ChainStatus};
use caelum_security::clock::Clock;
use caelum_security::keys::{
    database_key, instance_key, IfMissing, InstanceKey, DB_KEY_ENTRY, INSTANCE_KEY_ENTRY,
};
use caelum_security::keystore::KeyStore;
use caelum_security::role::{
    alarm_authority, AlarmAuthority, Command, Denied, DeniedReason, Principal, UserIdentity,
};
use caelum_security::session::{AuthError, BreakGlassGrant, SessionConfig, SessionManager};
use caelum_security::SecurityError;
use caelum_store::Database;
use serde_json::json;

use crate::alarms::{AlarmService, AlarmServiceError, Output, RulesetStatus};

mod admin;
pub mod clinical;
pub use clinical::{CrewOverviewItem, PvtResult, PROTOCOLS_KIND};
pub mod pathb;
pub mod reads;
mod runtime;
pub mod vehicle;
pub use admin::{RosterApplied, UserSummary, CREW_KIND};
pub use runtime::{Ingested, BLE_SIG_ADAPTER, RUNTIME_ADAPTERS};

/// How an acknowledgement was applied.
#[derive(Debug, Clone, PartialEq)]
pub enum Acknowledged {
    /// Onboard: escalation and sound stopped.
    Onboard(Output),
    /// Ground stage only (flight surgeon): recorded; onboard escalation continues.
    GroundStage,
}

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
    /// The request was refused for the reason given (invalid input, signature, policy).
    #[error("{0}")]
    Rejected(String),
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
    data_dir: PathBuf,
    self_test: Vec<Check>,
    monitor: caelum_alarms::technical::Monitor,
    watch: Watch,
    acquisition: crate::acquisition::Acquisition,
    attachments: caelum_security::attachments::AttachmentStore,
    path_b: Option<(String, pathb::PathB)>,
    /// Advisory models (F-ML-01, CR-018). Empty until signed packages are installed; outputs
    /// are advisory only and never reach the rules except through the observe-only gate.
    models: caelum_ml::registry::Registry,
}

/// Watchdog state between ticks (F-SYS-09).
#[derive(Debug, Default)]
struct Watch {
    /// Last (TAI, monotonic) seen, to detect the wall clock going backwards (SR-012).
    last: Option<(i64, u64)>,
    /// Monotonic time of the last storage check.
    storage_checked: Option<u64>,
}

/// Seconds between storage watchdog checks.
pub const STORAGE_CHECK_S: u64 = 10;

/// One self-test check (F-SYS-09).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Check name (`database`, `audit_chain`, `ruleset`, `clock`, `storage`).
    pub name: &'static str,
    /// Passed.
    pub ok: bool,
    /// What was found.
    pub detail: String,
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
        let mut station = Self {
            db,
            key,
            clock,
            sessions: SessionManager::new(config),
            name,
            alarms,
            data_dir: data_dir.to_path_buf(),
            self_test: Vec::new(),
            monitor: caelum_alarms::technical::Monitor::new(
                caelum_alarms::technical::Limits::DEFAULT,
            ),
            watch: Watch::default(),
            attachments: caelum_security::attachments::AttachmentStore::open(
                &data_dir.join("attachments"),
                caelum_security::attachments::attachment_key(keystore)?,
            )
            .map_err(|e| StationError::Rejected(e.to_string()))?,
            path_b: None,
            models: caelum_ml::registry::Registry::new(),
            acquisition: crate::acquisition::Acquisition::new()
                .map_err(|e| StationError::Rejected(format!("leap-second table: {e}")))?,
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
        // Path B (SR-015): opened from the signed ground configuration in force. A stored
        // configuration that no longer verifies leaves path B closed and raises its alarm.
        if station.open_path_b().is_err() {
            let now = station.now()?;
            station.alarms.technical(
                &station.db,
                pathb::PATH_B_RULE,
                None,
                Some(json!({ "path_b": "the stored ground configuration could not be opened" })),
                now,
            )?;
        }
        // F-SYS-09: start-up self-test; failures raise technical alarms.
        station.run_self_test(caelum_alarms::technical::Limits::DEFAULT.storage_low_bytes)?;
        Ok(station)
    }

    /// The last self-test result.
    pub fn self_test_report(&self) -> &[Check] {
        &self.self_test
    }

    /// Runs the self-test (F-SYS-09): database integrity, audit chain, ruleset in force, clock and
    /// leap-second table, free storage. Each failed check raises `technical.self_test.<name>`;
    /// a passing check clears it. Returns the checks.
    pub fn run_self_test(&mut self, storage_low_bytes: u64) -> Result<Vec<Check>, StationError> {
        let mut checks = Vec::new();
        let db = self.db.quick_check();
        checks.push(Check {
            name: "database",
            ok: db.is_ok(),
            detail: db.err().unwrap_or_else(|| "integrity ok".to_owned()),
        });
        let chain = self.log().verify()?;
        checks.push(Check {
            name: "audit_chain",
            ok: chain.is_intact(),
            detail: format!(
                "{} entries, {} problems",
                chain.entries,
                chain.problems.len()
            ),
        });
        let ruleset = match self.alarms.status() {
            RulesetStatus::Verified { version, .. } => {
                (true, format!("ruleset {version} verified"))
            }
            RulesetStatus::Fallback {
                version, reason, ..
            } => (false, format!("fallback to {version}: {reason}")),
            RulesetStatus::SafeState { reason } => {
                (false, format!("no verified ruleset: {reason}"))
            }
        };
        checks.push(Check {
            name: "ruleset",
            ok: ruleset.0,
            detail: ruleset.1,
        });
        let clock = match (
            self.clock.now_tai(),
            caelum_domain::time::LeapSeconds::bundled(),
        ) {
            (Ok(now), Ok(leap)) => match leap.tai_to_utc(now) {
                Ok((_, caelum_domain::time::Validity::Exact)) => {
                    (true, "clock set, leap-second table current".to_owned())
                }
                Ok((_, caelum_domain::time::Validity::BeyondTable)) => (
                    false,
                    "leap-second table expired: update it by signed configuration".to_owned(),
                ),
                Err(e) => (false, format!("clock: {e}")),
            },
            (Err(e), _) => (false, format!("clock: {e}")),
            (_, Err(e)) => (false, format!("leap-second table: {e}")),
        };
        checks.push(Check {
            name: "clock",
            ok: clock.0,
            detail: clock.1,
        });
        let storage = match fs4::available_space(&self.data_dir) {
            Ok(free) => (
                free >= storage_low_bytes,
                format!("{free} bytes free (limit {storage_low_bytes})"),
            ),
            Err(e) => (false, format!("free space unknown: {e}")),
        };
        checks.push(Check {
            name: "storage",
            ok: storage.0,
            detail: storage.1,
        });
        // With a broken clock the alarm is still raised; its time is then marked unknown (the
        // stored instant is the epoch, never presented as a real time).
        let (now, time_known) = match self.clock.now_tai() {
            Ok(t) => (t, true),
            Err(_) => (caelum_domain::time::TaiNs(0), false),
        };
        for c in &checks {
            let rule = format!("technical.self_test.{}", c.name);
            let why = (!c.ok)
                .then(|| json!({ "check": c.name, "detail": c.detail, "time_known": time_known }));
            self.alarms.technical(&self.db, &rule, None, why, now)?;
        }
        self.self_test = checks.clone();
        Ok(checks)
    }

    /// The audit log (for wiring and tests).
    pub fn log(&self) -> AuditLog<'_> {
        AuditLog::new(&self.db, &self.key, &*self.clock)
    }

    /// The signed-in user allowed to use `command` on `alarm`, with their authority over it.
    fn alarm_actor(
        &mut self,
        command: Command,
        alarm: AlarmId,
    ) -> Result<(UserIdentity, AlarmAuthority), StationError> {
        let Principal::User(user) = self.authorize(command)? else {
            return Err(StationError::Auth(AuthError::NotSignedIn));
        };
        let crew = self
            .alarms
            .crew_of(alarm)
            .ok_or(StationError::Alarms(AlarmServiceError::UnknownAlarm))?;
        let authority = alarm_authority(&user, crew);
        if authority == AlarmAuthority::None {
            return Err(StationError::Denied(Denied {
                command,
                reason: DeniedReason::Role,
            }));
        }
        Ok((user, authority))
    }

    fn now(&self) -> Result<caelum_domain::time::TaiNs, StationError> {
        Ok(self.clock.now_tai().map_err(SecurityError::from)?)
    }

    /// `setup_status` (public): whether the installation still needs its first administrator.
    pub fn needs_first_administrator(&self) -> Result<bool, StationError> {
        Ok(self.db.user_count()? == 0)
    }

    /// `setup_create_administrator` (public, CR-007 §4): creates the first administrator, only
    /// while no account exists; audited with actor `system`. Refused afterwards.
    pub fn create_first_administrator(
        &mut self,
        username: &str,
        secret: &str,
    ) -> Result<(), StationError> {
        self.authorize(Command::SetupCreateAdministrator)?;
        if !self.needs_first_administrator()? {
            return Err(StationError::Denied(Denied {
                command: Command::SetupCreateAdministrator,
                reason: DeniedReason::Role,
            }));
        }
        caelum_security::users::create_user(
            &self.log(),
            &Actor::system(),
            caelum_security::users::NewUser {
                username,
                role: caelum_security::role::Role::Administrator,
                crew_member_id: None,
                secret,
            },
        )?;
        Ok(())
    }

    /// `alarm_acknowledge` (F-ALM-05): onboard for the astronaut (own) and the CMO; the flight
    /// surgeon's acknowledgement counts for the ground stage only (ROLE_MATRIX §5.4). Audited.
    pub fn alarm_acknowledge(&mut self, alarm: AlarmId) -> Result<Acknowledged, StationError> {
        let (user, authority) = self.alarm_actor(Command::AlarmAcknowledge, alarm)?;
        let now = self.now()?;
        let by = user.user_id.to_string();
        let result = if authority == AlarmAuthority::GroundStage {
            self.alarms.acknowledge_ground(&self.db, alarm, &by, now)?;
            Acknowledged::GroundStage
        } else {
            Acknowledged::Onboard(self.alarms.acknowledge(&self.db, alarm, &by, now)?)
        };
        let action = match result {
            Acknowledged::GroundStage => AuditAction::AlarmGroundAcknowledged,
            Acknowledged::Onboard(_) => AuditAction::AlarmAcknowledged,
        };
        self.log().append(
            &Actor::user(&user),
            action,
            Some(&alarm.to_string()),
            &json!({}),
        )?;
        Ok(result)
    }

    /// `alarm_silence` (SR-008): for up to 2 min (critical) or 10 min (warning). Audited with the
    /// requested and applied durations.
    pub fn alarm_silence(
        &mut self,
        alarm: AlarmId,
        seconds: u32,
    ) -> Result<(Output, u32), StationError> {
        let (user, authority) = self.alarm_actor(Command::AlarmSilence, alarm)?;
        if authority != AlarmAuthority::Onboard {
            return Err(StationError::Denied(Denied {
                command: Command::AlarmSilence,
                reason: DeniedReason::Role,
            }));
        }
        let now = self.now()?;
        let (out, applied) = self.alarms.silence(&self.db, alarm, seconds, now)?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::AlarmSilenced,
            Some(&alarm.to_string()),
            &json!({ "requested_s": seconds, "applied_s": applied }),
        )?;
        Ok((out, applied))
    }

    /// `alarm_escalate` (F-ALM-05): the next stage now. Audited.
    pub fn alarm_escalate(&mut self, alarm: AlarmId) -> Result<Output, StationError> {
        let (user, authority) = self.alarm_actor(Command::AlarmEscalate, alarm)?;
        if authority != AlarmAuthority::Onboard {
            return Err(StationError::Denied(Denied {
                command: Command::AlarmEscalate,
                reason: DeniedReason::Role,
            }));
        }
        let now = self.now()?;
        let out = self.alarms.escalate(&self.db, alarm, now)?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::AlarmEscalated,
            Some(&alarm.to_string()),
            &json!({}),
        )?;
        Ok(out)
    }

    /// Evaluates a stored observation in the ingest path (docs/03 §3 step 5) and refreshes the
    /// technical monitor (staleness, battery).
    pub fn observe(&mut self, obs: &caelum_ingest::Observation) -> Result<Output, StationError> {
        let value = match obs.value {
            caelum_ingest::Value::Number(v) => Some(v),
            caelum_ingest::Value::State(_) => None,
        };
        let changes = self.monitor.observation(
            obs.crew_member_id,
            Some(obs.device_id),
            obs.parameter,
            value,
            obs.measured_at,
        );
        self.apply_technical(changes, obs.measured_at)?;
        Ok(self.alarms.observe(&self.db, obs, None)?)
    }

    fn apply_technical(
        &mut self,
        changes: Vec<caelum_alarms::technical::Change>,
        now: caelum_domain::time::TaiNs,
    ) -> Result<(), StationError> {
        for c in changes {
            self.alarms
                .technical(&self.db, &c.rule_id, c.crew, c.raise, now)?;
        }
        Ok(())
    }

    /// The wristbands an alarm's alert goes to (F-ALM-04, WX-002): devices registered with the
    /// wristband adapter and assigned to the alarm's crew member. Empty when there is none; the
    /// crew stations sound either way (SR-015).
    pub fn wristband_targets(&self, alarm: AlarmId) -> Result<Vec<String>, StationError> {
        let Some(crew) = self.alarms.crew_of(alarm) else {
            return Ok(Vec::new());
        };
        let crew = crew.to_string();
        Ok(self
            .db
            .devices()?
            .into_iter()
            .filter(|d| {
                d.adapter == caelum_devices::ble::WRISTBAND_ADAPTER
                    && d.crew_member_id.as_deref() == Some(crew.as_str())
            })
            .map(|d| d.id)
            .collect())
    }

    /// Sounds to resume after a restart (SR-007), delivered once.
    pub fn take_restored(&mut self) -> Output {
        Output {
            events: Vec::new(),
            notices: self.alarms.take_restored(),
        }
    }

    /// The 4 Hz tick (docs/03 §3): alarm engine and escalation, technical monitor, and the
    /// watchdogs (F-SYS-09): clock going backwards (SR-012) and free storage every
    /// [`STORAGE_CHECK_S`] seconds. Alarms never depend on sign-in, so nothing here is authorised.
    pub fn tick(&mut self) -> Result<Output, StationError> {
        let now = self.now()?;
        let mono = self.clock.monotonic_ns();
        let backwards = self.watch.last.is_some_and(|(t, _)| now.0 < t);
        self.watch.last = Some((now.0, mono));
        let mut changes = self.monitor.clock(None, None, backwards);
        changes.extend(self.monitor.tick(now));
        let storage_due = self
            .watch
            .storage_checked
            .is_none_or(|m| mono.saturating_sub(m) >= STORAGE_CHECK_S * 1_000_000_000);
        if storage_due {
            self.watch.storage_checked = Some(mono);
            match fs4::available_space(&self.data_dir) {
                Ok(free) => changes.extend(self.monitor.storage(free)),
                Err(_) => changes.extend(self.monitor.storage(0)),
            }
        }
        self.apply_technical(changes, now)?;
        let out = self.alarms.tick(&self.db, now)?;
        // SR-015: the ground stage goes to the vehicle on path B, never via the medical system.
        self.publish_ops(&out.notices, now)?;
        Ok(out)
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

    /// `auth_me`: who is signed in and whether the session is locked. Reading the state is not
    /// activity, so a window polling it cannot keep a session from auto-locking (ROLE_MATRIX §4).
    pub fn me(&mut self) -> Result<Me, StationError> {
        let (sessions, log) = self.parts();
        match sessions.status(&log)? {
            caelum_security::session::SessionStatus::Active(identity) => Ok(Me {
                identity,
                locked: false,
            }),
            caelum_security::session::SessionStatus::Locked(identity) => Ok(Me {
                identity,
                locked: true,
            }),
            caelum_security::session::SessionStatus::SignedOut => {
                Err(StationError::Denied(Denied {
                    command: Command::AuthMe,
                    reason: DeniedReason::SignInRequired,
                }))
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
