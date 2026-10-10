//! IPC commands for clinical workflows (CR-019; F-CLN-01, F-CLN-02, F-CLN-04, F-CLN-05,
//! F-CLN-06, F-CLN-07, F-CLN-08). Thin: scope, audit and checks are in the core.

use caelum_core::station::clinical::{CheckinAnswers, Protocol, StepState};
use caelum_core::station::{CrewOverviewItem, PvtResult};
use caelum_core::store::{
    AttachmentRecord, CheckinRecord, ChecklistRunRecord, EncounterRecord, MedicationRecord,
};
use caelum_domain::ids::{AlarmId, CrewMemberId};
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;

use crate::commands::{CommandError, ErrorKind};
use crate::reads::{utc_ms, Freshness, Reading};
use crate::state::AppState;

fn invalid(what: &str) -> CommandError {
    CommandError {
        kind: ErrorKind::Rejected,
        message: format!("not a valid {what}"),
        retry_after_s: None,
    }
}

fn crew(id: &str) -> Result<CrewMemberId, CommandError> {
    id.parse().map_err(|_| invalid("crew member id"))
}

/// One crew member in the overview.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OverviewEntry {
    /// Crew member.
    pub crew_member_id: String,
    /// Name.
    pub name: String,
    /// Highest open alarm priority.
    pub top_priority: Option<String>,
    /// Key vitals with freshness.
    pub vitals: Vec<OverviewVital>,
}

/// A key vital in the overview.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct OverviewVital {
    /// The value.
    pub reading: Reading,
    /// Freshness.
    pub status: Freshness,
}

impl From<CrewOverviewItem> for OverviewEntry {
    fn from(c: CrewOverviewItem) -> Self {
        use caelum_core::station::reads::ValueStatus;
        Self {
            crew_member_id: c.crew_member_id,
            name: c.name,
            top_priority: c.top_priority,
            vitals: c
                .vitals
                .into_iter()
                .map(|(o, s)| OverviewVital {
                    reading: o.into(),
                    status: match s {
                        ValueStatus::Live => Freshness::Live,
                        ValueStatus::Stale => Freshness::Stale,
                        ValueStatus::Unusable => Freshness::Unusable,
                    },
                })
                .collect(),
        }
    }
}

/// An encounter.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Encounter {
    /// ID.
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
    /// Vitals snapshot (JSON).
    pub vitals: Option<String>,
    /// Created, UTC ms.
    pub created_at_utc_ms: Option<f64>,
    /// Signed, UTC ms.
    pub signed_at_utc_ms: Option<f64>,
    /// Attachment IDs.
    pub attachments: Vec<String>,
}

impl From<EncounterRecord> for Encounter {
    fn from(e: EncounterRecord) -> Self {
        Self {
            attachments: e
                .attachments
                .as_deref()
                .and_then(|a| serde_json::from_str(a).ok())
                .unwrap_or_default(),
            id: e.id,
            crew_member_id: e.crew_member_id,
            author: e.author,
            subjective: e.subjective,
            objective: e.objective,
            assessment: e.assessment,
            plan: e.plan,
            vitals: e.vitals,
            created_at_utc_ms: e.created_at_tai.and_then(utc_ms),
            signed_at_utc_ms: e.signed_at_tai.and_then(utc_ms),
        }
    }
}

/// Check-in answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Checkin {
    /// Mood 1–5.
    pub mood: u8,
    /// Symptoms.
    pub symptoms: Vec<String>,
    /// Hours slept.
    pub sleep_hours: Option<f64>,
}

/// A stored check-in.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CheckinEntry {
    /// ID.
    pub id: String,
    /// Answers; `None` if the stored record cannot be read.
    pub answers: Option<Checkin>,
    /// Tier.
    pub privacy_tier: String,
    /// At, UTC ms.
    pub at_utc_ms: Option<f64>,
}

