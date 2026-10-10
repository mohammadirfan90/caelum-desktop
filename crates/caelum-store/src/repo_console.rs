//! Repository for the ground console profile (F-GND-08; CR-025): downlinked summaries, OPS
//! records and files, and the private medical conference record.

use rusqlite::OptionalExtension;

use crate::db::{Database, StoreError};

/// One downlinked parameter summary.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundSummaryRecord {
    /// Crew member.
    pub crew_member_id: String,
    /// Parameter key.
    pub parameter: String,
    /// Window start, TAI ns.
    pub window_start: i64,
    /// Window end, TAI ns.
    pub window_end: i64,
    /// Worst quality.
    pub quality: String,
    /// Minimum (numeric).
    pub min_value: Option<f64>,
    /// Mean (numeric).
    pub mean_value: Option<f64>,
    /// Maximum (numeric).
    pub max_value: Option<f64>,
    /// Last state (enumerated).
    pub state: Option<String>,
    /// Readings in the window.
    pub count: i64,
}

/// One received file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundFileRecord {
    /// ID.
    pub id: String,
    /// Name as sent.
    pub name: String,
    /// Kind (`event_packet`, …).
    pub kind: String,
    /// Received, TAI ns.
    pub received_at: i64,
}

/// One private medical conference record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmcRecord {
    /// ID.
    pub id: String,
    /// Crew member.
    pub crew_member_id: String,
    /// Author (user ID).
    pub author: String,
    /// When the conference was held, TAI ns.
    pub held_at: i64,
    /// Notes.
    pub notes: String,
    /// Created, TAI ns.
    pub created_at: i64,
    /// Signed, TAI ns.
    pub signed_at: Option<i64>,
}

impl Database {
    /// Stores a summary; a resent one (same crew, parameter, window end) is ignored.
    pub fn insert_ground_summary(
        &self,
        id: &str,
        r: &GroundSummaryRecord,
        received_at: i64,
    ) -> Result<bool, StoreError> {
        let n = self.connection().execute(
            "INSERT OR IGNORE INTO ground_summary (id, crew_member_id, parameter, window_start, window_end,
             quality, min_value, mean_value, max_value, state, count, received_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                id,
                r.crew_member_id,
                r.parameter,
                r.window_start,
                r.window_end,
                r.quality,
                r.min_value,
                r.mean_value,
                r.max_value,
                r.state,
                r.count,
                received_at
            ],
        )?;
        Ok(n == 1)
    }

    /// Summaries of a crew member since `from`, newest window first.
    pub fn ground_summaries(
        &self,
        crew: &str,
        from: i64,
    ) -> Result<Vec<GroundSummaryRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(
            "SELECT crew_member_id, parameter, window_start, window_end, quality, min_value,
             mean_value, max_value, state, count FROM ground_summary
             WHERE crew_member_id = ?1 AND window_end >= ?2 ORDER BY window_end DESC, parameter",
        )?;
        let rows = stmt.query_map(rusqlite::params![crew, from], |r| {
            Ok(GroundSummaryRecord {
                crew_member_id: r.get(0)?,
                parameter: r.get(1)?,
                window_start: r.get(2)?,
                window_end: r.get(3)?,
                quality: r.get(4)?,
                min_value: r.get(5)?,
                mean_value: r.get(6)?,
                max_value: r.get(7)?,
                state: r.get(8)?,
                count: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Stores an OPS record (JSON); a resent one is ignored. Returns whether it was new.
    pub fn insert_ground_ops(
        &self,
        key: &str,
        kind: &str,
        at: i64,
        message: &str,
    ) -> Result<bool, StoreError> {
        Ok(self.connection().execute(
            "INSERT OR IGNORE INTO ground_ops (seq_key, kind, at_tai, message) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![key, kind, at, message],
        )? == 1)
    }

    /// OPS records of `kind` with packet time in `[from, to]`, oldest first.
    pub fn ground_ops(&self, kind: &str, from: i64, to: i64) -> Result<Vec<String>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(
            "SELECT message FROM ground_ops WHERE kind = ?1 AND at_tai BETWEEN ?2 AND ?3 ORDER BY at_tai",
        )?;
        let rows = stmt.query_map(rusqlite::params![kind, from, to], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Stores a received file; a resent one (same name) is ignored.
    pub fn insert_ground_file(
        &self,
        id: &str,
        name: &str,
        kind: &str,
        content: &[u8],
        at: i64,
    ) -> Result<bool, StoreError> {
        Ok(self.connection().execute(
            "INSERT OR IGNORE INTO ground_file (id, name, kind, content, received_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, name, kind, content, at],
        )? == 1)
    }

    /// Received files, newest first.
    pub fn ground_files(&self) -> Result<Vec<GroundFileRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(
            "SELECT id, name, kind, received_at FROM ground_file ORDER BY received_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(GroundFileRecord {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                received_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The content of a received file.
    pub fn ground_file(&self, id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self
            .connection()
            .query_row("SELECT content FROM ground_file WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Inserts or updates an unsigned PMC record.
    pub fn upsert_pmc(&self, r: &PmcRecord) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO pmc_record (id, crew_member_id, author, held_at, notes, created_at, signed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (id) DO UPDATE SET held_at = excluded.held_at, notes = excluded.notes,
             signed_at = excluded.signed_at",
            rusqlite::params![r.id, r.crew_member_id, r.author, r.held_at, r.notes, r.created_at, r.signed_at],
        )?;
        Ok(())
    }

    /// PMC records of a crew member, newest first.
    pub fn pmc_records(&self, crew: &str) -> Result<Vec<PmcRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(
            "SELECT id, crew_member_id, author, held_at, notes, created_at, signed_at FROM pmc_record
             WHERE crew_member_id = ?1 ORDER BY held_at DESC",
        )?;
        let rows = stmt.query_map([crew], |r| {
            Ok(PmcRecord {
                id: r.get(0)?,
                crew_member_id: r.get(1)?,
                author: r.get(2)?,
                held_at: r.get(3)?,
                notes: r.get(4)?,
                created_at: r.get(5)?,
                signed_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// One PMC record.
    pub fn pmc_record(&self, id: &str) -> Result<Option<PmcRecord>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                "SELECT id, crew_member_id, author, held_at, notes, created_at, signed_at FROM pmc_record WHERE id = ?1",
                [id],
                |r| {
                    Ok(PmcRecord {
                        id: r.get(0)?,
                        crew_member_id: r.get(1)?,
                        author: r.get(2)?,
                        held_at: r.get(3)?,
                        notes: r.get(4)?,
                        created_at: r.get(5)?,
                        signed_at: r.get(6)?,
                    })
                },
            )
            .optional()?)
    }
}
