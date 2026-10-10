//! IPC read commands for the crew screens and device assignment (CR-012 §4). Thin: scope checks
//! and OPS filtering happen in the core. TAI times are decimal strings (no JavaScript rounding).

use caelum_core::station::reads::{
    AlarmDetail, AlarmItem, AlarmReport, DeviceItem, RuleStats, Trend, ValueStatus, Vital,
};
use caelum_core::store::{AlarmView, NotificationRecord, ObservationRecord, WaveformChunkRecord};
use caelum_domain::ids::{AlarmId, CrewMemberId, DeviceId};
use caelum_domain::time::{LeapSeconds, TaiNs};
use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::commands::{CommandError, ErrorKind};
use crate::state::AppState;

/// UTC as Unix milliseconds for display (docs/07 §5: times in UTC), from the bundled
/// leap-second table; `None` if the time cannot be converted (shown as NO DATA, never guessed).
pub fn utc_ms(tai: i64) -> Option<f64> {
    static LEAP: std::sync::LazyLock<Option<LeapSeconds>> =
        std::sync::LazyLock::new(|| LeapSeconds::bundled().ok());
    let (utc, _) = LEAP.as_ref()?.tai_to_utc(TaiNs(tai)).ok()?;
    Some((utc.unix_ns / 1_000_000) as f64)
}

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

/// Freshness of a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Freshness {
    /// Current.
    Live,
    /// Older than the parameter's stale limit.
    Stale,
    /// Marked unusable by the device.
    Unusable,
}

/// One stored value.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    /// Parameter key (`docs/05` §1).
    pub parameter: String,
    /// Numeric value.
    pub value: Option<f64>,
    /// State value (enumerated parameters).
    pub text: Option<String>,
    /// UCUM unit.
    pub unit: String,
    /// Measured at, TAI ns (decimal).
    pub measured_at_tai: String,
    /// Measured at, UTC Unix ms.
    pub measured_at_utc_ms: Option<f64>,
    /// `good` / `degraded` / `poor` / `unusable`.
    pub quality: String,
    /// `measured` / `derived` / `entered` / `imported` (`simulated` in test builds).
    pub classification: String,
    /// Source device ID.
    pub device_id: Option<String>,
}

impl From<ObservationRecord> for Reading {
    fn from(o: ObservationRecord) -> Self {
        Self {
            parameter: o.parameter,
            value: o.value,
            text: o.value_text,
            unit: o.unit,
            measured_at_tai: o.measured_at_tai.to_string(),
            measured_at_utc_ms: utc_ms(o.measured_at_tai),
            quality: o.quality,
            classification: o.classification,
            device_id: o.device_id,
        }
    }
}

/// The newest value of a parameter with its freshness and source.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct VitalItem {
    /// The value.
    pub reading: Reading,
    /// Freshness now.
    pub status: Freshness,
    /// Age in seconds.
    pub age_s: u32,
    /// Device model.
    pub device_model: Option<String>,
}

impl From<Vital> for VitalItem {
    fn from(v: Vital) -> Self {
        Self {
            reading: v.observation.into(),
            status: match v.status {
                ValueStatus::Live => Freshness::Live,
                ValueStatus::Stale => Freshness::Stale,
                ValueStatus::Unusable => Freshness::Unusable,
            },
            age_s: u32::try_from(v.age_s).unwrap_or(u32::MAX),
            device_model: v.device_model,
        }
    }
}

/// A personal baseline.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    /// Median.
    pub median: f64,
    /// Median absolute deviation.
    pub mad: f64,
    /// Samples.
    pub n: u32,
    /// Status.
    pub status: String,
}

/// A parameter over a window.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TrendData {
    /// Points, oldest first.
    pub points: Vec<Reading>,
    /// Personal baseline, if one exists.
    pub baseline: Option<Baseline>,
}

impl From<Trend> for TrendData {
    fn from(t: Trend) -> Self {
        Self {
            points: t.points.into_iter().map(Into::into).collect(),
            baseline: t.baseline.map(|b| Baseline {
                median: b.median,
                mad: b.mad,
                n: u32::try_from(b.n).unwrap_or(0),
                status: b.status,
            }),
        }
    }
}

