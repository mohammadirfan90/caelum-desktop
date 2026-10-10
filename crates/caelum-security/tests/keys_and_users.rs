//! Keys in the keystore (F-SYS-02, R-SEC-01) and account creation (F-SYS-01).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use caelum_security::audit::Actor;
use caelum_security::keys::{database_key, instance_key, IfMissing, DB_KEY_ENTRY};
use caelum_security::keystore::KeyStore;
use caelum_security::role::Role;
use caelum_security::users::{create_user, NewUser};
use caelum_security::SecurityError;
use caelum_store::Database;
use common::{Fixture, MemoryKeyStore};

/// A key is created once for a new installation and the same key is loaded afterwards.
#[test]
fn keys_are_created_once_and_reloaded() {
    let store = MemoryKeyStore::default();
    let (_, created) = database_key(&store, IfMissing::Create).unwrap();
    assert!(created);
    let (_, created_again) = database_key(&store, IfMissing::Create).unwrap();
    assert!(!created_again);
    let (signing, created) = instance_key(&store, IfMissing::Create).unwrap();
    assert!(created);
    let (reloaded, _) = instance_key(&store, IfMissing::Fail).unwrap();
    assert_eq!(signing.verifying_key(), reloaded.verifying_key());
}

/// The key from the keystore opens the database; the database file alone does not.
#[test]
fn database_opens_with_the_keystore_key_only() {
    let tmp = common::TempDb::new();
    let path = tmp.dir.join("keyed.db");
    let store = MemoryKeyStore::default();
    let (key, _) = database_key(&store, IfMissing::Create).unwrap();
    drop(Database::open(&path, &key).unwrap());
    let (again, _) = database_key(&store, IfMissing::Fail).unwrap();
    Database::open(&path, &again).expect("same key opens");
    let other = MemoryKeyStore::default();
    let (fresh, _) = database_key(&other, IfMissing::Create).unwrap();
    assert!(Database::open(&path, &fresh).is_err());
}

/// An existing installation never silently replaces a missing key.
#[test]
fn missing_key_for_existing_installation_is_an_error() {
    let store = MemoryKeyStore::default();
    assert!(matches!(
        database_key(&store, IfMissing::Fail),
        Err(SecurityError::KeyMissing(DB_KEY_ENTRY))
    ));
    store.set(DB_KEY_ENTRY, &[1, 2, 3]).unwrap();
    assert!(matches!(
        database_key(&store, IfMissing::Fail),
        Err(SecurityError::KeyMalformed(DB_KEY_ENTRY))
    ));
    let dropping = MemoryKeyStore::dropping_writes();
    assert!(matches!(
        database_key(&dropping, IfMissing::Create),
        Err(SecurityError::KeyNotPersisted(DB_KEY_ENTRY))
    ));
}

/// The operating-system store round trip (Windows Credential Manager on the reference platform).
/// Linux CI has no Secret Service daemon (CR-006 §1), so this runs on Windows only.
#[cfg(windows)]
#[test]
fn os_keystore_round_trip() {
    use caelum_security::keystore::OsKeyStore;
    let service = format!("org.chronostrider.caelum.test-{}", std::process::id());
    let store = OsKeyStore::open_with_service(&service).unwrap();
    assert!(store.get("probe").unwrap().is_none());
    store.set("probe", &[9; 32]).unwrap();
    assert_eq!(store.get("probe").unwrap().unwrap().as_slice(), &[9; 32]);
    store.delete("probe").unwrap();
    assert!(store.get("probe").unwrap().is_none());
}

#[test]
fn accounts_follow_the_name_scope_and_secret_policy() {
    let f = Fixture::new();
    let crew = f.crew_member("Crew One");
    let new = |username, role, crew_member_id, secret| NewUser {
        username,
        role,
        crew_member_id,
        secret,
    };
    let log = f.log();
    let sys = Actor::system();
    assert!(create_user(
        &log,
        &sys,
        new("bad name", Role::Bme, None, "a long enough pass")
    )
    .is_err());
    assert!(create_user(&log, &sys, new("astro", Role::Astronaut, None, "246813")).is_err());
    assert!(create_user(
        &log,
        &sys,
        new("fs", Role::FlightSurgeon, Some(crew), "a long enough pass")
    )
    .is_err());
    assert!(create_user(
        &log,
        &sys,
        new("astro", Role::Astronaut, Some(crew), "12345")
    )
    .is_err());
    let id = create_user(
        &log,
        &sys,
        new("Astro", Role::Astronaut, Some(crew), "246813"),
    )
    .unwrap();
    let stored = f.tmp.db.user_by_username("astro").unwrap().unwrap();
    assert_eq!(stored.id, id.to_string());
    assert!(stored.credential.starts_with("$argon2id$"));
    assert!(
        !stored.credential.contains("246813"),
        "only the hash is stored"
    );
    // Names are unique regardless of case.
    assert!(create_user(
        &log,
        &sys,
        new("ASTRO", Role::Astronaut, Some(crew), "246813")
    )
    .is_err());
}
