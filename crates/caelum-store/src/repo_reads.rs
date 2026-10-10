//! Read queries for the crew screens (CR-012 §4): newest values, trends, baselines, waveform
//! chunks, alarm history with its escalation timeline. Authorisation is the caller's job.

use rusqlite::{OptionalExtension, Row};

use crate::db::{Database, StoreError};
use crate::repo_acquisition::{observation_from_row, OBSERVATION_COLUMNS};
use crate::ObservationRecord;

/// A row of the `baseline` table.
#[derive(Debug, Clone, PartialEq)]
pub struct BaselineRecord {
    /// Median.
    pub median: f64,
    /// Median absolute deviation.
    pub mad: f64,
    /// Samples behind it.
    pub n: i64,
    /// Window description.
    pub window: String,
    /// Status (e.g. `established`).
    pub status: String,
    /// Estimation method version.
    pub method_version: String,
}

/// A row of the `waveform_chunk` table.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveformChunkRecord {
    /// Device.
    pub device_id: Option<String>,
    /// Signal name (e.g. `ecg_lead_ii`).
    pub signal: String,
    /// Start, TAI ns.
    pub start_tai: i64,
    /// Samples per second.
    pub sample_rate: f64,
    /// Sample encoding.
    pub encoding: String,
    /// Samples.
    pub data: Vec<u8>,
    /// Quality.
    pub quality: String,
}

/// An alarm with everything the alarm screens show.
#[derive(Debug, Clone, PartialEq)]
pub struct AlarmView {
    /// Alarm ID.
    pub id: String,
    /// Crew member.
    pub crew_member_id: Option<String>,
    /// `physiological` / `technical`.
    pub kind: String,
    /// Category.
    pub category: String,
    /// Rule ID.
    pub rule_id: String,
    /// Ruleset version.
    pub ruleset_version: String,
    /// Priority.
    pub priority: String,
    /// State.
    pub state: String,
    /// Title (physiological alarms).
    pub title: Option<String>,
    /// Triggering value.
    pub trigger_value: Option<f64>,
    /// Why (JSON).
    pub why: String,
    /// Action.
    pub action: Option<String>,
    /// Raised, TAI ns.
    pub created_at_tai: i64,
    /// Acknowledged by (user ID).
    pub acknowledged_by: Option<String>,
    /// Acknowledged at.
    pub acknowledged_at_tai: Option<i64>,
    /// Ground stage acknowledged by.
    pub ground_acknowledged_by: Option<String>,
    /// Ground stage acknowledged at.
    pub ground_acknowledged_at_tai: Option<i64>,
    /// Silenced until.
    pub silenced_until_tai: Option<i64>,
    /// Ended at.
    pub resolved_at_tai: Option<i64>,
    /// Escalation stage reached.
    pub escalation_stage: i64,
}

/// One delivered escalation stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRecord {
    /// Audience.
    pub audience: String,
    /// Stage index.
    pub stage: i64,
    /// Delivered at, TAI ns.
    pub at_tai: i64,
}

const ALARM_COLUMNS: &str = "id, crew_member_id, kind, category, rule_id, ruleset_version, priority, \
     state, title, trigger_value, why, action, created_at_tai, acknowledged_by, acknowledged_at_tai, \
     ground_acknowledged_by, ground_acknowledged_at_tai, silenced_until_tai, resolved_at_tai, \
     escalation_stage";

fn alarm_from_row(r: &Row<'_>) -> rusqlite::Result<AlarmView> {
    Ok(AlarmView {
        id: r.get(0)?,
        crew_member_id: r.get(1)?,
        kind: r.get(2)?,
        category: r.get(3)?,
        rule_id: r.get(4)?,
        ruleset_version: r.get(5)?,
        priority: r.get(6)?,
        state: r.get(7)?,
        title: r.get(8)?,
        trigger_value: r.get(9)?,
        why: r.get(10)?,
        action: r.get(11)?,
        created_at_tai: r.get(12)?,
        acknowledged_by: r.get(13)?,
        acknowledged_at_tai: r.get(14)?,
        ground_acknowledged_by: r.get(15)?,
        ground_acknowledged_at_tai: r.get(16)?,
        silenced_until_tai: r.get(17)?,
        resolved_at_tai: r.get(18)?,
        escalation_stage: r.get(19)?,
    })
}

