//! Repositories for acquisition: the `device` registry table and `observation` (F-ACQ-09,
//! docs/05 §2). Observations are appended only; this module has no update or delete for them.

use rusqlite::{OptionalExtension, Row};

use crate::db::{Database, StoreError};

/// A row of the `device` table.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceRecord {
    /// Device ID (UUID text).
    pub id: String,
    /// Adapter name (e.g. `ble-sig`).
    pub adapter: String,
    /// Model.
    pub model: String,
    /// Vendor.
    pub vendor: String,
    /// Serial number.
    pub serial: String,
    /// Firmware version, if read from the device.
    pub firmware: Option<String>,
    /// Accuracy class, from the device's documentation.
    pub accuracy_class: Option<String>,
    /// Calibration due (TAI ns).
    pub calibration_due_tai: Option<i64>,
    /// Assigned crew member.
    pub crew_member_id: Option<String>,
    /// User slot on a multi-user device (CR-008 §4).
    pub device_user: Option<u8>,
    /// Registry status.
    pub status: String,
    /// Last time data arrived (TAI ns).
    pub last_seen_tai: Option<i64>,
    /// Last measured clock offset (ms).
    pub clock_offset_ms: Option<f64>,
}

/// A row of the `observation` table.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservationRecord {
    /// Observation ID (UUID text).
    pub id: String,
    /// Crew member.
    pub crew_member_id: String,
    /// Source device, if any.
    pub device_id: Option<String>,
    /// Catalog key.
    pub parameter: String,
    /// Numeric value (exactly one of `value` and `value_text`).
    pub value: Option<f64>,
    /// Enumerated value.
    pub value_text: Option<String>,
    /// UCUM unit.
    pub unit: String,
    /// TAI ns.
    pub measured_at_tai: i64,
    /// TAI ns.
    pub received_at_tai: i64,
    /// `good` / `degraded` / `poor` / `unusable`.
    pub quality: String,
    /// `measured` / `derived` / `entered` / `imported` (/ `simulated` in test-equipment builds).
    pub classification: String,
    /// `continuous` / `spot`.
    pub measurement_mode: String,
    /// Algorithm version for derived values.
    pub algorithm_version: Option<String>,
    /// Context and provenance JSON.
    pub context: Option<String>,
}

const DEVICE_COLUMNS: &str = "id, adapter, model, vendor, serial, firmware, accuracy_class, \
     calibration_due, crew_member_id, device_user, status, last_seen_tai, clock_offset_ms";
const OBSERVATION_COLUMNS: &str = "id, crew_member_id, device_id, parameter, value, value_text, \
     unit, measured_at_tai, received_at_tai, quality, classification, measurement_mode, \
     algorithm_version, context";

fn device_from_row(r: &Row<'_>) -> rusqlite::Result<DeviceRecord> {
    Ok(DeviceRecord {
        id: r.get(0)?,
        adapter: r.get(1)?,
        model: r.get(2)?,
        vendor: r.get(3)?,
        serial: r.get(4)?,
        firmware: r.get(5)?,
        accuracy_class: r.get(6)?,
        calibration_due_tai: r.get(7)?,
        crew_member_id: r.get(8)?,
        device_user: r.get(9)?,
        status: r.get(10)?,
        last_seen_tai: r.get(11)?,
        clock_offset_ms: r.get(12)?,
    })
}

fn observation_from_row(r: &Row<'_>) -> rusqlite::Result<ObservationRecord> {
    Ok(ObservationRecord {
        id: r.get(0)?,
        crew_member_id: r.get(1)?,
        device_id: r.get(2)?,
        parameter: r.get(3)?,
        value: r.get(4)?,
        value_text: r.get(5)?,
        unit: r.get(6)?,
        measured_at_tai: r.get(7)?,
        received_at_tai: r.get(8)?,
        quality: r.get(9)?,
        classification: r.get(10)?,
        measurement_mode: r.get(11)?,
        algorithm_version: r.get(12)?,
        context: r.get(13)?,
    })
}

