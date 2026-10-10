//! Crew roster and user administration (F-SYS-07, F-SYS-01; CR-012 §2–3, CR-007 §4): signed
//! rosters only, administrator with recent re-authentication, scoped reads, audited changes.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use caelum_core::security::audit::Actor;
use caelum_core::security::role::{DeniedReason, Role};
use caelum_core::security::session::AuthError;
use caelum_core::security::users::{create_user, NewUser};
use caelum_core::station::{Station, StationError};
use caelum_domain::ids::{CrewMemberId, UserId};
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN_SECRET: &str = "ground station passphrase";
const CREW_A: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a41";
const CREW_B: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a42";

fn fixture(name: &str) -> (Vec<u8>, String) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/roster/");
    (
        std::fs::read(format!("{dir}{name}.toml")).unwrap(),
        std::fs::read_to_string(format!("{dir}{name}.sig.toml")).unwrap(),
    )
}

fn apply(
    station: &mut Station,
    name: &str,
) -> Result<caelum_core::station::RosterApplied, StationError> {
    let (bytes, sig) = fixture(name);
    station.config_apply_crew(&bytes, &sig)
}

fn rejected(r: Result<impl std::fmt::Debug, StationError>) -> String {
    match r {
        Err(StationError::Rejected(m)) => m,
        other => panic!("expected a rejection, got {other:?}"),
    }
}

fn denied(r: Result<impl std::fmt::Debug, StationError>) -> Option<DeniedReason> {
    match r {
        Err(StationError::Denied(d)) => Some(d.reason),
        _ => None,
    }
}

struct Setup {
    _dir: DataDir,
    clock: TestClock,
    station: Station,
}

/// A new station with its first administrator signed in and re-authenticated.
fn admin_station() -> Setup {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    let mut station = open(&dir, &keys, &clock).unwrap();
    station
        .create_first_administrator("admin", ADMIN_SECRET)
        .unwrap();
    station.login("admin", ADMIN_SECRET.as_bytes()).unwrap();
    station.reauth(ADMIN_SECRET.as_bytes()).unwrap();
    Setup {
        _dir: dir,
        clock,
        station,
    }
}

fn crew(id: &str) -> CrewMemberId {
    CrewMemberId::from_uuid(uuid::Uuid::parse_str(id).unwrap())
}

fn actions(station: &mut Station) -> Vec<String> {
    station
        .audit_list()
        .unwrap()
        .into_iter()
        .map(|e| e.action.as_str().to_owned())
        .collect()
}

#[test]
fn f_sys_07_a_signed_roster_becomes_the_roster_in_force_and_is_audited() {
    let mut s = admin_station();
    let applied = apply(&mut s.station, "crew-v1").unwrap();
    assert_eq!(
        (
            applied.version.as_str(),
            applied.members,
            applied.development
        ),
        ("1", 2, true)
    );
    let roster = s.station.crew_list().unwrap();
    assert_eq!(roster.len(), 2);
    assert!(roster.iter().all(|c| c.active && c.mission == "TEST-1"));
    assert!(actions(&mut s.station).contains(&"config.apply".to_owned()));

    // Re-applying the same bundle is harmless; version 2 makes the missing member inactive.
    apply(&mut s.station, "crew-v1").unwrap();
    apply(&mut s.station, "crew-v2").unwrap();
    let roster = s.station.crew_list().unwrap();
    let b = roster.iter().find(|c| c.id == CREW_B).unwrap();
    assert!(!b.active, "removed members stay on record as inactive");
    assert!(roster.iter().find(|c| c.id == CREW_A).unwrap().active);
}

#[test]
fn f_sys_07_unsigned_tampered_or_inconsistent_rosters_are_refused() {
    let mut s = admin_station();
    let (bytes, sig) = fixture("crew-v1");
    let mut tampered = bytes.clone();
    tampered.extend(b"\n");
    assert!(rejected(s.station.config_apply_crew(&tampered, &sig)).contains("signature"));
    assert!(rejected(s.station.config_apply_crew(&bytes, "")).contains("signature"));
    assert!(rejected(apply(&mut s.station, "crew-mismatch")).contains("not the signed version"));
    assert!(rejected(apply(&mut s.station, "crew-duplicate")).contains("listed twice"));
    assert!(s.station.crew_list().unwrap().is_empty(), "nothing applied");

    apply(&mut s.station, "crew-v1").unwrap();
    assert!(rejected(apply(&mut s.station, "crew-v1-other")).contains("already in force"));
}

#[test]
fn role_matrix_4_roster_and_user_changes_need_recent_reauthentication() {
    let mut s = admin_station();
    s.clock.advance_s(121);
    assert!(matches!(
        apply(&mut s.station, "crew-v1"),
        Err(StationError::Auth(AuthError::ReauthRequired))
    ));
    assert!(matches!(
        s.station
            .user_create("astro.a", Role::Astronaut, Some(crew(CREW_A)), "246813"),
        Err(StationError::Auth(AuthError::ReauthRequired))
    ));
    s.station.reauth(ADMIN_SECRET.as_bytes()).unwrap();
    apply(&mut s.station, "crew-v1").unwrap();
}

