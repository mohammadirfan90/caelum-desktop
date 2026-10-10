//! Test doubles for the security tests: a temporary encrypted database, an in-memory keystore and
//! a manual clock. They exist only in the test tree, never in a build.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)] // shared test code

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;

use caelum_domain::ids::CrewMemberId;
use caelum_domain::time::TaiNs;
use caelum_security::audit::{Actor, AuditLog};
use caelum_security::clock::{Clock, ClockError};
use caelum_security::keys::InstanceKey;
use caelum_security::keystore::{KeyStore, KeyStoreError};
use caelum_security::role::Role;
use caelum_security::users::{create_user, NewUser};
use caelum_store::{Database, DbKey};
use zeroize::Zeroizing;

pub const NS_PER_S: u64 = 1_000_000_000;

/// A fresh encrypted database in its own temporary directory. Fields drop in order, so the
/// database is closed before the directory is removed (Windows cannot delete open files).
pub struct TempDb {
    pub db: Database,
    pub dir: PathBuf,
    _guard: DirGuard,
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl TempDb {
    pub fn new() -> Self {
        // A random name: process IDs are reused, and a leftover directory must never be reopened.
        let dir = std::env::temp_dir().join(format!("caelum-security-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let db = Database::open(&dir.join("caelum.db"), &DbKey::from_bytes([0x5a; 32]))
            .expect("open database");
        Self {
            db,
            _guard: DirGuard(dir.clone()),
            dir,
        }
    }
}

/// A clock the test moves by hand.
pub struct ManualClock {
    tai: AtomicI64,
    mono: AtomicU64,
}

impl ManualClock {
    pub fn new() -> Self {
        // 2026-10-10T00:00:00 UTC as TAI ns since 1958 (any fixed value works for the tests).
        Self {
            tai: AtomicI64::new(2_107_382_437_000_000_000),
            mono: AtomicU64::new(1_000 * NS_PER_S),
        }
    }

    pub fn advance_s(&self, s: u64) {
        self.mono.fetch_add(s * NS_PER_S, Ordering::SeqCst);
        self.tai
            .fetch_add(i64::try_from(s).unwrap() * 1_000_000_000, Ordering::SeqCst);
    }

    /// Moves only the wall clock (as a user changing the system time would).
    pub fn set_wall_back_s(&self, s: i64) {
        self.tai.fetch_sub(s * 1_000_000_000, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_tai(&self) -> Result<TaiNs, ClockError> {
        Ok(TaiNs(self.tai.load(Ordering::SeqCst)))
    }

    fn monotonic_ns(&self) -> u64 {
        self.mono.load(Ordering::SeqCst)
    }
}

/// An in-memory keystore.
#[derive(Default)]
pub struct MemoryKeyStore {
    entries: Mutex<HashMap<String, Vec<u8>>>,
    drop_writes: bool,
}

impl MemoryKeyStore {
    /// A store that accepts writes but keeps nothing (a failing platform store).
    pub fn dropping_writes() -> Self {
        Self {
            drop_writes: true,
            ..Self::default()
        }
    }
}

impl KeyStore for MemoryKeyStore {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .map(Zeroizing::new))
    }

    fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeyStoreError> {
        if !self.drop_writes {
            self.entries
                .lock()
                .unwrap()
                .insert(name.to_owned(), secret.to_vec());
        }
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), KeyStoreError> {
        self.entries.lock().unwrap().remove(name);
        Ok(())
    }
}

/// Everything a security test needs.
pub struct Fixture {
    pub tmp: TempDb,
    pub key: InstanceKey,
    pub clock: ManualClock,
}

impl Fixture {
    pub fn new() -> Self {
        Self {
            tmp: TempDb::new(),
            key: InstanceKey::from_seed(&[0x42; 32]),
            clock: ManualClock::new(),
        }
    }

    pub fn log(&self) -> AuditLog<'_> {
        AuditLog::new(&self.tmp.db, &self.key, &self.clock)
    }

    /// Adds a crew member row (onboard accounts belong to one).
    pub fn crew_member(&self, name: &str) -> CrewMemberId {
        let id = CrewMemberId::from_uuid(uuid::Uuid::new_v4());
        self.tmp
            .db
            .connection()
            .execute(
                "INSERT INTO crew_member (id, name, role, mission, active) VALUES (?1, ?2, 'crew', 'M-1', 1)",
                (id.to_string(), name),
            )
            .expect("crew member");
        id
    }

    /// Creates an account with the role's policy-compliant test secret (see [`secret_for`]).
    pub fn user(&self, username: &str, role: Role) {
        let crew = role.is_onboard_crew().then(|| self.crew_member(username));
        create_user(
            &self.log(),
            &Actor::system(),
            NewUser {
                username,
                role,
                crew_member_id: crew,
                secret: secret_for(role),
            },
        )
        .expect("create user");
    }
}

/// Test secret meeting the policy for `role` (PIN for onboard crew, password otherwise).
pub fn secret_for(role: Role) -> &'static str {
    if role.is_onboard_crew() {
        "246813"
    } else {
        "ground station passphrase"
    }
}
