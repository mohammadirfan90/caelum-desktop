//! M1 gate: audit tampering (any field, time included) is detected (F-SYS-03, R-SEC-03).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use caelum_security::audit::{
    record_hash, verify_records, Actor, AuditAction, AuditCategory, ProblemKind,
};
use caelum_security::keys::{hex, InstanceKey};
use caelum_security::role::Role;
use common::Fixture;
use serde_json::json;

/// Three entries, then the update/delete triggers removed, as an attacker with database access
/// could do. The chain must still expose every change.
fn tamper_fixture() -> Fixture {
    let f = Fixture::new();
    let log = f.log();
    for (i, action) in [AuditAction::Login, AuditAction::Reauth, AuditAction::Logout]
        .into_iter()
        .enumerate()
    {
        f.clock.advance_s(1);
        log.append(
            &Actor::system(),
            action,
            Some(&format!("target-{i}")),
            &json!({"n": i}),
        )
        .expect("append");
    }
    f.tmp
        .db
        .connection()
        .execute_batch("DROP TRIGGER audit_entry_no_update; DROP TRIGGER audit_entry_no_delete;")
        .expect("drop triggers");
    f
}

fn problems(f: &Fixture) -> Vec<(i64, ProblemKind)> {
    f.log()
        .verify()
        .expect("verify")
        .problems
        .into_iter()
        .map(|p| (p.seq, p.kind))
        .collect()
}

#[test]
fn an_untouched_chain_verifies() {
    let f = tamper_fixture();
    let status = f.log().verify().expect("verify");
    assert_eq!(status.entries, 3);
    assert!(status.is_intact(), "{status:?}");
    let records = f.tmp.db.audit_records().unwrap();
    assert_eq!(records[0].prev_hash, caelum_security::audit::GENESIS_HASH);
    assert_eq!(records[1].prev_hash, records[0].hash);
    assert_eq!(records[2].prev_hash, records[1].hash);
}

/// Changing any stored field of entry 2 is detected at entry 2.
#[test]
fn changing_any_field_is_detected() {
    let changes = [
        ("id", "'00000000-0000-0000-0000-000000000000'"),
        ("at_tai", "at_tai + 1"),
        ("actor", "'someone-else'"),
        ("role", "'cmo'"),
        ("action", "'auth.logout'"),
        ("target", "NULL"),
        ("details", "'{\"n\":9}'"),
    ];
    for (column, value) in changes {
        let f = tamper_fixture();
        f.tmp
            .db
            .connection()
            .execute(
                &format!("UPDATE audit_entry SET {column} = {value} WHERE seq = 2"),
                [],
            )
            .expect("tamper");
        let found = problems(&f);
        assert!(
            found.contains(&(2, ProblemKind::HashMismatch)),
            "{column}: {found:?}"
        );
    }
}

/// Changing a field and storing the matching hash still fails without the instance key: the
/// signature no longer matches, and the next entry's link breaks.
#[test]
fn recomputing_the_hash_without_the_key_is_detected() {
    let f = tamper_fixture();
    let mut forged = f.tmp.db.audit_records().unwrap().remove(1);
    forged.details = r#"{"n":9}"#.to_owned();
    let c = f.tmp.db.connection();
    c.execute(
        "UPDATE audit_entry SET details = ?1, hash = ?2 WHERE seq = 2",
        [&forged.details, &hex(&record_hash(&forged))],
    )
    .unwrap();
    let found = problems(&f);
    assert!(
        !found.contains(&(2, ProblemKind::HashMismatch)),
        "{found:?}"
    );
    assert!(found.contains(&(2, ProblemKind::BadSignature)), "{found:?}");
    assert!(found.contains(&(3, ProblemKind::BrokenLink)), "{found:?}");
}

/// Overwriting only the stored hash breaks the hash check and the next link.
#[test]
fn a_replaced_hash_is_detected() {
    let f = tamper_fixture();
    f.tmp
        .db
        .connection()
        .execute(
            "UPDATE audit_entry SET hash = ?1 WHERE seq = 2",
            [hex(&[0xaa; 32])],
        )
        .unwrap();
    let found = problems(&f);
    assert!(found.contains(&(2, ProblemKind::HashMismatch)), "{found:?}");
    assert!(found.contains(&(3, ProblemKind::BrokenLink)), "{found:?}");
}

