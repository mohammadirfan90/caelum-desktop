//! Opening the encrypted database and applying migrations (F-SYS-02, R-SEC-01).

use std::path::Path;

use rusqlite::Connection;

use crate::key::DbKey;

mod embedded {
    refinery::embed_migrations!("migrations");
}

/// Storage errors.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The key does not open this database, or the file is not a CAELUM database.
    #[error("the database cannot be opened with this key (wrong key or not a database)")]
    WrongKeyOrNotDatabase,
    /// SQLite or SQLCipher error.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Migration error.
    #[error("migration error: {0}")]
    Migration(#[from] refinery::Error),
    /// SQLCipher is not compiled in (a plain SQLite build must never be used).
    #[error("SQLCipher is not available in this build")]
    NoEncryption,
}

/// An open, migrated, encrypted CAELUM database.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Opens (creating if needed) the database at `path` with `key`, verifies the key, applies the
    /// settings for crash safety and integrity, and runs pending migrations.
    pub fn open(path: &Path, key: &DbKey) -> Result<Self, StoreError> {
        let mut conn = Connection::open(path)?;
        let version: Option<String> = conn
            .query_row("PRAGMA cipher_version", [], |r| r.get(0))
            .ok();
        if version.as_deref().is_none_or(str::is_empty) {
            return Err(StoreError::NoEncryption);
        }
        conn.pragma_update(None, "key", &*key.pragma_value())?;
        // Any read fails if the key is wrong ("file is not a database").
        if conn
            .query_row("SELECT count(*) FROM sqlite_master", [], |r| {
                r.get::<_, i64>(0)
            })
            .is_err()
        {
            return Err(StoreError::WrongKeyOrNotDatabase);
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // The MDS connector has its own connection (CR-016): wait for the other's writes.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "secure_delete", "ON")?;
        embedded::migrations::runner().run(&mut conn)?;
        Self::guard_simulated(&conn)?;
        Ok(Self { conn })
    }

    /// CLAUDE.md rule 3, docs/05 §3: outside `test-equipment` builds the database itself refuses
    /// simulated observations (defence in depth behind the type system).
    fn guard_simulated(conn: &Connection) -> Result<(), StoreError> {
        if cfg!(feature = "test-equipment") {
            return Ok(());
        }
        conn.execute_batch(
            "CREATE TEMP TRIGGER IF NOT EXISTS observation_no_simulated
             BEFORE INSERT ON main.observation WHEN NEW.classification = 'simulated'
             BEGIN SELECT RAISE(ABORT, 'simulated observations are not allowed in this build'); END;",
        )?;
        Ok(())
    }

    /// The underlying connection, for repositories in this crate and for tests.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// SQLite integrity check (`PRAGMA quick_check`): `Ok(())` or the problems found (F-SYS-09).
    pub fn quick_check(&self) -> Result<(), String> {
        let mut stmt = self
            .conn
            .prepare("PRAGMA quick_check")
            .map_err(|e| e.to_string())?;
        let rows: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        if rows == ["ok"] {
            Ok(())
        } else {
            Err(rows.join("; "))
        }
    }

    /// Names of all user tables, sorted.
    pub fn table_names(&self) -> Result<Vec<String>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table'
             AND name NOT LIKE 'sqlite_%' AND name <> 'refinery_schema_history' ORDER BY name",
        )?;
        let names = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(names)
    }
}