impl Database {
    /// The newest observation of every parameter for a crew member.
    pub fn latest_observations(&self, crew: &str) -> Result<Vec<ObservationRecord>, StoreError> {
        let mut stmt = self.connection().prepare(&format!(
            "SELECT {OBSERVATION_COLUMNS} FROM observation o
             WHERE crew_member_id = ?1 AND measured_at_tai = (
                 SELECT max(measured_at_tai) FROM observation
                 WHERE crew_member_id = ?1 AND parameter = o.parameter)
             ORDER BY parameter, id"
        ))?;
        let rows = stmt.query_map([crew], observation_from_row)?;
        let mut out: Vec<ObservationRecord> = Vec::new();
        for r in rows {
            let r = r?;
            // Two readings at the same instant: keep one per parameter.
            if out.last().is_none_or(|l| l.parameter != r.parameter) {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// One parameter's observations in `[from, to]`, oldest first.
    pub fn observations_between(
        &self,
        crew: &str,
        parameter: &str,
        from_tai: i64,
        to_tai: i64,
    ) -> Result<Vec<ObservationRecord>, StoreError> {
        let mut stmt = self.connection().prepare(&format!(
            "SELECT {OBSERVATION_COLUMNS} FROM observation
             WHERE crew_member_id = ?1 AND parameter = ?2 AND measured_at_tai BETWEEN ?3 AND ?4
             ORDER BY measured_at_tai, id"
        ))?;
        let rows = stmt.query_map(
            rusqlite::params![crew, parameter, from_tai, to_tai],
            observation_from_row,
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Every observation of a crew member in `[from, to]`, oldest first (export, F-SYS-08).
    pub fn observations_for_crew_between(
        &self,
        crew: &str,
        from_tai: i64,
        to_tai: i64,
    ) -> Result<Vec<ObservationRecord>, StoreError> {
        let mut stmt = self.connection().prepare(&format!(
            "SELECT {OBSERVATION_COLUMNS} FROM observation
             WHERE crew_member_id = ?1 AND measured_at_tai BETWEEN ?2 AND ?3
             ORDER BY measured_at_tai, id"
        ))?;
        let rows = stmt.query_map(
            rusqlite::params![crew, from_tai, to_tai],
            observation_from_row,
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The newest observation of a parameter reported by a device (battery, link metrics).
    pub fn latest_device_observation(
        &self,
        device: &str,
        parameter: &str,
    ) -> Result<Option<ObservationRecord>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                &format!(
                    "SELECT {OBSERVATION_COLUMNS} FROM observation WHERE device_id = ?1 AND parameter = ?2
                     ORDER BY measured_at_tai DESC, id LIMIT 1"
                ),
                [device, parameter],
                observation_from_row,
            )
            .optional()?)
    }

    /// Stores a waveform chunk (F-ACQ-03; docs/03 §3 step 3: 1 s chunks).
    pub fn insert_waveform_chunk(
        &self,
        crew: &str,
        chunk: &WaveformChunkRecord,
    ) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO waveform_chunk (id, crew_member_id, device_id, signal, start_tai, sample_rate, encoding, data, quality)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                uuid_like(crew, chunk.start_tai, &chunk.signal),
                crew,
                chunk.device_id,
                chunk.signal,
                chunk.start_tai,
                chunk.sample_rate,
                chunk.encoding,
                chunk.data,
                chunk.quality
            ],
        )?;
        Ok(())
    }

    /// Stores (replaces) a crew member's personal baseline for a parameter (F-ALM-08).
    pub fn upsert_baseline(
        &self,
        crew: &str,
        parameter: &str,
        b: &BaselineRecord,
    ) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO baseline (crew_member_id, parameter, median, mad, n, window, status, method_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (crew_member_id, parameter) DO UPDATE SET median = excluded.median, mad = excluded.mad,
                 n = excluded.n, window = excluded.window, status = excluded.status, method_version = excluded.method_version",
            rusqlite::params![crew, parameter, b.median, b.mad, b.n, b.window, b.status, b.method_version],
        )?;
        Ok(())
    }

