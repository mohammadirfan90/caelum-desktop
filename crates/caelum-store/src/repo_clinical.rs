//! Repositories for clinical records (CR-019; F-CLN-02, F-CLN-04, F-CLN-05, F-CLN-07, F-CLN-08):
//! encounters, check-ins, checklist runs, the medication log and attachment metadata. Rows are
//! plain records; authorisation and audit are the caller's job.

use rusqlite::{OptionalExtension, Row};

use crate::db::{Database, StoreError};

/// A row of the `encounter` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncounterRecord {
    /// Encounter ID.
    pub id: String,
    /// Crew member.
    pub crew_member_id: String,
    /// Author (user ID).
    pub author: String,
    /// Subjective.
    pub subjective: Option<String>,
    /// Objective.
    pub objective: Option<String>,
    /// Assessment.
    pub assessment: Option<String>,
    /// Plan.
    pub plan: Option<String>,
    /// Vitals snapshot at creation (JSON).
    pub vitals: Option<String>,
    /// Created at, TAI ns.
    pub created_at_tai: Option<i64>,
    /// Signed at, TAI ns (immutable afterwards).
    pub signed_at_tai: Option<i64>,
    /// Attachment IDs (JSON array).
    pub attachments: Option<String>,
}

/// A row of the `checkin` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckinRecord {
    /// ID.
    pub id: String,
    /// Crew member.
    pub crew_member_id: String,
    /// Answers (JSON).
    pub answers: String,
    /// `private` / `medical` / `status`.
    pub privacy_tier: String,
    /// At, TAI ns.
    pub at_tai: i64,
}

/// A row of the `checklist_run` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecklistRunRecord {
    /// Run ID.
    pub id: String,
    /// Protocol ID.
    pub protocol_id: String,
    /// Protocol version.
    pub protocol_version: String,
    /// Linked alarm.
    pub alarm_id: Option<String>,
    /// Crew member.
    pub crew_member_id: Option<String>,
    /// Started by (user ID).
    pub started_by: Option<String>,
    /// Step states (JSON array).
    pub steps: String,
    /// Started at.
    pub started_at_tai: i64,
    /// Completed at.
    pub completed_at_tai: Option<i64>,
}

/// A row of the `medication_entry` table.
#[derive(Debug, Clone, PartialEq)]
pub struct MedicationRecord {
    /// ID.
    pub id: String,
    /// Crew member (doses only).
    pub crew_member_id: Option<String>,
    /// `dose` / `kit_use` / `kit_restock`.
    pub kind: String,
    /// Item.
    pub item: String,
    /// Amount.
    pub amount: Option<f64>,
    /// Unit.
    pub unit: Option<String>,
    /// Note.
    pub note: Option<String>,
    /// Recorded by (user ID).
    pub by_user: String,
    /// At, TAI ns.
    pub at_tai: i64,
}

/// A row of the `attachment` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRecord {
    /// ID.
    pub id: String,
    /// `dicom` / `ecg` / `file`.
    pub kind: String,
    /// SHA-256 of the plaintext, hex.
    pub content_hash: String,
    /// Metadata (JSON).
    pub metadata: String,
    /// Crew member.
    pub crew_member_id: Option<String>,
    /// Original file name.
    pub filename: Option<String>,
    /// Imported at.
    pub created_at_tai: Option<i64>,
}

const ENCOUNTER_COLUMNS: &str =
    "id, crew_member_id, author, subjective, objective, assessment, plan, vitals, \
     created_at_tai, signed_at_tai, attachments";

fn encounter_from_row(r: &Row<'_>) -> rusqlite::Result<EncounterRecord> {
    Ok(EncounterRecord {
        id: r.get(0)?,
        crew_member_id: r.get(1)?,
        author: r.get(2)?,
        subjective: r.get(3)?,
        objective: r.get(4)?,
        assessment: r.get(5)?,
        plan: r.get(6)?,
        vitals: r.get(7)?,
        created_at_tai: r.get(8)?,
        signed_at_tai: r.get(9)?,
        attachments: r.get(10)?,
    })
}