impl From<CheckinRecord> for CheckinEntry {
    fn from(c: CheckinRecord) -> Self {
        let answers = serde_json::from_str::<CheckinAnswers>(&c.answers)
            .ok()
            .map(|a| Checkin {
                mood: a.mood,
                symptoms: a.symptoms,
                sleep_hours: a.sleep_hours,
            });
        Self {
            id: c.id,
            answers,
            privacy_tier: c.privacy_tier,
            at_utc_ms: utc_ms(c.at_tai),
        }
    }
}

/// A PVT result.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Pvt {
    /// Median reaction time, ms.
    pub median_ms: f64,
    /// Lapses.
    pub lapses: u32,
    /// Responses.
    pub responses: u32,
}

impl From<PvtResult> for Pvt {
    fn from(p: PvtResult) -> Self {
        Self {
            median_ms: p.median_ms,
            lapses: p.lapses,
            responses: p.responses,
        }
    }
}

/// A medication or kit entry.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MedicationEntry {
    /// ID.
    pub id: String,
    /// Crew member (doses).
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
    /// At, UTC ms.
    pub at_utc_ms: Option<f64>,
}

impl From<MedicationRecord> for MedicationEntry {
    fn from(m: MedicationRecord) -> Self {
        Self {
            id: m.id,
            crew_member_id: m.crew_member_id,
            kind: m.kind,
            item: m.item,
            amount: m.amount,
            unit: m.unit,
            note: m.note,
            at_utc_ms: utc_ms(m.at_tai),
        }
    }
}

/// A protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolItem {
    /// ID.
    pub id: String,
    /// Version.
    pub version: String,
    /// Title.
    pub title: String,
    /// Rules it applies to.
    pub rules: Vec<String>,
    /// Steps.
    pub steps: Vec<String>,
}

impl From<Protocol> for ProtocolItem {
    fn from(p: Protocol) -> Self {
        Self {
            id: p.id,
            version: p.version,
            title: p.title,
            rules: p.rules,
            steps: p.steps,
        }
    }
}

/// A checklist step state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    /// Not done.
    Open,
    /// Done.
    Done,
    /// Skipped.
    Skipped,
}

/// A checklist run.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistRun {
    /// ID.
    pub id: String,
    /// Protocol.
    pub protocol_id: String,
    /// Protocol version.
    pub protocol_version: String,
    /// Linked alarm.
    pub alarm_id: Option<String>,
    /// Crew member.
    pub crew_member_id: Option<String>,
    /// Step states.
    pub steps: Vec<Step>,
    /// Started, UTC ms.
    pub started_at_utc_ms: Option<f64>,
    /// Completed, UTC ms.
    pub completed_at_utc_ms: Option<f64>,
}

impl From<ChecklistRunRecord> for ChecklistRun {
    fn from(r: ChecklistRunRecord) -> Self {
        let steps: Vec<StepState> = serde_json::from_str(&r.steps).unwrap_or_default();
        Self {
            id: r.id,
            protocol_id: r.protocol_id,
            protocol_version: r.protocol_version,
            alarm_id: r.alarm_id,
            crew_member_id: r.crew_member_id,
            steps: steps
                .into_iter()
                .map(|s| match s {
                    StepState::Open => Step::Open,
                    StepState::Done => Step::Done,
                    StepState::Skipped => Step::Skipped,
                })
                .collect(),
            started_at_utc_ms: utc_ms(r.started_at_tai),
            completed_at_utc_ms: r.completed_at_tai.and_then(utc_ms),
        }
    }
}

/// Attachment metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// ID.
    pub id: String,
    /// `dicom` / `ecg` / `file`.
    pub kind: String,
    /// SHA-256.
    pub sha256: String,
    /// Metadata (JSON).
    pub metadata: String,
    /// Crew member.
    pub crew_member_id: Option<String>,
    /// File name.
    pub filename: Option<String>,
    /// Imported, UTC ms.
    pub created_at_utc_ms: Option<f64>,
}

impl From<AttachmentRecord> for Attachment {
    fn from(a: AttachmentRecord) -> Self {
        Self {
            id: a.id,
            kind: a.kind,
            sha256: a.content_hash,
            metadata: a.metadata,
            crew_member_id: a.crew_member_id,
            filename: a.filename,
            created_at_utc_ms: a.created_at_tai.and_then(utc_ms),
        }
    }
}

