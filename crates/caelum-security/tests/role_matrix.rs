//! M1 gate: role-matrix tests for every command against `docs/ROLE_MATRIX.md` (F-SYS-01,
//! R-SEC-02). The expected access is read from the approved document, not restated here.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use caelum_domain::ids::UserId;
use caelum_security::audit::AuditCategory;
use caelum_security::role::{Access, Command, Principal, Role, UserIdentity};

const MATRIX: &str = include_str!("../../../docs/ROLE_MATRIX.md");

/// Rows of the markdown table in the section starting with `heading`.
fn table_rows(heading: &str) -> Vec<Vec<String>> {
    let start = MATRIX.find(heading).expect("section");
    MATRIX[start..]
        .lines()
        .skip(1)
        .skip_while(|l| !l.starts_with('|'))
        .take_while(|l| l.starts_with('|'))
        .skip(2) // header and separator
        .map(|l| {
            l.trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_owned())
                .collect()
        })
        .collect()
}

fn role_from_label(label: &str) -> Role {
    match label.trim() {
        "Astronaut" => Role::Astronaut,
        "CMO" => Role::Cmo,
        "Flight Surgeon" => Role::FlightSurgeon,
        "BME" => Role::Bme,
        "MCC" => Role::Mcc,
        "Admin" => Role::Administrator,
        "Gateway" => Role::DeviceGateway,
        other => panic!("unknown role label {other:?}"),
    }
}

/// §3 "Roles allowed" text to an access rule.
fn access_from_text(text: &str) -> (Access, Vec<Role>) {
    let t = text.to_lowercase();
    if t.starts_with("public") || t == "no login" {
        return (Access::Public, Vec::new());
    }
    if t == "any signed-in user" {
        return (Access::AnySignedIn, Vec::new());
    }
    // "CMO, Flight Surgeon (all); BME (technical); Admin (configuration and security)"
    let roles = text
        .split([',', ';'])
        .map(|part| part.split('(').next().unwrap_or(part))
        .filter(|p| !p.trim().is_empty())
        .map(role_from_label)
        .collect();
    (Access::Roles(&[]), roles)
}

fn identity(role: Role) -> UserIdentity {
    UserIdentity {
        user_id: UserId::from_uuid(uuid::Uuid::nil()),
        username: role.as_str().to_owned(),
        role,
        crew_member_id: None,
    }
}

/// Anonymous, break-glass, and every role both unlocked and locked.
fn principals() -> Vec<Principal> {
    let mut p = vec![Principal::Anonymous, Principal::BreakGlass];
    p.extend(Role::ALL.map(|r| Principal::User(identity(r))));
    p.extend(Role::ALL.map(|r| Principal::Locked(identity(r))));
    p
}

/// Every command in §3 exists in code and every code command is in §3, with the same access.
#[test]
fn every_command_matches_role_matrix_section_3() {
    let rows = table_rows("## 3. Commands");
    let mut documented = Vec::new();
    for row in &rows {
        let name = row[0].trim_matches('`');
        let command = Command::from_name(name)
            .unwrap_or_else(|| panic!("{name} is in ROLE_MATRIX.md §3 but not in code"));
        documented.push(command);
        let (expected, roles) = access_from_text(&row[1]);
        match (command.access(), expected) {
            (Access::Roles(actual), Access::Roles(_)) => {
                let mut actual = actual.to_vec();
                let mut roles = roles;
                actual.sort_by_key(|r| r.as_str());
                roles.sort_by_key(|r| r.as_str());
                assert_eq!(actual, roles, "{name}");
            }
            (actual, expected) => assert_eq!(actual, expected, "{name}"),
        }
    }
    for command in Command::ALL {
        assert!(
            documented.contains(&command),
            "{} is in code but not in ROLE_MATRIX.md §3",
            command.name()
        );
    }
}

/// Every command, for every principal: allowed exactly when its access rule says so.
#[test]
fn authorize_every_command_for_every_principal() {
    for command in Command::ALL {
        for principal in principals() {
            let expected = match (command.access(), &principal) {
                (Access::Public, _) => true,
                (Access::AnySignedIn, Principal::User(_) | Principal::Locked(_)) => true,
                (Access::Roles(roles), Principal::User(u)) => roles.contains(&u.role),
                _ => false,
            };
            assert_eq!(
                command.authorize(&principal).is_ok(),
                expected,
                "{} for {principal:?}",
                command.name()
            );
        }
    }
}