    /// A crew member's personal baseline for a parameter, if one was estimated.
    pub fn baseline(
        &self,
        crew: &str,
        parameter: &str,
    ) -> Result<Option<BaselineRecord>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                "SELECT median, mad, n, window, status, method_version FROM baseline
                 WHERE crew_member_id = ?1 AND parameter = ?2",
                [crew, parameter],
                |r| {
                    Ok(BaselineRecord {
                        median: r.get(0)?,
                        mad: r.get(1)?,
                        n: r.get(2)?,
                        window: r.get(3)?,
                        status: r.get(4)?,
                        method_version: r.get(5)?,
                    })
                },
            )
            .optional()?)
    }

    /// Waveform chunks of one signal starting at or after `from_tai`, oldest first.
    pub fn waveform_chunks(
        &self,
        crew: &str,
        signal: &str,
        from_tai: i64,
    ) -> Result<Vec<WaveformChunkRecord>, StoreError> {
        let mut stmt = self.connection().prepare(
            "SELECT device_id, signal, start_tai, sample_rate, encoding, data, quality FROM waveform_chunk
             WHERE crew_member_id = ?1 AND signal = ?2 AND start_tai >= ?3 ORDER BY start_tai, id",
        )?;
        let rows = stmt.query_map(rusqlite::params![crew, signal, from_tai], |r| {
            Ok(WaveformChunkRecord {
                device_id: r.get(0)?,
                signal: r.get(1)?,
                start_tai: r.get(2)?,
                sample_rate: r.get(3)?,
                encoding: r.get(4)?,
                data: r.get(5)?,
                quality: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Alarms open now or ended at or after `ended_since_tai`, newest first.
    pub fn alarms_recent(&self, ended_since_tai: i64) -> Result<Vec<AlarmView>, StoreError> {
        let mut stmt = self.connection().prepare(&format!(
            "SELECT {ALARM_COLUMNS} FROM alarm
             WHERE resolved_at_tai IS NULL OR resolved_at_tai >= ?1
             ORDER BY created_at_tai DESC, id"
        ))?;
        let rows = stmt.query_map([ended_since_tai], alarm_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Alarms raised in `[from, to]`, oldest first (history and report, F-ALM-09).
    pub fn alarms_raised_between(
        &self,
        from_tai: i64,
        to_tai: i64,
    ) -> Result<Vec<AlarmView>, StoreError> {
        let mut stmt = self.connection().prepare(&format!(
            "SELECT {ALARM_COLUMNS} FROM alarm WHERE created_at_tai BETWEEN ?1 AND ?2
             ORDER BY created_at_tai, id"
        ))?;
        let rows = stmt.query_map([from_tai, to_tai], alarm_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// One alarm.
    pub fn alarm_view(&self, id: &str) -> Result<Option<AlarmView>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {ALARM_COLUMNS} FROM alarm WHERE id = ?1"),
                [id],
                alarm_from_row,
            )
            .optional()?)
    }

    /// An alarm's delivered escalation stages, in order.
    pub fn alarm_notifications(&self, id: &str) -> Result<Vec<NotificationRecord>, StoreError> {
        let mut stmt = self.connection().prepare(
            "SELECT audience, stage, at_tai FROM alarm_notification WHERE alarm_id = ?1 ORDER BY stage",
        )?;
        let rows = stmt.query_map([id], |r| {
            Ok(NotificationRecord {
                audience: r.get(0)?,
                stage: r.get(1)?,
                at_tai: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// A stable chunk ID from its crew member, start and signal (one chunk per signal and start).
fn uuid_like(crew: &str, start: i64, signal: &str) -> String {
    format!("{crew}/{signal}/{start}")
}