#[test]
fn cr_007_accounts_belong_to_active_roster_members_and_are_audited() {
    let mut s = admin_station();
    apply(&mut s.station, "crew-v1").unwrap();
    let a = s
        .station
        .user_create("Astro.A", Role::Astronaut, Some(crew(CREW_A)), "246813")
        .unwrap();
    assert!(rejected(
        s.station
            .user_create("astro.a", Role::Cmo, Some(crew(CREW_B)), "135792")
    )
    .contains("taken"));
    assert!(rejected(s.station.user_create(
        "nobody",
        Role::Astronaut,
        Some(CrewMemberId::from_uuid(uuid::Uuid::new_v4())),
        "246813"
    ))
    .contains("roster"));
    assert!(rejected(s.station.user_create("gw", Role::DeviceGateway, None, "x")).contains("keys"));
    // CR-007 policy still applies (crew roles need a crew member).
    assert!(s
        .station
        .user_create("astro.c", Role::Astronaut, None, "246813")
        .is_err());
    let users = s.station.user_list().unwrap();
    let astro = users.iter().find(|u| u.id == a.to_string()).unwrap();
    assert_eq!(
        (astro.username.as_str(), astro.role),
        ("astro.a", Role::Astronaut)
    );
    assert!(actions(&mut s.station).contains(&"user.create".to_owned()));

    // Inactive roster members cannot get new accounts.
    apply(&mut s.station, "crew-v2").unwrap();
    assert!(rejected(
        s.station
            .user_create("cmo.b", Role::Cmo, Some(crew(CREW_B)), "135792")
    )
    .contains("roster"));
}

#[test]
fn cr_012_users_are_disabled_but_never_the_caller_so_never_the_last_administrator() {
    let mut s = admin_station();
    apply(&mut s.station, "crew-v1").unwrap();
    let me = s.station.me().unwrap().identity.user_id;
    assert!(rejected(s.station.user_disable(me)).contains("own account"));
    let astro = s
        .station
        .user_create("astro.a", Role::Astronaut, Some(crew(CREW_A)), "246813")
        .unwrap();
    s.station.user_disable(astro).unwrap();
    assert!(s
        .station
        .user_list()
        .unwrap()
        .iter()
        .any(|u| u.id == astro.to_string() && u.disabled));
    assert!(rejected(
        s.station
            .user_disable(UserId::from_uuid(uuid::Uuid::new_v4()))
    )
    .contains("no such account"));
    assert!(actions(&mut s.station).contains(&"user.disable".to_owned()));

    // A second administrator may disable the first, but not the last one standing.
    let admin2 = s
        .station
        .user_create("admin2", Role::Administrator, None, ADMIN_SECRET)
        .unwrap();
    s.station.logout().unwrap();
    s.station.login("admin2", ADMIN_SECRET.as_bytes()).unwrap();
    s.station.reauth(ADMIN_SECRET.as_bytes()).unwrap();
    s.station.user_disable(me).unwrap();
    // Only admin2 is active now: it is the caller, and the last administrator.
    assert!(rejected(s.station.user_disable(admin2)).contains("own account"));
    let disabled_admin = create_user(
        &s.station.log(),
        &Actor::system(),
        NewUser {
            username: "admin3",
            role: Role::Administrator,
            crew_member_id: None,
            secret: ADMIN_SECRET,
        },
    )
    .unwrap();
    s.station.user_disable(disabled_admin).unwrap();
}

#[test]
fn role_matrix_2_roster_and_accounts_are_scoped_by_role() {
    let mut s = admin_station();
    apply(&mut s.station, "crew-v1").unwrap();
    s.station
        .user_create("astro.a", Role::Astronaut, Some(crew(CREW_A)), "246813")
        .unwrap();
    s.station
        .user_create("mcc.one", Role::Mcc, None, ADMIN_SECRET)
        .unwrap();
    s.station
        .user_create("cmo.b", Role::Cmo, Some(crew(CREW_B)), "135792")
        .unwrap();
    s.station.logout().unwrap();

    s.station.login("astro.a", b"246813").unwrap();
    let own = s.station.crew_list().unwrap();
    assert_eq!(
        own.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        [CREW_A]
    );
    let me = s.station.user_list().unwrap();
    assert_eq!(
        me.iter().map(|u| u.username.as_str()).collect::<Vec<_>>(),
        ["astro.a"]
    );
    assert_eq!(
        denied(apply(&mut s.station, "crew-v1")),
        Some(DeniedReason::Role)
    );
    assert_eq!(
        denied(s.station.user_create("x", Role::Bme, None, ADMIN_SECRET)),
        Some(DeniedReason::Role)
    );
    s.station.logout().unwrap();

    s.station.login("mcc.one", ADMIN_SECRET.as_bytes()).unwrap();
    assert_eq!(denied(s.station.crew_list()), Some(DeniedReason::Role));
    assert_eq!(s.station.user_list().unwrap().len(), 1);
    s.station.logout().unwrap();

    s.station.login("cmo.b", b"135792").unwrap();
    assert_eq!(s.station.crew_list().unwrap().len(), 2);
    assert_eq!(s.station.user_list().unwrap().len(), 4);
    s.station.logout().unwrap();

    assert_eq!(
        denied(s.station.crew_list()),
        Some(DeniedReason::SignInRequired)
    );
}