/// SR-014: break-glass needs no sign-in; it does not grant any signed-in command.
#[test]
fn break_glass_opens_without_sign_in_but_grants_nothing_else() {
    assert!(Command::BreakGlassOpen
        .authorize(&Principal::Anonymous)
        .is_ok());
    for command in Command::ALL {
        if command.access() != Access::Public {
            assert!(
                command.authorize(&Principal::BreakGlass).is_err(),
                "{}",
                command.name()
            );
        }
    }
}

/// ROLE_MATRIX.md §4: a locked session can only see who is signed in, unlock or sign out; every
/// role-restricted command needs an unlocked session.
#[test]
fn a_locked_session_reaches_only_the_session_commands() {
    let session_commands = [Command::AuthMe, Command::AuthReauth, Command::AuthLogout];
    for role in Role::ALL {
        let locked = Principal::Locked(identity(role));
        for command in Command::ALL {
            let expected =
                command.access() == Access::Public || session_commands.contains(&command);
            assert_eq!(
                command.authorize(&locked).is_ok(),
                expected,
                "{} {role}",
                command.name()
            );
        }
    }
}

/// §2 "Audit log" row: which entries each role reads.
#[test]
fn audit_visibility_matches_role_matrix_section_2() {
    let rows = table_rows("## 2. Data access");
    let header_start = MATRIX.find("## 2. Data access").unwrap();
    let header: Vec<String> = MATRIX[header_start..]
        .lines()
        .find(|l| l.starts_with("| Data"))
        .unwrap()
        .trim_matches('|')
        .split('|')
        .map(|c| c.trim().to_owned())
        .collect();
    let audit = rows
        .iter()
        .find(|r| r[0] == "Audit log")
        .expect("audit row");
    for (label, cell) in header.iter().zip(audit).skip(1) {
        let role = role_from_label(label);
        let cell = cell.to_lowercase();
        let expected: Vec<AuditCategory> = if cell == "—" {
            vec![]
        } else if cell.contains("all entries") {
            AuditCategory::ALL.to_vec()
        } else if cell.contains("technical") {
            vec![AuditCategory::Technical]
        } else if cell.contains("configuration and security") {
            vec![AuditCategory::Configuration, AuditCategory::Security]
        } else {
            panic!("unrecognised audit cell {cell:?} for {label}");
        };
        assert_eq!(role.audit_categories(), expected.as_slice(), "{label}");
    }
}

#[test]
fn role_names_round_trip_and_match_the_schema() {
    let schema = include_str!("../../caelum-store/migrations/V1__initial_schema.sql");
    for role in Role::ALL {
        assert_eq!(Role::parse(role.as_str()), Some(role));
        assert!(schema.contains(&format!("'{}'", role.as_str())), "{role}");
    }
    assert_eq!(Role::parse("superuser"), None);
}

/// §2 "Alarm actions: acknowledge, silence" (F-ALM-05): own / all / ground stage only / none,
/// read from the document.
#[test]
fn alarm_actions_match_role_matrix_section_2() {
    use caelum_domain::ids::CrewMemberId;
    use caelum_security::role::{alarm_authority, AlarmAuthority};
    let rows = table_rows("## 2. Data access");
    let header_start = MATRIX.find("## 2. Data access").unwrap();
    let header: Vec<String> = MATRIX[header_start..]
        .lines()
        .find(|l| l.starts_with("| Data"))
        .unwrap()
        .trim_matches('|')
        .split('|')
        .map(|c| c.trim().to_owned())
        .collect();
    let row = rows
        .iter()
        .find(|r| r[0].starts_with("Alarm actions"))
        .expect("alarm actions row");
    let mine = CrewMemberId::from_uuid(uuid::Uuid::from_u128(1));
    let theirs = CrewMemberId::from_uuid(uuid::Uuid::from_u128(2));
    for (label, cell) in header.iter().zip(row).skip(1) {
        let role = role_from_label(label);
        let user = UserIdentity {
            crew_member_id: Some(mine),
            ..identity(role)
        };
        let (own, other) = match cell.as_str() {
            "own" => (AlarmAuthority::Onboard, AlarmAuthority::None),
            "all" => (AlarmAuthority::Onboard, AlarmAuthority::Onboard),
            c if c.starts_with("ground stage only") => {
                (AlarmAuthority::GroundStage, AlarmAuthority::GroundStage)
            }
            "—" => (AlarmAuthority::None, AlarmAuthority::None),
            other => panic!("unrecognised alarm-actions cell {other:?} for {label}"),
        };
        assert_eq!(alarm_authority(&user, mine), own, "{label} own");
        assert_eq!(alarm_authority(&user, theirs), other, "{label} other");
    }
}