/// Crew overview (CMO, flight surgeon).
#[tauri::command]
#[specta::specta]
pub async fn crew_overview(state: State<'_, AppState>) -> Result<Vec<OverviewEntry>, CommandError> {
    state.with(|s| Ok(s.crew_overview()?.into_iter().map(Into::into).collect()))
}

/// A crew member's encounters.
#[tauri::command]
#[specta::specta]
pub async fn encounter_list(
    state: State<'_, AppState>,
    crew_member_id: String,
) -> Result<Vec<Encounter>, CommandError> {
    let c = crew(&crew_member_id)?;
    state.with(|s| Ok(s.encounter_list(c)?.into_iter().map(Into::into).collect()))
}

/// Creates (no `id`) or updates an unsigned encounter; returns its ID.
#[tauri::command]
#[specta::specta]
#[allow(clippy::too_many_arguments)]
pub async fn encounter_save(
    state: State<'_, AppState>,
    id: Option<String>,
    crew_member_id: String,
    subjective: Option<String>,
    objective: Option<String>,
    assessment: Option<String>,
    plan: Option<String>,
    attachments: Vec<String>,
) -> Result<String, CommandError> {
    let c = crew(&crew_member_id)?;
    state.with(|s| {
        Ok(s.encounter_save(
            id.as_deref(),
            c,
            [subjective, objective, assessment, plan],
            &attachments,
        )?)
    })
}

/// Signs an encounter (author, re-authentication).
#[tauri::command]
#[specta::specta]
pub async fn encounter_sign(state: State<'_, AppState>, id: String) -> Result<(), CommandError> {
    state.with(|s| Ok(s.encounter_sign(&id)?))
}

/// Submits the own daily check-in in a privacy tier.
#[tauri::command]
#[specta::specta]
pub async fn checkin_submit(
    state: State<'_, AppState>,
    answers: Checkin,
    tier: String,
) -> Result<(), CommandError> {
    let a = CheckinAnswers {
        mood: answers.mood,
        symptoms: answers.symptoms,
        sleep_hours: answers.sleep_hours,
    };
    state.with(|s| Ok(s.checkin_submit(&a, &tier)?))
}

/// Check-ins the caller may read for a crew member.
#[tauri::command]
#[specta::specta]
pub async fn checkin_list(
    state: State<'_, AppState>,
    crew_member_id: String,
) -> Result<Vec<CheckinEntry>, CommandError> {
    let c = crew(&crew_member_id)?;
    state.with(|s| Ok(s.checkin_list(c)?.into_iter().map(Into::into).collect()))
}

/// Stores a PVT run.
#[tauri::command]
#[specta::specta]
pub async fn pvt_submit(
    state: State<'_, AppState>,
    reaction_ms: Vec<u32>,
) -> Result<Pvt, CommandError> {
    state.with(|s| Ok(s.pvt_submit(&reaction_ms)?.into()))
}

/// The medication and kit log.
#[tauri::command]
#[specta::specta]
pub async fn medication_log(
    state: State<'_, AppState>,
) -> Result<Vec<MedicationEntry>, CommandError> {
    state.with(|s| Ok(s.medication_log()?.into_iter().map(Into::into).collect()))
}

/// Records a dose (with crew member) or kit use/restock (without).
#[tauri::command]
#[specta::specta]
pub async fn medication_add(
    state: State<'_, AppState>,
    crew_member_id: Option<String>,
    kind: String,
    item: String,
    amount: Option<f64>,
    unit: Option<String>,
    note: Option<String>,
) -> Result<String, CommandError> {
    let c = crew_member_id.as_deref().map(crew).transpose()?;
    state.with(|s| Ok(s.medication_add(c, &kind, &item, amount, unit, note)?))
}

/// The signed protocol library (public, SR-014).
#[tauri::command]
#[specta::specta]
pub async fn protocol_list(state: State<'_, AppState>) -> Result<Vec<ProtocolItem>, CommandError> {
    state.with(|s| Ok(s.protocol_list()?.into_iter().map(Into::into).collect()))
}

