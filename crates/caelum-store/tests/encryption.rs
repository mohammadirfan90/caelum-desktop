//! Storage gate tests (M1): encryption at rest (F-SYS-02, R-SEC-01), schema matches
//! docs/05_DATA_MODEL.md §2, append-only audit log (R-SEC-03), no simulated observations outside
//! test-equipment builds (CLAUDE.md rule 3).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use caelum_store::{Database, DbKey, StoreError};

const MARKER: &str = "PLAINTEXT-MARKER-CREW-ARMSTRONG";
const DATA_MODEL_DOC: &str = include_str!("../../../docs/05_DATA_MODEL.md");

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh database path in a private temporary directory, removed on drop.
struct TempDb(PathBuf);

impl TempDb {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("caelum-store-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir.join("caelum.db"))
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        if let Some(dir) = self.0.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn key(byte: u8) -> DbKey {
    DbKey::from_bytes([byte; 32])
}

fn insert_crew(db: &Database, id: &str, name: &str) {
    db.connection()
        .execute(
            "INSERT INTO crew_member (id, name, role, mission, active) VALUES (?1, ?2, 'astronaut', 'M-1', 1)",
            (id, name),
        )
        .expect("insert crew member");
}

/// M1 gate, R-SEC-01: the file on disk reveals nothing without the key, and a wrong key fails.
#[test]
fn database_file_is_unreadable_without_the_key() {
    let tmp = TempDb::new();
    {
        let db = Database::open(&tmp.0, &key(0x11)).expect("create");
        insert_crew(&db, "c1", MARKER);
        db.connection()
            .pragma_update(None, "wal_checkpoint", "TRUNCATE")
            .expect("checkpoint");
    }
    let raw = std::fs::read(&tmp.0).expect("read file");
    assert!(
        !raw.starts_with(b"SQLite format 3"),
        "plain SQLite header on disk"
    );
    assert!(
        !raw.windows(MARKER.len()).any(|w| w == MARKER.as_bytes()),
        "plaintext found in the database file"
    );

    match Database::open(&tmp.0, &key(0x22)) {
        Err(StoreError::WrongKeyOrNotDatabase) => {}
        other => panic!("wrong key must be refused, got {:?}", other.err()),
    }

    let db = Database::open(&tmp.0, &key(0x11)).expect("right key opens");
    let name: String = db
        .connection()
        .query_row("SELECT name FROM crew_member WHERE id = 'c1'", [], |r| {
            r.get(0)
        })
        .expect("row");
    assert_eq!(name, MARKER);
}

/// docs/05 §2 table names, read from the document's "Core tables" section.
fn documented_tables() -> Vec<String> {
    let start = DATA_MODEL_DOC.find("## 2. Core tables").expect("section 2");
    let end = DATA_MODEL_DOC[start..].find("## 3.").expect("section 3") + start;
    let mut names: Vec<String> = DATA_MODEL_DOC[start..end]
        .lines()
        .filter_map(|l| l.strip_prefix("| "))
        .filter_map(|l| l.split(" |").next())
        .map(str::trim)
        .filter(|n| !n.is_empty() && *n != "Table" && !n.starts_with("---"))
        .map(str::to_owned)
        .collect();
    names.sort();
    names
}

/// F-SYS-02: migrations create exactly the tables docs/05 §2 lists, and re-opening is idempotent.
#[test]
fn migrations_create_every_documented_table() {
    let tmp = TempDb::new();
    let db = Database::open(&tmp.0, &key(0x33)).expect("create");
    let expected = documented_tables();
    assert_eq!(expected.len(), 20, "{expected:?}");
    assert_eq!(db.table_names().expect("tables"), expected);
    drop(db);
    let again = Database::open(&tmp.0, &key(0x33)).expect("re-open");
    assert_eq!(again.table_names().expect("tables"), expected);
}

/// R-SEC-03, docs/03 §4: audit entries can be added but never changed or deleted.
#[test]
fn audit_log_is_append_only() {
    let tmp = TempDb::new();
    let db = Database::open(&tmp.0, &key(0x44)).expect("create");
    let c = db.connection();
    c.execute(
        "INSERT INTO audit_entry (id, at_tai, actor, role, action, target, details, prev_hash, hash, signature)
         VALUES ('a1', 1, 'u1', 'cmo', 'login', NULL, '{}', '00', 'h1', 's1')",
        [],
    )
    .expect("append");
    let update = c.execute(
        "UPDATE audit_entry SET action = 'tampered' WHERE id = 'a1'",
        [],
    );
    assert!(update.is_err(), "update must be refused");
    let delete = c.execute("DELETE FROM audit_entry WHERE id = 'a1'", []);
    assert!(delete.is_err(), "delete must be refused");
    let action: String = c
        .query_row("SELECT action FROM audit_entry WHERE id = 'a1'", [], |r| {
            r.get(0)
        })
        .expect("row");
    assert_eq!(action, "login");
}

fn insert_observation(db: &Database, id: &str, classification: &str) -> rusqlite::Result<usize> {
    db.connection().execute(
        "INSERT INTO observation (id, crew_member_id, parameter, value, unit, measured_at_tai,
         received_at_tai, quality, classification, measurement_mode)
         VALUES (?1, 'c1', 'spo2', 97.0, '%', 1, 2, 'good', ?2, 'continuous')",
        (id, classification),
    )
}

/// CLAUDE.md rule 3, docs/05 §3: the database refuses simulated observations in this build.
#[cfg(not(feature = "test-equipment"))]
#[test]
fn simulated_observations_are_refused_outside_test_equipment_builds() {
    let tmp = TempDb::new();
    let db = Database::open(&tmp.0, &key(0x55)).expect("create");
    insert_crew(&db, "c1", "Crew One");
    insert_observation(&db, "o1", "measured").expect("measured accepted");
    let err = insert_observation(&db, "o2", "simulated").expect_err("simulated refused");
    assert!(
        err.to_string()
            .contains("simulated observations are not allowed"),
        "{err}"
    );
}

/// Schema constraints reject values outside the documented enumerations.
#[test]
fn schema_rejects_undocumented_values() {
    let tmp = TempDb::new();
    let db = Database::open(&tmp.0, &key(0x66)).expect("create");
    insert_crew(&db, "c1", "Crew One");
    assert!(insert_observation(&db, "o1", "guessed").is_err());
    let bad_role = db.connection().execute(
        "INSERT INTO user (id, username, role, credential) VALUES ('u1', 'x', 'superuser', 'h')",
        [],
    );
    assert!(bad_role.is_err());
}
