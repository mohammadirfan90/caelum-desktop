//! Clinical workflows (CR-019; F-CLN-01, F-CLN-02, F-CLN-04, F-CLN-05, F-CLN-06, F-CLN-07,
//! F-CLN-08): crew overview, encounters with sign-off, the private check-in, the PVT result,
//! the protocol library and checklist runs, the medication and kit log, encrypted attachments.
//! Every command is role- and crew-scoped (ROLE_MATRIX §2) and every change is audited.

use caelum_alarms::signing::{built_in_keys, verify_config, TrustPolicy};
use caelum_domain::ids::{AlarmId, CrewMemberId};
use caelum_domain::observation::{Classification, MeasurementMode, Quality};
use caelum_domain::parameters::Parameter;
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::{Command, Denied, DeniedReason, Role, UserIdentity};
use caelum_store::{
    AttachmentRecord, CheckinRecord, ChecklistRunRecord, EncounterRecord, MedicationRecord,
    ObservationRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::reads::ValueStatus;
use super::{Station, StationError};

/// Configuration kind of the protocol library.
pub const PROTOCOLS_KIND: &str = "protocols";
/// A PVT response at or above this many milliseconds is a lapse (⚕ DRAFT, CR-019 §3).
pub const PVT_LAPSE_MS: u32 = 500;
/// Fewer responses than this store nothing.
pub const PVT_MIN_RESPONSES: usize = 10;
/// Largest attachment accepted (256 MiB).
pub const ATTACHMENT_MAX: usize = 256 * 1024 * 1024;

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

fn denied(command: Command) -> StationError {
    StationError::Denied(Denied {
        command,
        reason: DeniedReason::Role,
    })
}

fn new_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}

/// One crew member in the overview (F-CLN-01).
#[derive(Debug, Clone, PartialEq)]
pub struct CrewOverviewItem {
    /// Crew member ID.
    pub crew_member_id: String,
    /// Name.
    pub name: String,
    /// Highest open alarm priority (`critical` / `warning` / `observe` / `advisory`), if any.
    pub top_priority: Option<String>,
    /// Newest heart rate, SpO2, breathing rate and temperature with freshness.
    pub vitals: Vec<(ObservationRecord, ValueStatus)>,
}

/// A recorded PVT run.
#[derive(Debug, Clone, PartialEq)]
pub struct PvtResult {
    /// Median reaction time, ms.
    pub median_ms: f64,
    /// Responses at or above [`PVT_LAPSE_MS`].
    pub lapses: u32,
    /// Responses counted.
    pub responses: u32,
}

/// Check-in answers (F-CLN-07).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckinAnswers {
    /// Mood, 1 (very low) to 5 (very good).
    pub mood: u8,
    /// Symptoms, as the crew member wrote them.
    pub symptoms: Vec<String>,
    /// Hours slept, as reported.
    pub sleep_hours: Option<f64>,
}

/// One protocol of the signed library (F-CLN-02).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Protocol {
    /// Protocol ID.
    pub id: String,
    /// Protocol version.
    pub version: String,
    /// Title.
    pub title: String,
    /// Alarm rule IDs (or prefixes ending in `*`) it applies to.
    #[serde(default)]
    pub rules: Vec<String>,
    /// Ordered steps.
    pub steps: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolFile {
    library: LibraryHead,
    #[serde(default, rename = "protocol")]
    protocols: Vec<Protocol>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LibraryHead {
    version: String,
}

/// A step state in a checklist run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    /// Not done yet.
    Open,
    /// Done.
    Done,
    /// Skipped on purpose.
    Skipped,
}

fn parse_protocols(bytes: &[u8], signed_version: &str) -> Result<Vec<Protocol>, StationError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| rejected("the protocol library is not UTF-8 text"))?;
    let file: ProtocolFile = toml::from_str(text)
        .map_err(|e| rejected(format!("the protocol library does not parse: {e}")))?;
    if file.library.version != signed_version {
        return Err(rejected("the library version is not the signed version"));
    }
    let mut seen = std::collections::HashSet::new();
    for p in &file.protocols {
        if p.id.trim().is_empty() || p.title.trim().is_empty() || p.steps.is_empty() {
            return Err(rejected(format!(
                "protocol {} needs an ID, a title and steps",
                p.id
            )));
        }
        if !seen.insert(p.id.clone()) {
            return Err(rejected(format!("protocol {} is listed twice", p.id)));
        }
    }
    Ok(file.protocols)
}