impl Database {
    /// Adds a device.
    pub fn insert_device(&self, d: &DeviceRecord) -> Result<(), StoreError> {
        self.connection().execute(
            &format!(
                "INSERT INTO device ({DEVICE_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
            ),
            rusqlite::params![
                d.id,
                d.adapter,
                d.model,
                d.vendor,
                d.serial,
                d.firmware,
                d.accuracy_class,
                d.calibration_due_tai,
                d.crew_member_id,
                d.device_user,
                d.status,
                d.last_seen_tai,
                d.clock_offset_ms,
            ],
        )?;
        Ok(())
    }

    /// The device with this ID.
    pub fn device(&self, id: &str) -> Result<Option<DeviceRecord>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                &format!("SELECT {DEVICE_COLUMNS} FROM device WHERE id = ?1"),
                [id],
                device_from_row,
            )
            .optional()?)
    }

    /// The device with this vendor, model and serial.
    pub fn device_by_identity(
        &self,
        vendor: &str,
        model: &str,
        serial: &str,
    ) -> Result<Option<DeviceRecord>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                &format!(
                    "SELECT {DEVICE_COLUMNS} FROM device WHERE vendor = ?1 AND model = ?2 AND serial = ?3"
                ),
                [vendor, model, serial],
                device_from_row,
            )
            .optional()?)
    }

    /// Every device, by vendor, model and serial.
    pub fn devices(&self) -> Result<Vec<DeviceRecord>, StoreError> {
        let mut stmt = self.connection().prepare(&format!(
            "SELECT {DEVICE_COLUMNS} FROM device ORDER BY vendor, model, serial"
        ))?;
        let rows = stmt
            .query_map([], device_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Sets (or clears) a device's crew assignment and user slot. Returns false if no such device.
    pub fn set_device_assignment(
        &self,
        id: &str,
        crew_member_id: Option<&str>,
        device_user: Option<u8>,
    ) -> Result<bool, StoreError> {
        let n = self.connection().execute(
            "UPDATE device SET crew_member_id = ?2, device_user = ?3 WHERE id = ?1",
            rusqlite::params![id, crew_member_id, device_user],
        )?;
        Ok(n == 1)
    }

    /// Records the device's status, last data time and (if measured) clock offset.
    pub fn set_device_status(
        &self,
        id: &str,
        status: &str,
        last_seen_tai: Option<i64>,
        clock_offset_ms: Option<f64>,
    ) -> Result<bool, StoreError> {
        let n = self.connection().execute(
            "UPDATE device SET status = ?2,
                 last_seen_tai = COALESCE(?3, last_seen_tai),
                 clock_offset_ms = COALESCE(?4, clock_offset_ms)
             WHERE id = ?1",
            rusqlite::params![id, status, last_seen_tai, clock_offset_ms],
        )?;
        Ok(n == 1)
    }

    /// Appends an observation.
    pub fn insert_observation(&self, o: &ObservationRecord) -> Result<(), StoreError> {
        self.connection().execute(
            &format!(
                "INSERT INTO observation ({OBSERVATION_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
            ),
            rusqlite::params![
                o.id,
                o.crew_member_id,
                o.device_id,
                o.parameter,
                o.value,
                o.value_text,
                o.unit,
                o.measured_at_tai,
                o.received_at_tai,
                o.quality,
                o.classification,
                o.measurement_mode,
                o.algorithm_version,
                o.context,
            ],
        )?;
        Ok(())
    }

    /// A crew member's observations of one parameter, oldest first.
    pub fn observations(
        &self,
        crew_member_id: &str,
        parameter: &str,
    ) -> Result<Vec<ObservationRecord>, StoreError> {
        let mut stmt = self.connection().prepare(&format!(
            "SELECT {OBSERVATION_COLUMNS} FROM observation
             WHERE crew_member_id = ?1 AND parameter = ?2 ORDER BY measured_at_tai, id"
        ))?;
        let rows = stmt
            .query_map([crew_member_id, parameter], observation_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// A row of the `alarm` table (docs/05 §2).
#[derive(Debug, Clone, PartialEq)]
pub struct AlarmRecord {
    /// Alarm ID.
    pub id: String,
    /// Crew member (none for station-wide technical alarms).
    pub crew_member_id: Option<String>,
    /// `physiological` or `technical`.
    pub kind: String,
    /// Category.
    pub category: String,
    /// Rule ID (`technical.*` for technical alarms).
    pub rule_id: String,
    /// Ruleset version in force.
    pub ruleset_version: String,
    /// `critical` / `warning` / `observe` / `advisory`.
    pub priority: String,
    /// State.
    pub state: String,
    /// Spot readings behind a confirming alarm (JSON).
    pub confirming_readings: Option<String>,
    /// Latched until acknowledged.
    pub latched: bool,
    /// Triggering value.
    pub trigger_value: Option<f64>,
    /// Why (JSON).
    pub why: String,
    /// Action.
    pub action: Option<String>,
    /// TAI ns.
    pub created_at_tai: i64,
}

impl Database {
    /// Adds an alarm.
    pub fn insert_alarm(&self, a: &AlarmRecord) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO alarm (id, crew_member_id, kind, category, rule_id, ruleset_version, priority,
                 state, confirming_readings, latched, trigger_value, why, action, created_at_tai)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                a.id,
                a.crew_member_id,
                a.kind,
                a.category,
                a.rule_id,
                a.ruleset_version,
                a.priority,
                a.state,
                a.confirming_readings,
                i64::from(a.latched),
                a.trigger_value,
                a.why,
                a.action,
                a.created_at_tai,
            ],
        )?;
        Ok(())
    }

    /// Updates an alarm's state and why; `resolved_at_tai` when it ended.
    pub fn update_alarm(
        &self,
        id: &str,
        state: &str,
        why: Option<&str>,
        resolved_at_tai: Option<i64>,
    ) -> Result<bool, StoreError> {
        let n = self.connection().execute(
            "UPDATE alarm SET state = ?2, why = COALESCE(?3, why),
                 resolved_at_tai = COALESCE(?4, resolved_at_tai) WHERE id = ?1",
            rusqlite::params![id, state, why, resolved_at_tai],
        )?;
        Ok(n == 1)
    }

    /// Alarms not yet ended (no `resolved_at_tai`), oldest first.
    pub fn open_alarms(&self) -> Result<Vec<(String, String, String)>, StoreError> {
        let mut stmt = self.connection().prepare(
            "SELECT id, rule_id, state FROM alarm WHERE resolved_at_tai IS NULL ORDER BY created_at_tai, id",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Records a verified configuration bundle with its content (SR-009 fallback).
    pub fn put_config_bundle(
        &self,
        id: &str,
        kind: &str,
        version: &str,
        signature: &str,
        content: &str,
        applied_at_tai: i64,
    ) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO config_bundle (id, kind, version, signature, content, applied_at_tai)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (id) DO UPDATE SET applied_at_tai = excluded.applied_at_tai",
            rusqlite::params![id, kind, version, signature, content, applied_at_tai],
        )?;
        Ok(())
    }

    /// The most recently applied bundle of `kind` with content: (version, signature, content).
    pub fn latest_config_bundle(
        &self,
        kind: &str,
    ) -> Result<Option<(String, String, String)>, StoreError> {
        Ok(self
            .connection()
            .query_row(
                "SELECT version, signature, content FROM config_bundle
                 WHERE kind = ?1 AND content IS NOT NULL
                 ORDER BY applied_at_tai DESC LIMIT 1",
                [kind],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    }
}

/// An alarm that has not ended, with its escalation state (restart, SR-007).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenAlarmRecord {
    /// Alarm ID.
    pub id: String,
    /// Crew member.
    pub crew_member_id: Option<String>,
    /// `physiological` or `technical`.
    pub kind: String,
    /// Category.
    pub category: String,
    /// Rule ID.
    pub rule_id: String,
    /// Priority.
    pub priority: String,
    /// State.
    pub state: String,
    /// Latched.
    pub latched: bool,
    /// Creation (TAI ns).
    pub created_at_tai: i64,
    /// Next escalation stage.
    pub escalation_stage: i64,
    /// Acknowledged by (user ID).
    pub acknowledged_by: Option<String>,
    /// Silenced until (TAI ns).
    pub silenced_until_tai: Option<i64>,
}

impl Database {
    /// Persists the next escalation stage (SR-007).
    pub fn set_alarm_escalation_stage(&self, id: &str, stage: i64) -> Result<bool, StoreError> {
        let n = self.connection().execute(
            "UPDATE alarm SET escalation_stage = ?2 WHERE id = ?1",
            rusqlite::params![id, stage],
        )?;
        Ok(n == 1)
    }

    /// Records the onboard acknowledgement.
    pub fn set_alarm_acknowledged(
        &self,
        id: &str,
        by: &str,
        at_tai: i64,
    ) -> Result<bool, StoreError> {
        let n = self.connection().execute(
            "UPDATE alarm SET acknowledged_by = ?2, acknowledged_at_tai = ?3, silenced_until_tai = NULL
             WHERE id = ?1 AND acknowledged_at_tai IS NULL",
            rusqlite::params![id, by, at_tai],
        )?;
        Ok(n == 1)
    }

    /// Records the ground-stage acknowledgement (ROLE_MATRIX §5.4).
    pub fn set_alarm_ground_acknowledged(
        &self,
        id: &str,
        by: &str,
        at_tai: i64,
    ) -> Result<bool, StoreError> {
        let n = self.connection().execute(
            "UPDATE alarm SET ground_acknowledged_by = ?2, ground_acknowledged_at_tai = ?3
             WHERE id = ?1 AND ground_acknowledged_at_tai IS NULL",
            rusqlite::params![id, by, at_tai],
        )?;
        Ok(n == 1)
    }

    /// Records a silence (SR-008).
    pub fn set_alarm_silenced(&self, id: &str, until_tai: i64) -> Result<bool, StoreError> {
        let n = self.connection().execute(
            "UPDATE alarm SET silenced_until_tai = ?2 WHERE id = ?1",
            rusqlite::params![id, until_tai],
        )?;
        Ok(n == 1)
    }

    /// Every alarm that has not ended, oldest first.
    pub fn open_alarm_records(&self) -> Result<Vec<OpenAlarmRecord>, StoreError> {
        let mut stmt = self.connection().prepare(
            "SELECT id, crew_member_id, kind, category, rule_id, priority, state, latched,
                    created_at_tai, escalation_stage, acknowledged_by, silenced_until_tai
             FROM alarm WHERE resolved_at_tai IS NULL ORDER BY created_at_tai, id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(OpenAlarmRecord {
                    id: r.get(0)?,
                    crew_member_id: r.get(1)?,
                    kind: r.get(2)?,
                    category: r.get(3)?,
                    rule_id: r.get(4)?,
                    priority: r.get(5)?,
                    state: r.get(6)?,
                    latched: r.get::<_, i64>(7)? != 0,
                    created_at_tai: r.get(8)?,
                    escalation_stage: r.get(9)?,
                    acknowledged_by: r.get(10)?,
                    silenced_until_tai: r.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

impl Database {
    /// Records a delivered escalation stage; delivering a stage twice is refused (primary key).
    pub fn insert_alarm_notification(
        &self,
        alarm_id: &str,
        audience: &str,
        stage: i64,
        at_tai: i64,
    ) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO alarm_notification (alarm_id, audience, stage, at_tai) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![alarm_id, audience, stage, at_tai],
        )?;
        Ok(())
    }
}
