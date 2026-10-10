//! Repository for the fail-over pair (F-SYS-04, R-OPS-02; CR-017): the replicated event log, its
//! quarantine and small state values. The log's meaning (signatures, chaining, fencing) belongs
//! to `caelum-cluster`; this layer stores rows.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};

use crate::db::{Database, StoreError};

/// One stored log entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterLogRecord {
    /// Position, from 1.
    pub seq: i64,
    /// Fence epoch of the producing primary.
    pub fence_epoch: i64,
    /// Node of the producing primary.
    pub fence_node: String,
    /// Event kind.
    pub kind: String,
    /// When the event happened (TAI ns).
    pub at_tai: i64,
    /// The core's serialised record.
    pub payload: Vec<u8>,
    /// Hash of the previous entry (32 bytes).
    pub prev_hash: Vec<u8>,
    /// Ed25519 signature (64 bytes).
    pub signature: Vec<u8>,
}

const COLUMNS: &str = "seq, fence_epoch, fence_node, kind, at_tai, payload, prev_hash, signature";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ClusterLogRecord> {
    Ok(ClusterLogRecord {
        seq: r.get(0)?,
        fence_epoch: r.get(1)?,
        fence_node: r.get(2)?,
        kind: r.get(3)?,
        at_tai: r.get(4)?,
        payload: r.get(5)?,
        prev_hash: r.get(6)?,
        signature: r.get(7)?,
    })
}

impl Database {
    /// The last log entry.
    pub fn cluster_log_last(&self) -> Result<Option<ClusterLogRecord>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {COLUMNS} FROM cluster_log ORDER BY seq DESC LIMIT 1"),
                [],
                from_row,
            )
            .optional()?)
    }

    /// Appends one entry; `apply` runs in the same immediate transaction (the core applies the
    /// entry to its record there), so an entry and its effect are stored together or not at all.
    pub fn cluster_log_append(
        &self,
        record: &ClusterLogRecord,
        apply: impl FnOnce(&Database) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let tx = Transaction::new_unchecked(self.connection(), TransactionBehavior::Immediate)?;
        tx.execute(
            &format!("INSERT INTO cluster_log ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
            rusqlite::params![
                record.seq,
                record.fence_epoch,
                record.fence_node,
                record.kind,
                record.at_tai,
                record.payload,
                record.prev_hash,
                record.signature,
            ],
        )?;
        apply(self)?;
        tx.commit()?;
        Ok(())
    }

    /// The entry at `seq`.
    pub fn cluster_log_get(&self, seq: i64) -> Result<Option<ClusterLogRecord>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {COLUMNS} FROM cluster_log WHERE seq = ?1"),
                [seq],
                from_row,
            )
            .optional()?)
    }

    /// Up to `limit` entries from `from` on, in order.
    pub fn cluster_log_range(
        &self,
        from: i64,
        limit: i64,
    ) -> Result<Vec<ClusterLogRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM cluster_log WHERE seq >= ?1 ORDER BY seq LIMIT ?2"
        ))?;
        let rows = stmt.query_map([from, limit], from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Moves every entry after `seq` into the quarantine (kept for review, never re-applied) and
    /// returns them, in one transaction; `undo` runs inside it for each, newest first.
    pub fn cluster_log_divert_after(
        &self,
        seq: i64,
        at_tai: i64,
        mut undo: impl FnMut(&Database, &ClusterLogRecord) -> Result<(), StoreError>,
    ) -> Result<Vec<ClusterLogRecord>, StoreError> {
        let tx = Transaction::new_unchecked(self.connection(), TransactionBehavior::Immediate)?;
        let moved: Vec<ClusterLogRecord> = {
            let mut stmt = tx.prepare(&format!(
                "SELECT {COLUMNS} FROM cluster_log WHERE seq > ?1 ORDER BY seq"
            ))?;
            let rows = stmt.query_map([seq], from_row)?;
            rows.collect::<Result<_, _>>()?
        };
        for r in moved.iter().rev() {
            undo(self, r)?;
        }
        tx.execute(
            &format!(
                "INSERT OR IGNORE INTO cluster_quarantine ({COLUMNS}, diverted_at)
                 SELECT {COLUMNS}, ?2 FROM cluster_log WHERE seq > ?1"
            ),
            [seq, at_tai],
        )?;
        tx.execute("DELETE FROM cluster_log WHERE seq > ?1", [seq])?;
        tx.commit()?;
        Ok(moved)
    }

    /// Quarantined entries, oldest first (review and export).
    pub fn cluster_quarantine(&self) -> Result<Vec<ClusterLogRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM cluster_quarantine ORDER BY diverted_at, fence_epoch, seq"
        ))?;
        let rows = stmt.query_map([], from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Sets a state value.
    pub fn cluster_set_state(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO cluster_state (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            (key, value),
        )?;
        Ok(())
    }

    /// A state value.
    pub fn cluster_state(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                "SELECT value FROM cluster_state WHERE key = ?1",
                [key],
                |r| r.get(0),
            )
            .optional()?)
    }
}

impl Database {
    /// Retention (F-SYS-08, CR-022): removes observations of `parameters` measured before
    /// `before_tai` and not within `guard_ns` of any alarm of the same crew member. Returns the
    /// number removed.
    pub fn purge_observations(
        &self,
        parameters: &[&str],
        before_tai: i64,
        guard_ns: i64,
    ) -> Result<usize, StoreError> {
        let mut n = 0;
        for p in parameters {
            n += self.connection().execute(
                "DELETE FROM observation WHERE parameter = ?1 AND measured_at_tai < ?2
                 AND NOT EXISTS (SELECT 1 FROM alarm a WHERE a.crew_member_id = observation.crew_member_id
                     AND a.created_at_tai BETWEEN observation.measured_at_tai - ?3
                                              AND observation.measured_at_tai + ?3)",
                rusqlite::params![p, before_tai, guard_ns],
            )?;
        }
        Ok(n)
    }

    /// Retention (CR-022): removes raw waveform chunks that start before `before_tai` and lie
    /// more than `guard_ns` from every alarm of their crew member. Returns the number removed.
    pub fn purge_waveforms(&self, before_tai: i64, guard_ns: i64) -> Result<usize, StoreError> {
        Ok(self.connection().execute(
            "DELETE FROM waveform_chunk WHERE start_tai < ?1
             AND NOT EXISTS (SELECT 1 FROM alarm a WHERE a.crew_member_id = waveform_chunk.crew_member_id
                 AND a.created_at_tai BETWEEN waveform_chunk.start_tai - ?2
                                          AND waveform_chunk.start_tai + ?2)",
            rusqlite::params![before_tai, guard_ns],
        )?)
    }
}