/// One waveform chunk.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WaveformChunk {
    /// Start, TAI ns (decimal).
    pub start_tai: String,
    /// Start, UTC Unix ms.
    pub start_utc_ms: Option<f64>,
    /// Samples per second.
    pub sample_rate: f64,
    /// Encoding of `data`.
    pub encoding: String,
    /// Samples.
    pub data: Vec<u8>,
    /// Quality.
    pub quality: String,
}

impl From<WaveformChunkRecord> for WaveformChunk {
    fn from(w: WaveformChunkRecord) -> Self {
        Self {
            start_tai: w.start_tai.to_string(),
            start_utc_ms: utc_ms(w.start_tai),
            sample_rate: w.sample_rate,
            encoding: w.encoding,
            data: w.data,
            quality: w.quality,
        }
    }
}

/// An alarm as listed. For MCC only the OPS fields are filled (docs/06 §3a).
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AlarmEntry {
    /// Alarm ID.
    pub id: String,
    /// Crew member.
    pub crew_member_id: Option<String>,
    /// `physiological` / `technical`.
    pub kind: String,
    /// Category.
    pub category: String,
    /// Priority.
    pub priority: String,
    /// State.
    pub state: String,
    /// Title (physiological alarms; never for MCC).
    pub title: Option<String>,
    /// Rule ID (empty for MCC).
    pub rule_id: String,
    /// Raised at, TAI ns.
    pub created_at_tai: String,
    /// Raised at, UTC Unix ms.
    pub created_at_utc_ms: Option<f64>,
    /// Acknowledged at, UTC Unix ms.
    pub acknowledged_at_utc_ms: Option<f64>,
    /// Ended at, UTC Unix ms.
    pub resolved_at_utc_ms: Option<f64>,
    /// Acknowledged at.
    pub acknowledged_at_tai: Option<String>,
    /// Ground stage acknowledged at.
    pub ground_acknowledged_at_tai: Option<String>,
    /// Silenced until.
    pub silenced_until_tai: Option<String>,
    /// Ended at.
    pub resolved_at_tai: Option<String>,
    /// Escalation stage reached.
    pub escalation_stage: u32,
    /// Only the OPS fields are present.
    pub ops_only: bool,
}

fn tai(t: Option<i64>) -> Option<String> {
    t.map(|t| t.to_string())
}

fn entry(a: AlarmView, ops_only: bool) -> AlarmEntry {
    AlarmEntry {
        id: a.id,
        crew_member_id: a.crew_member_id,
        kind: a.kind,
        category: a.category,
        priority: a.priority,
        state: a.state,
        title: a.title,
        rule_id: a.rule_id,
        created_at_tai: a.created_at_tai.to_string(),
        created_at_utc_ms: utc_ms(a.created_at_tai),
        acknowledged_at_utc_ms: a.acknowledged_at_tai.and_then(utc_ms),
        resolved_at_utc_ms: a.resolved_at_tai.and_then(utc_ms),
        acknowledged_at_tai: tai(a.acknowledged_at_tai),
        ground_acknowledged_at_tai: tai(a.ground_acknowledged_at_tai),
        silenced_until_tai: tai(a.silenced_until_tai),
        resolved_at_tai: tai(a.resolved_at_tai),
        escalation_stage: u32::try_from(a.escalation_stage).unwrap_or(0),
        ops_only,
    }
}

impl From<AlarmItem> for AlarmEntry {
    fn from(i: AlarmItem) -> Self {
        entry(i.alarm, i.ops_only)
    }
}

/// A delivered escalation stage.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StageDelivered {
    /// Audience.
    pub audience: String,
    /// Stage index.
    pub stage: u32,
    /// Delivered at, TAI ns.
    pub at_tai: String,
    /// Delivered at, UTC Unix ms.
    pub at_utc_ms: Option<f64>,
}

impl From<NotificationRecord> for StageDelivered {
    fn from(n: NotificationRecord) -> Self {
        Self {
            audience: n.audience,
            stage: u32::try_from(n.stage).unwrap_or(0),
            at_tai: n.at_tai.to_string(),
            at_utc_ms: utc_ms(n.at_tai),
        }
    }
}