const RUN_COLUMNS: &str =
    "id, protocol_id, protocol_version, alarm_id, crew_member_id, started_by, steps, \
     started_at_tai, completed_at_tai";

fn run_from_row(r: &Row<'_>) -> rusqlite::Result<ChecklistRunRecord> {
    Ok(ChecklistRunRecord {
        id: r.get(0)?,
        protocol_id: r.get(1)?,
        protocol_version: r.get(2)?,
        alarm_id: r.get(3)?,
        crew_member_id: r.get(4)?,
        started_by: r.get(5)?,
        steps: r.get(6)?,
        started_at_tai: r.get(7)?,
        completed_at_tai: r.get(8)?,
    })
}

impl Database {
    /// Adds an encounter.
    pub fn insert_encounter(&self, e: &EncounterRecord) -> Result<(), StoreError> {
        self.connection().execute(
            &format!("INSERT INTO encounter ({ENCOUNTER_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"),
            rusqlite::params![
                e.id,
                e.crew_member_id,
                e.author,
                e.subjective,
                e.objective,
                e.assessment,
                e.plan,
                e.vitals,
                e.created_at_tai,
                e.signed_at_tai,
                e.attachments
            ],
        )?;
        Ok(())
    }

    /// Updates an unsigned encounter's notes and attachments; `false` if it is signed or absent.
    pub fn update_unsigned_encounter(&self, e: &EncounterRecord) -> Result<bool, StoreError> {
        let n = self.connection().execute(
            "UPDATE encounter SET subjective = ?2, objective = ?3, assessment = ?4, plan = ?5, attachments = ?6
             WHERE id = ?1 AND signed_at_tai IS NULL",
            rusqlite::params![e.id, e.subjective, e.objective, e.assessment, e.plan, e.attachments],
        )?;
        Ok(n == 1)
    }

    /// Signs an unsigned encounter; `false` if it is signed already or absent.
    pub fn sign_encounter(&self, id: &str, at_tai: i64) -> Result<bool, StoreError> {
        let n = self.connection().execute(
            "UPDATE encounter SET signed_at_tai = ?2 WHERE id = ?1 AND signed_at_tai IS NULL",
            rusqlite::params![id, at_tai],
        )?;
        Ok(n == 1)
    }

