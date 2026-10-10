//! Sign-in, throttling, auto-lock, re-authentication and break-glass (F-SYS-01, R-SEC-02,
//! `docs/ROLE_MATRIX.md` §4, SR-014). Every outcome is audited.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use caelum_security::role::{Principal, Role};
use caelum_security::session::{AuthError, SessionConfig, SessionManager, SessionStatus};
use common::{secret_for, Fixture};

fn actions(f: &Fixture) -> Vec<String> {
    f.tmp
        .db
        .audit_records()
        .unwrap()
        .into_iter()
        .map(|r| r.action)
        .collect()
}

fn signed_in(f: &Fixture, username: &str, role: Role) -> SessionManager {
    f.user(username, role);
    let mut s = SessionManager::new(SessionConfig::default());
    s.login(&f.log(), username, secret_for(role).as_bytes())
        .expect("login");
    s
}

#[test]
fn sign_in_succeeds_with_the_right_secret_and_is_audited() {
    let f = Fixture::new();
    let mut s = signed_in(&f, "cmo.one", Role::Cmo);
    match s.principal(&f.log()).unwrap() {
        Principal::User(u) => {
            assert_eq!(u.username, "cmo.one");
            assert_eq!(u.role, Role::Cmo);
            assert!(u.crew_member_id.is_some());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(actions(&f), ["user.create", "auth.login"]);
    // The sign-in name is not case-sensitive.
    s.logout(&f.log()).unwrap();
    s.login(&f.log(), "CMO.One", secret_for(Role::Cmo).as_bytes())
        .unwrap();
}

#[test]
fn wrong_secret_and_unknown_name_fail_the_same_way_and_are_audited() {
    let f = Fixture::new();
    f.user("fs.one", Role::FlightSurgeon);
    let mut s = SessionManager::new(SessionConfig::default());
    let wrong = s
        .login(&f.log(), "fs.one", b"not the passphrase")
        .unwrap_err();
    let unknown = s.login(&f.log(), "nobody", b"whatever it is").unwrap_err();
    assert!(matches!(wrong, AuthError::InvalidCredentials));
    assert!(matches!(unknown, AuthError::InvalidCredentials));
    assert_eq!(wrong.to_string(), unknown.to_string());
    assert_eq!(s.principal(&f.log()).unwrap(), Principal::Anonymous);
    let records = f.tmp.db.audit_records().unwrap();
    let failed: Vec<_> = records
        .iter()
        .filter(|r| r.action == "auth.login_failed")
        .collect();
    assert_eq!(failed.len(), 2);
    assert_eq!(failed[0].role, "flight_surgeon");
    assert_eq!(failed[1].role, "unknown");
    assert!(
        !failed.iter().any(|r| r.details.contains("passphrase")),
        "secrets never logged"
    );
}

/// `docs/ROLE_MATRIX.md` §3: auth_login is rate-limited.
#[test]
fn repeated_failures_throttle_the_name_then_expire() {
    let f = Fixture::new();
    f.user("bme.one", Role::Bme);
    let mut s = SessionManager::new(SessionConfig::default());
    for _ in 0..5 {
        assert!(matches!(
            s.login(&f.log(), "bme.one", b"wrong wrong wrong"),
            Err(AuthError::InvalidCredentials)
        ));
    }
    // Even the right secret is refused while throttled.
    let right = secret_for(Role::Bme).as_bytes();
    match s.login(&f.log(), "bme.one", right) {
        Err(AuthError::Throttled { retry_after_s }) => assert_eq!(retry_after_s, 30),
        other => panic!("{other:?}"),
    }
    f.clock.advance_s(31);
    s.login(&f.log(), "bme.one", right)
        .expect("allowed after the period");
    assert!(actions(&f).iter().any(|a| a == "auth.login"));
}

#[test]
fn throttle_period_doubles_and_is_capped() {
    let f = Fixture::new();
    f.user("mcc.one", Role::Mcc);
    let mut s = SessionManager::new(SessionConfig::default());
    let mut last = 0;
    for _ in 0..12 {
        loop {
            match s.login(&f.log(), "mcc.one", b"wrong wrong wrong") {
                Err(AuthError::Throttled { retry_after_s }) => {
                    last = retry_after_s;
                    f.clock.advance_s(retry_after_s);
                }
                Err(AuthError::InvalidCredentials) => break,
                other => panic!("{other:?}"),
            }
        }
    }
    assert_eq!(last, 15 * 60, "capped at 15 minutes");
}

/// `docs/ROLE_MATRIX.md` §4: auto-lock after 10 minutes; the same user unlocks by re-auth.
#[test]
fn idle_session_locks_and_unlocks_with_reauth() {
    let f = Fixture::new();
    let mut s = signed_in(&f, "astro.one", Role::Astronaut);
    f.clock.advance_s(9 * 60 + 59);
    assert!(matches!(
        s.status(&f.log()).unwrap(),
        SessionStatus::Active(_)
    ));
    // Activity resets the idle timer.
    s.principal(&f.log()).unwrap();
    f.clock.advance_s(9 * 60);
    assert!(matches!(
        s.status(&f.log()).unwrap(),
        SessionStatus::Active(_)
    ));
    f.clock.advance_s(60);
    assert!(matches!(
        s.status(&f.log()).unwrap(),
        SessionStatus::Locked(_)
    ));
    assert!(matches!(
        s.principal(&f.log()).unwrap(),
        Principal::Locked(_)
    ));
    assert!(actions(&f).iter().any(|a| a == "auth.auto_lock"));

    assert!(s.reauth(&f.log(), b"000000").is_err());
    s.reauth(&f.log(), secret_for(Role::Astronaut).as_bytes())
        .unwrap();
    assert!(matches!(s.principal(&f.log()).unwrap(), Principal::User(_)));
}

/// Setting the wall clock back does not extend a session: timing is monotonic.
#[test]
fn wall_clock_changes_do_not_extend_a_session() {
    let f = Fixture::new();
    let mut s = signed_in(&f, "astro.two", Role::Astronaut);
    f.clock.advance_s(10 * 60);
    f.clock.set_wall_back_s(3600);
    assert!(matches!(
        s.status(&f.log()).unwrap(),
        SessionStatus::Locked(_)
    ));
}

/// `docs/ROLE_MATRIX.md` §4: sensitive actions need a re-auth within the last 2 minutes.
#[test]
fn reauth_is_valid_for_two_minutes() {
    let f = Fixture::new();
    let mut s = signed_in(&f, "admin.one", Role::Administrator);
    s.require_recent_reauth(&f.log()).expect("sign-in counts");
    f.clock.advance_s(121);
    assert!(matches!(
        s.require_recent_reauth(&f.log()),
        Err(AuthError::ReauthRequired)
    ));
    s.reauth(&f.log(), secret_for(Role::Administrator).as_bytes())
        .unwrap();
    s.require_recent_reauth(&f.log()).unwrap();
    s.logout(&f.log()).unwrap();
    assert!(matches!(
        s.require_recent_reauth(&f.log()),
        Err(AuthError::NotSignedIn)
    ));
}

/// SR-014: break-glass needs no sign-in, always opens, and is audited.
#[test]
fn break_glass_opens_without_sign_in_and_is_audited() {
    let f = Fixture::new();
    let mut s = SessionManager::new(SessionConfig::default());
    let grant = s.break_glass_open(&f.log(), Some("station-a"));
    assert!(grant.audit_error.is_none());
    assert_eq!(s.principal(&f.log()).unwrap(), Principal::BreakGlass);
    let records = f.tmp.db.audit_records().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].action, "break_glass.open");
    assert_eq!(records[0].target.as_deref(), Some("station-a"));
}

