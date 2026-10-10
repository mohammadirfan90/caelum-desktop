//! Repository for the MDS connector (F-GND-12, CR-016): outbox records, read-back de-duplication
//! and small state values (high-water mark, summary watermark, client identity).

use rusqlite::OptionalExtension;

use crate::db::{Database, StoreError};

impl Database {
    /// Inserts or replaces one outbox record (JSON).
    pub fn mds_put_outbox(&self, client_id: &str, record: &str) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO mds_outbox (client_id, record) VALUES (?1, ?2)
             ON CONFLICT (client_id) DO UPDATE SET record = excluded.record",
            (client_id, record),
        )?;
        Ok(())
    }

    /// Every outbox record (JSON).
    pub fn mds_outbox(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare("SELECT record FROM mds_outbox ORDER BY client_id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records a processed read-back resource.
    pub fn mds_put_seen(&self, source_id: &str, version: &str) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO mds_inbox_seen (source_id, version) VALUES (?1, ?2)
             ON CONFLICT (source_id) DO UPDATE SET version = excluded.version",
            (source_id, version),
        )?;
        Ok(())
    }

    /// Every processed read-back resource.
    pub fn mds_seen(&self) -> Result<Vec<(String, String)>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare("SELECT source_id, version FROM mds_inbox_seen")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Sets a state value.
    pub fn mds_set_state(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO mds_state (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            (key, value),
        )?;
        Ok(())
    }

    /// A state value.
    pub fn mds_state(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .connection()
            .query_row("SELECT value FROM mds_state WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }
}
