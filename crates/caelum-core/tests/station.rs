//! M1 gate: role-matrix tests for every command through the station (F-SYS-01, R-SEC-02), and
//! key handling on first and later starts (F-SYS-02, R-SEC-01).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use caelum_core::security::audit::Actor;
use caelum_core::security::clock::{Clock, ClockError};
use caelum_core::security::keys::{DB_KEY_ENTRY, INSTANCE_KEY_ENTRY};
use caelum_core::security::keystore::{KeyStore, KeyStoreError};
use caelum_core::security::role::{Access, Command, DeniedReason, Role};
use caelum_core::security::session::{AuthError, SessionConfig};
use caelum_core::security::users::{create_user, NewUser};
use caelum_core::security::SecurityError;
use caelum_core::station::{Station, StationError, DATABASE_FILE};
use caelum_domain::ids::CrewMemberId;
use caelum_domain::time::TaiNs;
use zeroize::Zeroizing;

#[derive(Default)]
struct MemoryKeyStore(Mutex<HashMap<String, Vec<u8>>>);

impl KeyStore for MemoryKeyStore {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .map(Zeroizing::new))
    }
    fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeyStoreError> {
        self.0
            .lock()
            .unwrap()
            .insert(name.to_owned(), secret.to_vec());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), KeyStoreError> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

#[derive(Default)]
struct Times {
    tai: AtomicI64,
    mono: AtomicU64,
}

#[derive(Clone, Default)]
struct TestClock(Arc<Times>);

impl TestClock {
    fn advance_s(&self, s: u64) {
        self.0.mono.fetch_add(s * 1_000_000_000, Ordering::SeqCst);
        self.0
            .tai
            .fetch_add(i64::try_from(s).unwrap() * 1_000_000_000, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_tai(&self) -> Result<TaiNs, ClockError> {
        Ok(TaiNs(self.0.tai.load(Ordering::SeqCst)))
    }
    fn monotonic_ns(&self) -> u64 {
        self.0.mono.load(Ordering::SeqCst)
    }
}

/// A data directory removed on drop.
struct DataDir(PathBuf);

impl DataDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("caelum-core-{}", uuid::Uuid::new_v4()));
        Self(dir)
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(dir: &DataDir, keys: &MemoryKeyStore, clock: &TestClock) -> Result<Station, StationError> {
    Station::open(
        &dir.0,
        keys,
        Box::new(clock.clone()),
        Some("station-a".to_owned()),
        SessionConfig::default(),
    )
}

fn secret_for(role: Role) -> &'static str {
    if role.is_onboard_crew() {
        "246813"
    } else {
        "ground station passphrase"
    }
}

fn add_user(station: &Station, username: &str, role: Role) {
    let crew = role.is_onboard_crew().then(|| {
        let id = CrewMemberId::from_uuid(uuid::Uuid::new_v4());
        station
            .db()
            .connection()
            .execute(
                "INSERT INTO crew_member (id, name, role, mission, active) VALUES (?1, ?2, 'crew', 'M-1', 1)",
                (id.to_string(), username),
            )
            .unwrap();
        id
    });
    create_user(
        &station.log(),
        &Actor::system(),
        NewUser {
            username,
            role,
            crew_member_id: crew,
            secret: secret_for(role),
        },
    )
    .unwrap();
}

fn denied(r: Result<impl std::fmt::Debug, StationError>) -> Option<DeniedReason> {
    match r {
        Err(StationError::Denied(d)) => Some(d.reason),
        _ => None,
    }
}

/// Calls every M1 command except sign-in as the current caller; returns which were refused.
fn refused_commands(station: &mut Station, secret: &str) -> Vec<Command> {
    let mut refused = Vec::new();
    let mut check = |command, r: Option<DeniedReason>| {
        if r.is_some() {
            refused.push(command);
        }
    };
    check(Command::AuthMe, denied(station.me()));
    check(
        Command::AuthReauth,
        denied(station.reauth(secret.as_bytes())),
    );
    check(Command::AuditList, denied(station.audit_list()));
    check(Command::AuditVerify, denied(station.audit_verify()));
    check(Command::AuthLogout, denied(station.logout()));
    refused
}

fn expected_refusals(role: Option<Role>) -> Vec<Command> {
    [
        Command::AuthMe,
        Command::AuthReauth,
        Command::AuditList,
        Command::AuditVerify,
        Command::AuthLogout,
    ]
    .into_iter()
    .filter(|c| match (c.access(), role) {
        (Access::Public, _) => false,
        (Access::AnySignedIn, Some(_)) => false,
        (Access::Roles(roles), Some(r)) => !roles.contains(&r),
        (_, None) => true,
    })
    .collect()
}