impl Station {
    /// Data about `crew` for `command`: an astronaut only their own (ROLE_MATRIX §2).
    fn crew_scope(
        &mut self,
        command: Command,
        crew: CrewMemberId,
    ) -> Result<UserIdentity, StationError> {
        let user = self.user_for(command)?;
        if user.role == Role::Astronaut && user.crew_member_id != Some(crew) {
            return Err(denied(command));
        }
        Ok(user)
    }

    /// `crew_overview` (F-CLN-01): every active crew member's top alarm and key vitals.
    pub fn crew_overview(&mut self) -> Result<Vec<CrewOverviewItem>, StationError> {
        self.user_for(Command::CrewOverview)?;
        let now = self.now()?;
        let open = self.db.alarms_recent(i64::MAX)?;
        let rank = |p: &str| match p {
            "critical" => 4,
            "warning" => 3,
            "observe" => 2,
            "advisory" => 1,
            _ => 0,
        };
        let mut out = Vec::new();
        for c in self.db.crew_members()?.into_iter().filter(|c| c.active) {
            let top = open
                .iter()
                .filter(|a| {
                    a.crew_member_id.as_deref() == Some(c.id.as_str())
                        && a.resolved_at_tai.is_none()
                })
                .max_by_key(|a| rank(&a.priority))
                .map(|a| a.priority.clone());
            let vitals = self
                .db
                .latest_observations(&c.id)?
                .into_iter()
                .filter(|o| {
                    [
                        "heart_rate",
                        "spo2",
                        "respiration_rate",
                        "body_temp_measured",
                    ]
                    .contains(&o.parameter.as_str())
                })
                .map(|o| {
                    let age = (now.0 - o.measured_at_tai) / 1_000_000_000;
                    let stale = Parameter::from_key(&o.parameter)
                        .map_or(i64::MAX, |p| i64::from(p.spec().stale_after_s));
                    let status = if o.quality == "unusable" {
                        ValueStatus::Unusable
                    } else if age > stale {
                        ValueStatus::Stale
                    } else {
                        ValueStatus::Live
                    };
                    (o, status)
                })
                .collect();
            out.push(CrewOverviewItem {
                crew_member_id: c.id,
                name: c.name,
                top_priority: top,
                vitals,
            });
        }
        Ok(out)
    }

    /// `encounter_list`: a crew member's encounters; an astronaut sees only their signed ones.
    pub fn encounter_list(
        &mut self,
        crew: CrewMemberId,
    ) -> Result<Vec<EncounterRecord>, StationError> {
        let user = self.crew_scope(Command::EncounterList, crew)?;
        let all = self.db.encounters(&crew.to_string())?;
        Ok(if user.role == Role::Astronaut {
            all.into_iter()
                .filter(|e| e.signed_at_tai.is_some())
                .collect()
        } else {
            all
        })
    }

    /// `encounter_save` (F-CLN-05): creates an encounter with a vitals snapshot, or updates the
    /// caller's own unsigned one. Audited.
    pub fn encounter_save(
        &mut self,
        id: Option<&str>,
        crew: CrewMemberId,
        soap: [Option<String>; 4],
        attachments: &[String],
    ) -> Result<String, StationError> {
        let user = self.user_for(Command::EncounterSave)?;
        let [subjective, objective, assessment, plan] = soap;
        let attachments = Some(serde_json::Value::from(attachments.to_vec()).to_string());
        let me = user.user_id.to_string();
        let (id, action) = match id {
            Some(id) => {
                let existing = self
                    .db
                    .encounter(id)?
                    .ok_or_else(|| rejected("no such encounter"))?;
                if existing.author != me {
                    return Err(rejected(
                        "only the author edits an encounter before sign-off",
                    ));
                }
                let updated = EncounterRecord {
                    subjective,
                    objective,
                    assessment,
                    plan,
                    attachments,
                    ..existing
                };
                if !self.db.update_unsigned_encounter(&updated)? {
                    return Err(rejected(
                        "a signed encounter cannot change; write a new one",
                    ));
                }
                (id.to_owned(), "updated")
            }
            None => {
                if !self
                    .db
                    .crew_member(&crew.to_string())?
                    .is_some_and(|c| c.active)
                {
                    return Err(rejected("the crew member is not on the roster in force"));
                }
                let now = self.now()?;
                let snapshot: Vec<serde_json::Value> = self
                    .db
                    .latest_observations(&crew.to_string())?
                    .into_iter()
                    .map(|o| {
                        json!({
                            "parameter": o.parameter, "value": o.value, "text": o.value_text, "unit": o.unit,
                            "measured_at_tai": o.measured_at_tai, "device_id": o.device_id, "quality": o.quality,
                        })
                    })
                    .collect();
                let id = new_id();
                self.db.insert_encounter(&EncounterRecord {
                    id: id.clone(),
                    crew_member_id: crew.to_string(),
                    author: me,
                    subjective,
                    objective,
                    assessment,
                    plan,
                    vitals: Some(serde_json::Value::from(snapshot).to_string()),
                    created_at_tai: Some(now.0),
                    signed_at_tai: None,
                    attachments,
                })?;
                (id, "created")
            }
        };
        self.log().append(
            &Actor::user(&user),
            AuditAction::EncounterSaved,
            Some(&id),
            &json!({ "crew_member": crew.to_string(), "change": action }),
        )?;
        Ok(id)
    }