/// Applies a signed protocol library (administrator, re-authentication).
#[tauri::command]
#[specta::specta]
pub async fn config_apply_protocols(
    state: State<'_, AppState>,
    bundle: String,
    signature: String,
) -> Result<u32, CommandError> {
    state.with(|s| {
        Ok(
            u32::try_from(s.config_apply_protocols(bundle.as_bytes(), &signature)?)
                .unwrap_or(u32::MAX),
        )
    })
}

/// Starts a checklist run.
#[tauri::command]
#[specta::specta]
pub async fn checklist_start(
    state: State<'_, AppState>,
    protocol_id: String,
    crew_member_id: Option<String>,
    alarm: Option<String>,
) -> Result<String, CommandError> {
    let c = crew_member_id.as_deref().map(crew).transpose()?;
    let a = alarm
        .as_deref()
        .map(|a| a.parse::<AlarmId>().map_err(|_| invalid("alarm id")))
        .transpose()?;
    state.with(|s| Ok(s.checklist_start(&protocol_id, c, a)?))
}

/// Marks a step, or completes the run.
#[tauri::command]
#[specta::specta]
pub async fn checklist_update(
    state: State<'_, AppState>,
    run: String,
    step: u32,
    mark: Step,
    complete: bool,
) -> Result<(), CommandError> {
    let st = match mark {
        Step::Open => StepState::Open,
        Step::Done => StepState::Done,
        Step::Skipped => StepState::Skipped,
    };
    let index = usize::try_from(step).map_err(|_| invalid("step"))?;
    state.with(|s| Ok(s.checklist_update(&run, index, st, complete)?))
}

/// Checklist runs in scope.
#[tauri::command]
#[specta::specta]
pub async fn checklist_list(state: State<'_, AppState>) -> Result<Vec<ChecklistRun>, CommandError> {
    state.with(|s| Ok(s.checklist_list()?.into_iter().map(Into::into).collect()))
}

/// Imports a file into the encrypted attachment store.
#[tauri::command]
#[specta::specta]
pub async fn attachment_import(
    state: State<'_, AppState>,
    crew_member_id: String,
    kind: String,
    filename: String,
    bytes: Vec<u8>,
) -> Result<String, CommandError> {
    let c = crew(&crew_member_id)?;
    // DICOM and ECG files must open before they are kept (CR-014).
    state.with(|s| Ok(s.file_import(c, &kind, &filename, &bytes)?))
}

/// Attachment metadata (optionally for one crew member).
#[tauri::command]
#[specta::specta]
pub async fn attachment_list(
    state: State<'_, AppState>,
    crew_member_id: Option<String>,
) -> Result<Vec<Attachment>, CommandError> {
    let c = crew_member_id.as_deref().map(crew).transpose()?;
    state.with(|s| Ok(s.attachment_list(c)?.into_iter().map(Into::into).collect()))
}

/// One decrypted attachment's bytes.
#[tauri::command]
#[specta::specta]
pub async fn attachment_get(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<u8>, CommandError> {
    state.with(|s| Ok(s.attachment_get(&id)?.1))
}

/// A FHIR R4 bundle (JSON) of a crew member's observations over the last `window_s`.
#[tauri::command]
#[specta::specta]
pub async fn export_fhir(
    state: State<'_, AppState>,
    crew_member_id: String,
    window_s: u32,
) -> Result<String, CommandError> {
    let c = crew(&crew_member_id)?;
    state.with(|s| Ok(s.export_fhir(c, window_s)?))
}

/// One parameter over the last `window_s` as CSV.
#[tauri::command]
#[specta::specta]
pub async fn export_csv(
    state: State<'_, AppState>,
    crew_member_id: String,
    parameter: String,
    window_s: u32,
) -> Result<String, CommandError> {
    let c = crew(&crew_member_id)?;
    state.with(|s| Ok(s.export_csv(c, &parameter, window_s)?))
}

/// One ECG lead for review (physical units).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EcgLead {
    /// Label from the file.
    pub label: String,
    /// Unit from the file.
    pub unit: String,
    /// Sample rate, Hz.
    pub sample_rate_hz: f64,
    /// Samples.
    pub samples: Vec<f64>,
}