#[test]
fn a_changed_link_or_signature_is_detected() {
    let f = tamper_fixture();
    f.tmp
        .db
        .connection()
        .execute(
            "UPDATE audit_entry SET prev_hash = ?1 WHERE seq = 3",
            [hex(&[1; 32])],
        )
        .unwrap();
    assert!(problems(&f).contains(&(3, ProblemKind::BrokenLink)));

    let f = tamper_fixture();
    f.tmp
        .db
        .connection()
        .execute(
            "UPDATE audit_entry SET signature = ?1 WHERE seq = 1",
            [hex(&[0; 64])],
        )
        .unwrap();
    assert_eq!(problems(&f), vec![(1, ProblemKind::BadSignature)]);
}

#[test]
fn removing_or_reordering_entries_is_detected() {
    let f = tamper_fixture();
    f.tmp
        .db
        .connection()
        .execute("DELETE FROM audit_entry WHERE seq = 2", [])
        .unwrap();
    let found = problems(&f);
    assert!(found.contains(&(3, ProblemKind::SequenceGap)), "{found:?}");
    assert!(found.contains(&(3, ProblemKind::BrokenLink)), "{found:?}");

    let f = tamper_fixture();
    f.tmp
        .db
        .connection()
        .execute_batch(
            "UPDATE audit_entry SET seq = 99 WHERE seq = 1;
             UPDATE audit_entry SET seq = 1 WHERE seq = 2;
             UPDATE audit_entry SET seq = 2 WHERE seq = 99;",
        )
        .unwrap();
    assert!(!f.log().verify().unwrap().is_intact());
}

/// A chain signed by another installation's key does not verify against this one.
#[test]
fn entries_signed_by_another_key_are_detected() {
    let f = tamper_fixture();
    let other = InstanceKey::from_seed(&[0x99; 32]);
    let status = verify_records(&f.tmp.db.audit_records().unwrap(), &other.verifying_key());
    assert_eq!(status.problems.len(), 3);
    assert!(status
        .problems
        .iter()
        .all(|p| p.kind == ProblemKind::BadSignature));
}

/// Without tampering, the append-only triggers stay in force.
#[test]
fn entries_cannot_be_changed_through_sql() {
    let f = Fixture::new();
    f.log()
        .append(&Actor::system(), AuditAction::KeyCreated, None, &json!({}))
        .unwrap();
    let c = f.tmp.db.connection();
    assert!(c.execute("UPDATE audit_entry SET actor = 'x'", []).is_err());
    assert!(c.execute("DELETE FROM audit_entry", []).is_err());
}

/// Entries carry the clock's TAI time, and listing filters by the reader's categories.
#[test]
fn entries_are_timestamped_and_listed_by_role() {
    let f = Fixture::new();
    let before = caelum_security::clock::Clock::now_tai(&f.clock).unwrap();
    let entry = f
        .log()
        .append(
            &Actor::system(),
            AuditAction::KeyCreated,
            Some("database-key"),
            &json!({}),
        )
        .unwrap();
    assert_eq!(entry.at_tai, before);
    assert_eq!(entry.seq, 1);
    let security = f
        .log()
        .list(Role::Administrator.audit_categories())
        .unwrap();
    assert_eq!(security.len(), 1);
    assert!(f
        .log()
        .list(Role::Bme.audit_categories())
        .unwrap()
        .is_empty());
    assert!(f
        .log()
        .list(Role::Astronaut.audit_categories())
        .unwrap()
        .is_empty());
    assert_eq!(f.log().list(&AuditCategory::ALL).unwrap().len(), 1);
}

/// Unknown action names (from a newer version) are shown only to readers of every category.
#[test]
fn unknown_actions_are_shown_only_to_full_readers() {
    let f = tamper_fixture();
    f.tmp
        .db
        .connection()
        .execute(
            "UPDATE audit_entry SET action = 'future.action' WHERE seq = 1",
            [],
        )
        .unwrap();
    assert_eq!(
        f.log()
            .list(Role::Administrator.audit_categories())
            .unwrap()
            .len(),
        2
    );
    assert_eq!(f.log().list(Role::Cmo.audit_categories()).unwrap().len(), 3);
}