    /// `encounter_sign`: the author signs after re-authentication; immutable afterwards. Audited.
    pub fn encounter_sign(&mut self, id: &str) -> Result<(), StationError> {
        let user = self.admin_for(Command::EncounterSign)?;
        let e = self
            .db
            .encounter(id)?
            .ok_or_else(|| rejected("no such encounter"))?;
        if e.author != user.user_id.to_string() {
            return Err(rejected("only the author signs an encounter"));
        }
        let now = self.now()?;
        if !self.db.sign_encounter(id, now.0)? {
            return Err(rejected("the encounter is signed already"));
        }
        self.log().append(
            &Actor::user(&user),
            AuditAction::EncounterSigned,
            Some(id),
            &json!({}),
        )?;
        Ok(())
    }

    /// `checkin_submit` (F-CLN-07): the astronaut's own check-in in the tier they choose. The
    /// audit records the submission, never its content.
    pub fn checkin_submit(
        &mut self,
        answers: &CheckinAnswers,
        tier: &str,
    ) -> Result<(), StationError> {
        let user = self.user_for(Command::CheckinSubmit)?;
        let crew = user
            .crew_member_id
            .ok_or_else(|| rejected("the account belongs to no crew member"))?;
        if !["private", "medical", "status"].contains(&tier) {
            return Err(rejected("unknown privacy tier"));
        }
        if !(1..=5).contains(&answers.mood) {
            return Err(rejected("mood is 1 to 5"));
        }
        if answers
            .sleep_hours
            .is_some_and(|h| !(0.0..=24.0).contains(&h))
        {
            return Err(rejected("sleep hours are 0 to 24"));
        }
        let now = self.now()?;
        let id = new_id();
        self.db.insert_checkin(&CheckinRecord {
            id: id.clone(),
            crew_member_id: crew.to_string(),
            answers: serde_json::to_string(answers).map_err(|e| rejected(e.to_string()))?,
            privacy_tier: tier.to_owned(),
            at_tai: now.0,
        })?;
        // Neither content nor tier: a private check-in stays private, also in the audit log.
        self.log().append(
            &Actor::user(&user),
            AuditAction::CheckinSubmitted,
            Some(&id),
            &json!({}),
        )?;
        Ok(())
    }

    /// `checkin_list`: own check-ins (all tiers) for the astronaut; the medical and status tiers
    /// for the CMO and flight surgeon (ROLE_MATRIX §2). Private stays private.
    pub fn checkin_list(&mut self, crew: CrewMemberId) -> Result<Vec<CheckinRecord>, StationError> {
        let user = self.crew_scope(Command::CheckinList, crew)?;
        let tiers: &[&str] = if user.role == Role::Astronaut {
            &["private", "medical", "status"]
        } else {
            &["medical", "status"]
        };
        Ok(self.db.checkins(&crew.to_string(), tiers)?)
    }