/// A decoded ECG recording (F-CLN-03).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EcgRecordingView {
    /// `edf`, `aecg`, `scp-ecg` or `dicom-waveform`.
    pub format: String,
    /// Leads.
    pub leads: Vec<EcgLead>,
}

/// A display window (centre and width, rescaled units).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DicomWindow {
    /// Centre.
    pub center: f64,
    /// Width.
    pub width: f64,
}

/// One DICOM frame rendered for display (F-CLN-04).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DicomImage {
    /// Rows.
    pub rows: u32,
    /// Columns.
    pub columns: u32,
    /// Frames in the object.
    pub frames: u32,
    /// The frame shown.
    pub frame: u32,
    /// 1 (grey) or 3 (RGB).
    pub channels: u8,
    /// Photometric interpretation.
    pub photometric: String,
    /// 8-bit display samples, interleaved, row by row.
    pub pixels: Vec<u8>,
    /// Window applied (grey only).
    pub window: Option<DicomWindow>,
    /// Window presets from the file.
    pub presets: Vec<DicomWindow>,
    /// Pixel spacing (row, column) in mm, if stated.
    pub pixel_spacing_mm: Option<(f64, f64)>,
}

/// Outcome of a FHIR import.
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FhirImportResult {
    /// Observations stored and evaluated.
    pub imported: u32,
    /// What was not imported, and why.
    pub rejected: Vec<String>,
}

fn window(w: (f64, f64)) -> DicomWindow {
    DicomWindow {
        center: w.0,
        width: w.1,
    }
}

/// Imports observations from a FHIR R4 JSON resource or bundle for one crew member (F-ACQ-08).
#[tauri::command]
#[specta::specta]
pub async fn fhir_import(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    crew_member_id: String,
    filename: String,
    json: String,
) -> Result<FhirImportResult, CommandError> {
    use tauri_specta::Event as _;
    let c = crew(&crew_member_id)?;
    let (n, rejected, out) = state.with(|s| Ok(s.fhir_import(c, &filename, json.as_bytes())?))?;
    for e in crate::events::bus_events(&out) {
        let _ = e.emit(&app);
    }
    if n > 0 {
        let _ = crate::events::ObservationsChanged {
            crew_id: crew_member_id,
        }
        .emit(&app);
    }
    Ok(FhirImportResult {
        imported: u32::try_from(n).unwrap_or(u32::MAX),
        rejected,
    })
}

/// An imported ECG recording, decoded for review (F-CLN-03).
#[tauri::command]
#[specta::specta]
pub async fn ecg_view(
    state: State<'_, AppState>,
    id: String,
) -> Result<EcgRecordingView, CommandError> {
    let r = state.with(|s| Ok(s.ecg_view(&id)?))?;
    Ok(EcgRecordingView {
        format: r.format.to_owned(),
        leads: r
            .leads
            .into_iter()
            .map(|l| EcgLead {
                label: l.label,
                unit: l.unit,
                sample_rate_hz: l.sample_rate_hz,
                samples: l.samples,
            })
            .collect(),
    })
}

/// One frame of an imported DICOM image, rendered with the given window (F-CLN-04).
#[tauri::command]
#[specta::specta]
pub async fn dicom_view(
    state: State<'_, AppState>,
    id: String,
    frame: u32,
    window: Option<DicomWindow>,
) -> Result<DicomImage, CommandError> {
    let v = state.with(|s| Ok(s.dicom_view(&id, frame, window.map(|w| (w.center, w.width)))?))?;
    Ok(DicomImage {
        rows: v.rows,
        columns: v.columns,
        frames: v.frames,
        frame: v.frame,
        channels: v.channels,
        photometric: v.photometric,
        pixels: v.pixels,
        window: v.window.map(self::window),
        presets: v.presets.into_iter().map(self::window).collect(),
        pixel_spacing_mm: v.pixel_spacing_mm,
    })
}