/// Every command, for every role that can sign in, and without sign-in.
#[test]
fn every_command_is_authorised_per_role_matrix() {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    let mut station = open(&dir, &keys, &clock).unwrap();
    for role in Role::ALL.into_iter().filter(|r| !r.is_machine()) {
        let name = format!("user.{}", role.as_str());
        add_user(&station, &name, role);
        let me = station.login(&name, secret_for(role).as_bytes()).unwrap();
        assert_eq!(me.identity.role, role);
        assert_eq!(
            refused_commands(&mut station, secret_for(role)),
            expected_refusals(Some(role)),
            "{role}"
        );
    }
    // Signed out (the last loop step signed out): everything but the public commands refused.
    assert_eq!(
        refused_commands(&mut station, "anything"),
        expected_refusals(None)
    );
    assert_eq!(
        denied(station.audit_list()),
        Some(DeniedReason::SignInRequired)
    );
    // Break-glass is public and always granted.
    assert!(station.break_glass_open().audit_error.is_none());
    assert_eq!(denied(station.me()), Some(DeniedReason::SignInRequired));
}

#[test]
fn machine_identities_cannot_sign_in_interactively() {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let mut station = open(&dir, &keys, &TestClock::default()).unwrap();
    add_user(&station, "gateway", Role::DeviceGateway);
    assert!(matches!(
        station.login("gateway", secret_for(Role::DeviceGateway).as_bytes()),
        Err(StationError::Auth(AuthError::NotInteractive))
    ));
}

/// A locked session reaches only identity, unlock and sign-out (ROLE_MATRIX.md §4).
#[test]
fn a_locked_session_must_unlock_before_role_commands() {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    let mut station = open(&dir, &keys, &clock).unwrap();
    add_user(&station, "cmo", Role::Cmo);
    station
        .login("cmo", secret_for(Role::Cmo).as_bytes())
        .unwrap();
    clock.advance_s(10 * 60);
    assert!(station.me().unwrap().locked);
    assert_eq!(denied(station.audit_list()), Some(DeniedReason::Locked));
    station.reauth(secret_for(Role::Cmo).as_bytes()).unwrap();
    assert!(!station.me().unwrap().locked);
    assert!(!station.audit_list().unwrap().is_empty());
}

/// Each role sees only its audit categories (ROLE_MATRIX.md §2); every entry so far is a security
/// entry, so the BME sees none and the administrator and CMO see all of them.
#[test]
fn audit_list_is_filtered_by_role() {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let mut station = open(&dir, &keys, &TestClock::default()).unwrap();
    for (name, role) in [
        ("bme", Role::Bme),
        ("admin", Role::Administrator),
        ("cmo", Role::Cmo),
    ] {
        add_user(&station, name, role);
    }
    let mut visible = HashMap::new();
    for (name, role) in [
        ("bme", Role::Bme),
        ("admin", Role::Administrator),
        ("cmo", Role::Cmo),
    ] {
        station.login(name, secret_for(role).as_bytes()).unwrap();
        visible.insert(name, station.audit_list().unwrap().len());
    }
    assert_eq!(visible["bme"], 0);
    assert!(visible["admin"] > 0);
    assert!(visible["cmo"] >= visible["admin"]);
    assert!(station.audit_verify().unwrap().is_intact());
}

/// F-SYS-02: a new installation creates both keys (audited); later starts reuse them; an
/// existing database never gets a fresh key.
#[test]
fn keys_are_created_on_first_start_only() {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    {
        let station = open(&dir, &keys, &clock).unwrap();
        let created: Vec<_> = station
            .db()
            .audit_records()
            .unwrap()
            .into_iter()
            .filter(|r| r.action == "key.create")
            .filter_map(|r| r.target)
            .collect();
        assert_eq!(created, [DB_KEY_ENTRY, INSTANCE_KEY_ENTRY]);
    }
    {
        let station = open(&dir, &keys, &clock).unwrap();
        assert_eq!(
            station.db().audit_records().unwrap().len(),
            2,
            "no new keys"
        );
        assert!(station.log().verify().unwrap().is_intact());
    }
    assert!(dir.0.join(DATABASE_FILE).exists());
    let empty = MemoryKeyStore::default();
    assert!(matches!(
        open(&dir, &empty, &clock),
        Err(StationError::Security(SecurityError::KeyMissing(
            DB_KEY_ENTRY
        )))
    ));
    assert!(
        empty.get(DB_KEY_ENTRY).unwrap().is_none(),
        "nothing created"
    );
}

/// SR-009: the station loads the signed ruleset at start-up; development builds run the
/// development-signed ruleset and report it for the DEVELOPMENT RULESET banner.
#[test]
fn sr_009_the_station_loads_the_ruleset_for_its_build() {
    use caelum_core::alarms::RulesetStatus;
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let station = open(&dir, &keys, &TestClock::default()).unwrap();
    assert!(
        matches!(station.ruleset_status(), RulesetStatus::Verified { development: true, .. }),
        "{:?}",
        station.ruleset_status()
    );
}