#[test]
fn disabled_and_machine_accounts_cannot_sign_in() {
    let f = Fixture::new();
    f.user("gateway.one", Role::DeviceGateway);
    f.user("fs.two", Role::FlightSurgeon);
    f.tmp
        .db
        .connection()
        .execute("UPDATE user SET disabled = 1 WHERE username = 'fs.two'", [])
        .unwrap();
    let mut s = SessionManager::new(SessionConfig::default());
    assert!(matches!(
        s.login(
            &f.log(),
            "gateway.one",
            secret_for(Role::DeviceGateway).as_bytes()
        ),
        Err(AuthError::NotInteractive)
    ));
    assert!(matches!(
        s.login(
            &f.log(),
            "fs.two",
            secret_for(Role::FlightSurgeon).as_bytes()
        ),
        Err(AuthError::AccountDisabled)
    ));
    // A disabled account with the wrong secret looks like any wrong secret.
    assert!(matches!(
        s.login(&f.log(), "fs.two", b"wrong wrong wrong"),
        Err(AuthError::InvalidCredentials)
    ));
}

#[test]
fn a_new_sign_in_replaces_the_session_and_logout_is_audited() {
    let f = Fixture::new();
    let mut s = signed_in(&f, "cmo.two", Role::Cmo);
    f.user("astro.three", Role::Astronaut);
    s.login(
        &f.log(),
        "astro.three",
        secret_for(Role::Astronaut).as_bytes(),
    )
    .unwrap();
    s.logout(&f.log()).unwrap();
    assert!(matches!(s.logout(&f.log()), Err(AuthError::NotSignedIn)));
    let records = f.tmp.db.audit_records().unwrap();
    let logouts: Vec<_> = records
        .iter()
        .filter(|r| r.action == "auth.logout")
        .collect();
    assert_eq!(logouts.len(), 2);
    assert!(logouts[0].details.contains("replaced"));
    assert!(f.log().verify().unwrap().is_intact());
}