    /// `pvt_submit` (F-CLN-06): the astronaut's reaction times from one PVT run; the median is
    /// stored as `pvt_median_rt`, measured by the station. Too few responses store nothing.
    pub fn pvt_submit(&mut self, reaction_ms: &[u32]) -> Result<PvtResult, StationError> {
        let user = self.user_for(Command::PvtSubmit)?;
        let crew = user
            .crew_member_id
            .ok_or_else(|| rejected("the account belongs to no crew member"))?;
        if reaction_ms.len() < PVT_MIN_RESPONSES {
            return Err(rejected(format!(
                "a PVT run needs at least {PVT_MIN_RESPONSES} responses"
            )));
        }
        if reaction_ms.iter().any(|&r| !(100..=30_000).contains(&r)) {
            return Err(rejected(
                "reaction times must be 100 ms to 30 s (false starts are not counted)",
            ));
        }
        let mut sorted = reaction_ms.to_vec();
        sorted.sort_unstable();
        let mid = sorted.len() / 2;
        let median_ms = if sorted.len().is_multiple_of(2) {
            (f64::from(sorted[mid - 1]) + f64::from(sorted[mid])) / 2.0
        } else {
            f64::from(sorted[mid])
        };
        let lapses = u32::try_from(sorted.iter().filter(|&&r| r >= PVT_LAPSE_MS).count())
            .unwrap_or(u32::MAX);
        let responses = u32::try_from(sorted.len()).unwrap_or(u32::MAX);
        let now = self.now()?;
        let spec = Parameter::PvtMedianRt.spec();
        let id = new_id();
        self.db.insert_observation(&ObservationRecord {
            id: id.clone(),
            crew_member_id: crew.to_string(),
            device_id: None,
            parameter: spec.key.to_owned(),
            value: Some(median_ms),
            value_text: None,
            unit: spec.unit.ucum().to_owned(),
            measured_at_tai: now.0,
            received_at_tai: now.0,
            quality: quality_name(Quality::Good).to_owned(),
            classification: classification_name(Classification::Measured).to_owned(),
            measurement_mode: mode_name(MeasurementMode::Spot).to_owned(),
            algorithm_version: Some("pvt-median-1".to_owned()),
            context: Some(
                json!({ "source": "station-pvt", "responses": responses, "lapses": lapses })
                    .to_string(),
            ),
        })?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::PvtRecorded,
            Some(&id),
            &json!({}),
        )?;
        Ok(PvtResult {
            median_ms,
            lapses,
            responses,
        })
    }

    /// `medication_log`: doses (own only for an astronaut) and kit entries.
    pub fn medication_log(&mut self) -> Result<Vec<MedicationRecord>, StationError> {
        let user = self.user_for(Command::MedicationLog)?;
        let all = self.db.medication_log()?;
        Ok(if user.role == Role::Astronaut {
            let own = user.crew_member_id.map(|c| c.to_string());
            all.into_iter()
                .filter(|m| m.crew_member_id.is_some() && m.crew_member_id == own)
                .collect()
        } else {
            all
        })
    }

    /// `medication_add` (F-CLN-08): a dose to a crew member, or kit use or restock. Audited.
    pub fn medication_add(
        &mut self,
        crew: Option<CrewMemberId>,
        kind: &str,
        item: &str,
        amount: Option<f64>,
        unit: Option<String>,
        note: Option<String>,
    ) -> Result<String, StationError> {
        let user = self.user_for(Command::MedicationAdd)?;
        if !["dose", "kit_use", "kit_restock"].contains(&kind) {
            return Err(rejected("unknown entry kind"));
        }
        if (kind == "dose") != crew.is_some() {
            return Err(rejected(
                "a dose names the crew member; kit entries name none",
            ));
        }
        if item.trim().is_empty() {
            return Err(rejected("name the item"));
        }
        if amount.is_some_and(|a| !a.is_finite() || a < 0.0) || (amount.is_some() != unit.is_some())
        {
            return Err(rejected("an amount needs a unit and must be zero or more"));
        }
        let now = self.now()?;
        let id = new_id();
        self.db.insert_medication(&MedicationRecord {
            id: id.clone(),
            crew_member_id: crew.map(|c| c.to_string()),
            kind: kind.to_owned(),
            item: item.trim().to_owned(),
            amount,
            unit,
            note,
            by_user: user.user_id.to_string(),
            at_tai: now.0,
        })?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::MedicationLogged,
            Some(&id),
            &json!({ "kind": kind }),
        )?;
        Ok(id)
    }

    /// The protocol library in force (none until a signed library is applied).
    fn protocols(&self) -> Result<Vec<Protocol>, StationError> {
        let Some((version, _, content)) = self.db.latest_config_bundle(PROTOCOLS_KIND)? else {
            return Ok(Vec::new());
        };
        parse_protocols(content.as_bytes(), &version)
    }

    /// `protocol_list` (public, SR-014): the signed protocol library; needs no sign-in.
    pub fn protocol_list(&self) -> Result<Vec<Protocol>, StationError> {
        self.protocols()
    }

    /// `config_apply_protocols` (F-CLN-02, CR-019 §4): administrator, re-authentication. Audited.
    pub fn config_apply_protocols(
        &mut self,
        bundle: &[u8],
        signature: &str,
    ) -> Result<usize, StationError> {
        let user = self.admin_for(Command::ConfigApplyProtocols)?;
        let verified = verify_config(
            PROTOCOLS_KIND,
            bundle,
            signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| rejected(format!("the library signature is not accepted: {e}")))?;
        let protocols = parse_protocols(bundle, &verified.version)?;
        let content = std::str::from_utf8(bundle).map_err(|_| rejected("not UTF-8 text"))?;
        let now = self.now()?;
        self.db.put_config_bundle(
            &format!("{PROTOCOLS_KIND}-{}", verified.sha256),
            PROTOCOLS_KIND,
            &verified.version,
            signature,
            content,
            now.0,
        )?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some(PROTOCOLS_KIND),
            &json!({ "kind": PROTOCOLS_KIND, "version": verified.version, "sha256": verified.sha256,
                     "protocols": protocols.len(), "development": verified.development }),
        )?;
        Ok(protocols.len())
    }

    /// `checklist_start` (F-CLN-02): a run of a protocol, optionally linked to an alarm. Audited.
    pub fn checklist_start(
        &mut self,
        protocol_id: &str,
        crew: Option<CrewMemberId>,
        alarm: Option<AlarmId>,
    ) -> Result<String, StationError> {
        let user = self.user_for(Command::ChecklistStart)?;
        if user.role == Role::Astronaut && crew.is_some_and(|c| Some(c) != user.crew_member_id) {
            return Err(denied(Command::ChecklistStart));
        }
        let protocol = self
            .protocols()?
            .into_iter()
            .find(|p| p.id == protocol_id)
            .ok_or_else(|| rejected("no such protocol in the library in force"))?;
        if let Some(a) = alarm {
            if self.db.alarm_view(&a.to_string())?.is_none() {
                return Err(rejected("no such alarm"));
            }
        }
        let now = self.now()?;
        let id = new_id();
        let steps = vec![StepState::Open; protocol.steps.len()];
        self.db.insert_checklist_run(&ChecklistRunRecord {
            id: id.clone(),
            protocol_id: protocol.id.clone(),
            protocol_version: protocol.version.clone(),
            alarm_id: alarm.map(|a| a.to_string()),
            crew_member_id: crew.map(|c| c.to_string()),
            started_by: Some(user.user_id.to_string()),
            steps: serde_json::to_string(&steps).map_err(|e| rejected(e.to_string()))?,
            started_at_tai: now.0,
            completed_at_tai: None,
        })?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::ChecklistStarted,
            Some(&id),
            &json!({ "protocol": protocol.id, "version": protocol.version, "alarm": alarm.map(|a| a.to_string()) }),
        )?;
        Ok(id)
    }

    /// `checklist_update`: marks one step, or completes the run (no open step left). Audited.
    pub fn checklist_update(
        &mut self,
        run_id: &str,
        step: usize,
        state: StepState,
        complete: bool,
    ) -> Result<(), StationError> {
        let user = self.user_for(Command::ChecklistUpdate)?;
        let run = self
            .db
            .checklist_run(run_id)?
            .ok_or_else(|| rejected("no such checklist run"))?;
        if user.role == Role::Astronaut
            && run.started_by.as_deref() != Some(user.user_id.to_string().as_str())
        {
            return Err(denied(Command::ChecklistUpdate));
        }
        let mut steps: Vec<StepState> =
            serde_json::from_str(&run.steps).map_err(|e| rejected(e.to_string()))?;
        let slot = steps
            .get_mut(step)
            .ok_or_else(|| rejected("no such step"))?;
        *slot = state;
        if complete && steps.contains(&StepState::Open) {
            return Err(rejected(
                "every step must be done or skipped before completing",
            ));
        }
        let now = self.now()?;
        let text = serde_json::to_string(&steps).map_err(|e| rejected(e.to_string()))?;
        if !self
            .db
            .update_checklist_run(run_id, &text, complete.then_some(now.0))?
        {
            return Err(rejected("the checklist run is completed"));
        }
        self.log().append(
            &Actor::user(&user),
            if complete {
                AuditAction::ChecklistCompleted
            } else {
                AuditAction::ChecklistStep
            },
            Some(run_id),
            &json!({ "step": step, "state": state }),
        )?;
        Ok(())
    }

    /// `checklist_list`: runs in scope (an astronaut's own), newest first.
    pub fn checklist_list(&mut self) -> Result<Vec<ChecklistRunRecord>, StationError> {
        let user = self.user_for(Command::ChecklistList)?;
        let me = user.user_id.to_string();
        Ok(self
            .db
            .checklist_runs()?
            .into_iter()
            .filter(|r| {
                user.role != Role::Astronaut || r.started_by.as_deref() == Some(me.as_str())
            })
            .collect())
    }

    /// `attachment_import` (F-CLN-04, CR-019 §6): stores a file encrypted, with its metadata.
    /// Audited.
    pub fn attachment_import(
        &mut self,
        crew: CrewMemberId,
        kind: &str,
        filename: &str,
        bytes: &[u8],
        metadata: serde_json::Value,
    ) -> Result<String, StationError> {
        let user = self.user_for(Command::AttachmentImport)?;
        self.attachment_store(&Actor::user(&user), crew, kind, filename, bytes, metadata)
    }

    /// Stores an attachment encrypted and audits it as `actor` (manual import or folder watch).
    pub(super) fn attachment_store(
        &mut self,
        actor: &Actor,
        crew: CrewMemberId,
        kind: &str,
        filename: &str,
        bytes: &[u8],
        metadata: serde_json::Value,
    ) -> Result<String, StationError> {
        if !["dicom", "ecg", "file"].contains(&kind) {
            return Err(rejected("unknown attachment kind"));
        }
        if bytes.is_empty() || bytes.len() > ATTACHMENT_MAX {
            return Err(rejected("the file is empty or larger than 256 MiB"));
        }
        let id = new_id();
        let stored = self
            .attachments
            .put(&id, bytes)
            .map_err(|e| rejected(e.to_string()))?;
        let now = self.now()?;
        self.db.insert_attachment(&AttachmentRecord {
            id: id.clone(),
            kind: kind.to_owned(),
            content_hash: stored.sha256.clone(),
            metadata: metadata.to_string(),
            crew_member_id: Some(crew.to_string()),
            filename: Some(filename.to_owned()),
            created_at_tai: Some(now.0),
        })?;
        self.log().append(
            actor,
            AuditAction::AttachmentImported,
            Some(&id),
            &json!({ "kind": kind, "sha256": stored.sha256, "bytes": stored.len }),
        )?;
        Ok(id)
    }

    /// `attachment_list`: attachment metadata (optionally for one crew member).
    pub fn attachment_list(
        &mut self,
        crew: Option<CrewMemberId>,
    ) -> Result<Vec<AttachmentRecord>, StationError> {
        self.user_for(Command::AttachmentList)?;
        Ok(self
            .db
            .attachments(crew.map(|c| c.to_string()).as_deref())?)
    }

    /// `attachment_get`: the decrypted, integrity-checked file.
    pub fn attachment_get(
        &mut self,
        id: &str,
    ) -> Result<(AttachmentRecord, Vec<u8>), StationError> {
        self.user_for(Command::AttachmentGet)?;
        let record = self
            .db
            .attachments(None)?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| rejected("no such attachment"))?;
        let bytes = self
            .attachments
            .get(id, &record.content_hash)
            .map_err(|e| rejected(e.to_string()))?;
        Ok((record, bytes))
    }
}

fn quality_name(q: Quality) -> &'static str {
    match q {
        Quality::Good => "good",
        Quality::Degraded => "degraded",
        Quality::Poor => "poor",
        Quality::Unusable => "unusable",
    }
}

fn classification_name(c: Classification) -> &'static str {
    match c {
        Classification::Measured => "measured",
        Classification::Derived => "derived",
        Classification::Entered => "entered",
        Classification::Imported => "imported",
        #[cfg(feature = "test-equipment")]
        Classification::Simulated => "simulated",
    }
}

fn mode_name(m: MeasurementMode) -> &'static str {
    match m {
        MeasurementMode::Continuous => "continuous",
        MeasurementMode::Spot => "spot",
    }
}