    /// One encounter.
    pub fn encounter(&self, id: &str) -> Result<Option<EncounterRecord>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {ENCOUNTER_COLUMNS} FROM encounter WHERE id = ?1"),
                [id],
                encounter_from_row,
            )
            .optional()?)
    }

    /// A crew member's encounters, newest first.
    pub fn encounters(&self, crew: &str) -> Result<Vec<EncounterRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(&format!(
            "SELECT {ENCOUNTER_COLUMNS} FROM encounter WHERE crew_member_id = ?1 ORDER BY created_at_tai DESC, id"
        ))?;
        let rows = stmt.query_map([crew], encounter_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Adds a check-in.
    pub fn insert_checkin(&self, c: &CheckinRecord) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO checkin (id, crew_member_id, answers, privacy_tier, at_tai) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![c.id, c.crew_member_id, c.answers, c.privacy_tier, c.at_tai],
        )?;
        Ok(())
    }

    /// A crew member's check-ins in the given tiers, newest first.
    pub fn checkins(&self, crew: &str, tiers: &[&str]) -> Result<Vec<CheckinRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(
            "SELECT id, crew_member_id, answers, privacy_tier, at_tai FROM checkin
             WHERE crew_member_id = ?1 ORDER BY at_tai DESC, id",
        )?;
        let rows = stmt.query_map([crew], |r| {
            Ok(CheckinRecord {
                id: r.get(0)?,
                crew_member_id: r.get(1)?,
                answers: r.get(2)?,
                privacy_tier: r.get(3)?,
                at_tai: r.get(4)?,
            })
        })?;
        let all: Vec<CheckinRecord> = rows.collect::<Result<_, _>>()?;
        Ok(all
            .into_iter()
            .filter(|c| tiers.contains(&c.privacy_tier.as_str()))
            .collect())
    }

    /// Adds a checklist run.
    pub fn insert_checklist_run(&self, r: &ChecklistRunRecord) -> Result<(), StoreError> {
        self.connection().execute(
            &format!("INSERT INTO checklist_run ({RUN_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"),
            rusqlite::params![
                r.id,
                r.protocol_id,
                r.protocol_version,
                r.alarm_id,
                r.crew_member_id,
                r.started_by,
                r.steps,
                r.started_at_tai,
                r.completed_at_tai
            ],
        )?;
        Ok(())
    }

    /// Updates an open run's steps and completion; `false` if it is completed already or absent.
    pub fn update_checklist_run(
        &self,
        id: &str,
        steps: &str,
        completed_at_tai: Option<i64>,
    ) -> Result<bool, StoreError> {
        let n = self.connection().execute(
            "UPDATE checklist_run SET steps = ?2, completed_at_tai = ?3 WHERE id = ?1 AND completed_at_tai IS NULL",
            rusqlite::params![id, steps, completed_at_tai],
        )?;
        Ok(n == 1)
    }

    /// One checklist run.
    pub fn checklist_run(&self, id: &str) -> Result<Option<ChecklistRunRecord>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {RUN_COLUMNS} FROM checklist_run WHERE id = ?1"),
                [id],
                run_from_row,
            )
            .optional()?)
    }

    /// Checklist runs, newest first.
    pub fn checklist_runs(&self) -> Result<Vec<ChecklistRunRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM checklist_run ORDER BY started_at_tai DESC, id"
        ))?;
        let rows = stmt.query_map([], run_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Adds a medication or kit entry.
    pub fn insert_medication(&self, m: &MedicationRecord) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO medication_entry (id, crew_member_id, kind, item, amount, unit, note, by_user, at_tai)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![m.id, m.crew_member_id, m.kind, m.item, m.amount, m.unit, m.note, m.by_user, m.at_tai],
        )?;
        Ok(())
    }

    /// The medication and kit log, newest first.
    pub fn medication_log(&self) -> Result<Vec<MedicationRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(
            "SELECT id, crew_member_id, kind, item, amount, unit, note, by_user, at_tai FROM medication_entry
             ORDER BY at_tai DESC, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(MedicationRecord {
                id: r.get(0)?,
                crew_member_id: r.get(1)?,
                kind: r.get(2)?,
                item: r.get(3)?,
                amount: r.get(4)?,
                unit: r.get(5)?,
                note: r.get(6)?,
                by_user: r.get(7)?,
                at_tai: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Adds attachment metadata.
    pub fn insert_attachment(&self, a: &AttachmentRecord) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO attachment (id, kind, content_hash, metadata, crew_member_id, filename, created_at_tai)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![a.id, a.kind, a.content_hash, a.metadata, a.crew_member_id, a.filename, a.created_at_tai],
        )?;
        Ok(())
    }

    /// Attachments, newest first (optionally for one crew member).
    pub fn attachments(&self, crew: Option<&str>) -> Result<Vec<AttachmentRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(
            "SELECT id, kind, content_hash, metadata, crew_member_id, filename, created_at_tai FROM attachment
             WHERE ?1 IS NULL OR crew_member_id = ?1 ORDER BY created_at_tai DESC, id",
        )?;
        let rows = stmt.query_map([crew], |r| {
            Ok(AttachmentRecord {
                id: r.get(0)?,
                kind: r.get(1)?,
                content_hash: r.get(2)?,
                metadata: r.get(3)?,
                crew_member_id: r.get(4)?,
                filename: r.get(5)?,
                created_at_tai: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