/// One alarm with its why and timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AlarmInfo {
    /// The alarm.
    pub alarm: AlarmEntry,
    /// Ruleset version in force when raised.
    pub ruleset_version: String,
    /// Triggering value.
    pub trigger_value: Option<f64>,
    /// Why (JSON text from the engine).
    pub why: String,
    /// Recommended action.
    pub action: Option<String>,
    /// Delivered escalation stages.
    pub timeline: Vec<StageDelivered>,
}

impl From<AlarmDetail> for AlarmInfo {
    fn from(d: AlarmDetail) -> Self {
        let a = d.alarm;
        Self {
            ruleset_version: a.ruleset_version.clone(),
            trigger_value: a.trigger_value,
            why: a.why.clone(),
            action: a.action.clone(),
            alarm: entry(a, false),
            timeline: d.notifications.into_iter().map(Into::into).collect(),
        }
    }
}

/// F-ALM-09 statistics for one rule and priority.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RuleStatsItem {
    /// Rule ID.
    pub rule_id: String,
    /// Priority.
    pub priority: String,
    /// Alarms raised.
    pub count: u32,
    /// Median seconds to acknowledge.
    pub median_ack_s: Option<f64>,
    /// Acknowledged before escalation beyond the crew member and CMO.
    pub acked_early: u32,
    /// Silenced.
    pub silenced: u32,
    /// Ended by themselves within 60 s without action.
    pub self_resolved: u32,
    /// Bursts of 3 or more within one hour for one crew member.
    pub repeat_bursts: u32,
}

impl From<RuleStats> for RuleStatsItem {
    fn from(r: RuleStats) -> Self {
        Self {
            rule_id: r.rule_id,
            priority: r.priority,
            count: r.count,
            median_ack_s: r.median_ack_s,
            acked_early: r.acked_early,
            silenced: r.silenced,
            self_resolved: r.self_resolved,
            repeat_bursts: r.repeat_bursts,
        }
    }
}

/// Alarms per day for one crew member.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CrewRate {
    /// Crew member.
    pub crew_member_id: String,
    /// Alarms per day.
    pub per_day: f64,
}

/// F-ALM-09 report.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AlarmReportData {
    /// Window start, TAI ns.
    pub from_tai: String,
    /// Window end, TAI ns.
    pub to_tai: String,
    /// Alarms raised in the window.
    pub history: Vec<AlarmEntry>,
    /// Per rule and priority.
    pub rules: Vec<RuleStatsItem>,
    /// Per crew member.
    pub per_crew: Vec<CrewRate>,
}

impl From<AlarmReport> for AlarmReportData {
    fn from(r: AlarmReport) -> Self {
        Self {
            from_tai: r.from_tai.to_string(),
            to_tai: r.to_tai.to_string(),
            history: r.history.into_iter().map(|a| entry(a, false)).collect(),
            rules: r.rules.into_iter().map(Into::into).collect(),
            per_crew: r
                .per_crew_per_day
                .into_iter()
                .map(|(crew_member_id, per_day)| CrewRate {
                    crew_member_id,
                    per_day,
                })
                .collect(),
        }
    }
}

/// A registered device with its health.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEntry {
    /// Device ID.
    pub id: String,
    /// Adapter (`ble-sig`, `ble-wristband`, `serial`, `test-equipment`, ...).
    pub adapter: String,
    /// Model.
    pub model: String,
    /// Vendor.
    pub vendor: String,
    /// Serial number.
    pub serial: String,
    /// Firmware.
    pub firmware: Option<String>,
    /// Assigned crew member.
    pub crew_member_id: Option<String>,
    /// User slot on a multi-user device.
    pub device_user: Option<u8>,
    /// Link status.
    pub status: String,
    /// Last seen, TAI ns.
    pub last_seen_tai: Option<String>,
    /// Last seen, UTC Unix ms.
    pub last_seen_utc_ms: Option<f64>,
    /// Measured clock offset, ms.
    pub clock_offset_ms: Option<f64>,
    /// Newest battery level, %.
    pub battery_pct: Option<f64>,
}

impl From<DeviceItem> for DeviceEntry {
    fn from(i: DeviceItem) -> Self {
        let d = i.device;
        Self {
            id: d.id,
            adapter: d.adapter,
            model: d.model,
            vendor: d.vendor,
            serial: d.serial,
            firmware: d.firmware,
            crew_member_id: d.crew_member_id,
            device_user: d.device_user,
            status: d.status,
            last_seen_tai: tai(d.last_seen_tai),
            last_seen_utc_ms: d.last_seen_tai.and_then(utc_ms),
            clock_offset_ms: d.clock_offset_ms,
            battery_pct: i.battery.and_then(|b| b.value),
        }
    }
}

/// Newest value per parameter for a crew member.
#[tauri::command]
#[specta::specta]
pub async fn vitals_latest(
    state: State<'_, AppState>,
    crew_member_id: String,
) -> Result<Vec<VitalItem>, CommandError> {
    let c = crew(&crew_member_id)?;
    state.with(|s| Ok(s.vitals_latest(c)?.into_iter().map(Into::into).collect()))
}

/// One parameter over the last `window_s` seconds (at most 24 h).
#[tauri::command]
#[specta::specta]
pub async fn vitals_trend(
    state: State<'_, AppState>,
    crew_member_id: String,
    parameter: String,
    window_s: u32,
) -> Result<TrendData, CommandError> {
    let c = crew(&crew_member_id)?;
    state.with(|s| Ok(s.vitals_trend(c, &parameter, window_s)?.into()))
}

/// Waveform chunks of one signal over the last `seconds` (at most 30 s).
#[tauri::command]
#[specta::specta]
pub async fn waveform_recent(
    state: State<'_, AppState>,
    crew_member_id: String,
    signal: String,
    seconds: u32,
) -> Result<Vec<WaveformChunk>, CommandError> {
    let c = crew(&crew_member_id)?;
    state.with(|s| {
        Ok(s.waveform_recent(c, &signal, seconds)?
            .into_iter()
            .map(Into::into)
            .collect())
    })
}

/// Open alarms and those ended in the last 24 h.
#[tauri::command]
#[specta::specta]
pub async fn alarm_list(state: State<'_, AppState>) -> Result<Vec<AlarmEntry>, CommandError> {
    state.with(|s| Ok(s.alarm_list()?.into_iter().map(Into::into).collect()))
}

/// One alarm's why, values and timeline.
#[tauri::command]
#[specta::specta]
pub async fn alarm_detail(
    state: State<'_, AppState>,
    alarm: String,
) -> Result<AlarmInfo, CommandError> {
    let id: AlarmId = alarm.parse().map_err(|_| invalid("alarm id"))?;
    state.with(|s| Ok(s.alarm_detail(id)?.into()))
}

/// Alarm history, statistics and fatigue indicators over the last `window_s` seconds.
#[tauri::command]
#[specta::specta]
pub async fn alarm_report(
    state: State<'_, AppState>,
    window_s: u32,
) -> Result<AlarmReportData, CommandError> {
    state.with(|s| Ok(s.alarm_report(window_s)?.into()))
}

/// The device registry with link health.
#[tauri::command]
#[specta::specta]
pub async fn device_list(state: State<'_, AppState>) -> Result<Vec<DeviceEntry>, CommandError> {
    state.with(|s| Ok(s.device_list()?.into_iter().map(Into::into).collect()))
}

/// Assigns a device (and user slot) to a crew member.
#[tauri::command]
#[specta::specta]
pub async fn device_assign(
    state: State<'_, AppState>,
    device_id: String,
    crew_member_id: String,
    device_user: Option<u8>,
) -> Result<(), CommandError> {
    let d: DeviceId = device_id.parse().map_err(|_| invalid("device id"))?;
    let c = crew(&crew_member_id)?;
    state.with(|s| Ok(s.device_assign(d, c, device_user)?))
}

/// Removes a device's assignment.
#[tauri::command]
#[specta::specta]
pub async fn device_unassign(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<(), CommandError> {
    let d: DeviceId = device_id.parse().map_err(|_| invalid("device id"))?;
    state.with(|s| Ok(s.device_unassign(d)?))
}
