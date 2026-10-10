//! Read commands for the crew screens and device assignment (CR-012 §4; F-ACQ-09, F-ACQ-10,
//! F-ALM-09, SR-004). Physiology leaves the core only here, after the role and crew-scope check
//! (CLAUDE.md rule 6); MCC gets alarm status with the OPS fields only (docs/06 §3a).

use std::collections::HashMap;

use caelum_devices::registry::Registry;
use caelum_domain::ids::{AlarmId, CrewMemberId, DeviceId};
use caelum_domain::parameters::Parameter;
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::{Command, Denied, DeniedReason, Role, UserIdentity};
use caelum_store::{
    AlarmView, BaselineRecord, DeviceRecord, NotificationRecord, ObservationRecord,
    WaveformChunkRecord,
};
use serde_json::json;

use super::{Station, StationError};

const NS: i64 = 1_000_000_000;
/// Longest trend window (CR-012 §4).
pub const TREND_MAX_S: u32 = 24 * 3600;
/// Most points returned for a trend; longer series are thinned evenly.
pub const TREND_MAX_POINTS: usize = 2000;
/// Longest waveform window.
pub const WAVEFORM_MAX_S: u32 = 30;
/// Ended alarms stay in the alarm list this long.
pub const RECENT_ALARMS_S: i64 = 24 * 3600;
/// An alarm that ends by itself within this time without any action counts as a nuisance
/// indicator (CR-012 §5).
pub const SELF_RESOLVED_S: i64 = 60;

/// Freshness of a value (SR-004; docs/07 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueStatus {
    /// Within the parameter's `stale_after_s`.
    Live,
    /// Older than `stale_after_s`.
    Stale,
    /// The device marked it unusable.
    Unusable,
}

/// The newest value of one parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct Vital {
    /// The stored observation (value, unit, times, quality, device).
    pub observation: ObservationRecord,
    /// Freshness now.
    pub status: ValueStatus,
    /// Age in seconds now (never negative).
    pub age_s: i64,
    /// Device model, if the device is registered.
    pub device_model: Option<String>,
}

/// A parameter over a window.
#[derive(Debug, Clone, PartialEq)]
pub struct Trend {
    /// Points, oldest first (thinned to at most [`TREND_MAX_POINTS`]).
    pub points: Vec<ObservationRecord>,
    /// Personal baseline, if one was estimated (CR-012 §6).
    pub baseline: Option<BaselineRecord>,
}

/// One alarm for a list; physiological detail removed for MCC (docs/06 §3a).
#[derive(Debug, Clone, PartialEq)]
pub struct AlarmItem {
    /// The record (for MCC: title, rule id, trigger value, why and action are cleared).
    pub alarm: AlarmView,
    /// Only the OPS fields are present.
    pub ops_only: bool,
}

/// One alarm with its timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct AlarmDetail {
    /// The record.
    pub alarm: AlarmView,
    /// Delivered escalation stages.
    pub notifications: Vec<NotificationRecord>,
}

/// F-ALM-09 counts for one rule and priority (CR-012 §5).
#[derive(Debug, Clone, PartialEq)]
pub struct RuleStats {
    /// Rule ID.
    pub rule_id: String,
    /// Priority.
    pub priority: String,
    /// Alarms raised.
    pub count: u32,
    /// Median seconds from raise to acknowledgement (acknowledged alarms only).
    pub median_ack_s: Option<f64>,
    /// Acknowledged before escalation went beyond the crew member and the CMO.
    pub acked_early: u32,
    /// Silenced at least once.
    pub silenced: u32,
    /// Ended by themselves within [`SELF_RESOLVED_S`] with no action.
    pub self_resolved: u32,
    /// Raised 3 or more times within one hour for the same crew member.
    pub repeat_bursts: u32,
}

/// F-ALM-09 report over a window.
#[derive(Debug, Clone, PartialEq)]
pub struct AlarmReport {
    /// Window start, TAI ns.
    pub from_tai: i64,
    /// Window end, TAI ns.
    pub to_tai: i64,
    /// Alarms raised in the window, oldest first.
    pub history: Vec<AlarmView>,
    /// Per rule and priority.
    pub rules: Vec<RuleStats>,
    /// Per crew member: alarms per day over the window.
    pub per_crew_per_day: Vec<(String, f64)>,
}

/// A device with its health (F-ACQ-09, F-ACQ-10).
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceItem {
    /// Registry record (status, last seen, clock offset, assignment).
    pub device: DeviceRecord,
    /// Newest battery level (%), when the device reports one.
    pub battery: Option<ObservationRecord>,
}

fn denied(command: Command) -> StationError {
    StationError::Denied(Denied {
        command,
        reason: DeniedReason::Role,
    })
}

/// Median of a non-empty list.
fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    Some(if v.len().is_multiple_of(2) {
        (v[mid - 1] + v[mid]) / 2.0
    } else {
        v[mid]
    })
}

impl Station {
    /// Authorises `command` for data about `crew`: astronauts only their own (ROLE_MATRIX §2).
    fn for_crew(
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

    /// `vitals_latest`: the newest value per parameter, with source and freshness. Parameters
    /// never reported are absent (the screen shows NO DATA).
    pub fn vitals_latest(&mut self, crew: CrewMemberId) -> Result<Vec<Vital>, StationError> {
        self.for_crew(Command::VitalsLatest, crew)?;
        let now = self.now()?;
        let models: HashMap<String, String> = self
            .db
            .devices()?
            .into_iter()
            .map(|d| (d.id, d.model))
            .collect();
        Ok(self
            .db
            .latest_observations(&crew.to_string())?
            .into_iter()
            .map(|o| {
                let age_s = ((now.0 - o.measured_at_tai) / NS).max(0);
                let stale_after = Parameter::from_key(&o.parameter)
                    .map_or(i64::MAX, |p| i64::from(p.spec().stale_after_s));
                let status = if o.quality == "unusable" {
                    ValueStatus::Unusable
                } else if age_s > stale_after {
                    ValueStatus::Stale
                } else {
                    ValueStatus::Live
                };
                let device_model = o.device_id.as_ref().and_then(|d| models.get(d).cloned());
                Vital {
                    observation: o,
                    status,
                    age_s,
                    device_model,
                }
            })
            .collect())
    }

    /// `vitals_trend`: one parameter over the last `window_s` (at most 24 h).
    pub fn vitals_trend(
        &mut self,
        crew: CrewMemberId,
        parameter: &str,
        window_s: u32,
    ) -> Result<Trend, StationError> {
        self.for_crew(Command::VitalsTrend, crew)?;
        let p = Parameter::from_key(parameter)
            .ok_or_else(|| StationError::Rejected(format!("unknown parameter {parameter}")))?;
        let key = p.spec().key;
        let now = self.now()?;
        let from = now.0 - i64::from(window_s.min(TREND_MAX_S)) * NS;
        let crew = crew.to_string();
        let mut points = self.db.observations_between(&crew, key, from, now.0)?;
        if points.len() > TREND_MAX_POINTS {
            let step = points.len().div_ceil(TREND_MAX_POINTS);
            points = points.into_iter().step_by(step).collect();
        }
        Ok(Trend {
            points,
            baseline: self.db.baseline(&crew, key)?,
        })
    }

    /// `waveform_recent`: chunks of one signal over the last `seconds` (at most 30 s).
    pub fn waveform_recent(
        &mut self,
        crew: CrewMemberId,
        signal: &str,
        seconds: u32,
    ) -> Result<Vec<WaveformChunkRecord>, StationError> {
        self.for_crew(Command::WaveformRecent, crew)?;
        let now = self.now()?;
        let from = now.0 - i64::from(seconds.min(WAVEFORM_MAX_S)) * NS;
        Ok(self.db.waveform_chunks(&crew.to_string(), signal, from)?)
    }

    /// `alarm_list`: open alarms and those ended in the last 24 h, newest first, in the caller's
    /// scope. MCC gets the OPS fields only (docs/06 §3a).
    pub fn alarm_list(&mut self) -> Result<Vec<AlarmItem>, StationError> {
        let user = self.user_for(Command::AlarmList)?;
        let now = self.now()?;
        let own = user.crew_member_id.map(|c| c.to_string());
        let ops_only = user.role == Role::Mcc;
        Ok(self
            .db
            .alarms_recent(now.0 - RECENT_ALARMS_S * NS)?
            .into_iter()
            .filter(|a| user.role != Role::Astronaut || a.crew_member_id == own)
            .map(|mut a| {
                if ops_only {
                    a.title = None;
                    a.rule_id = String::new();
                    a.trigger_value = None;
                    a.why = String::new();
                    a.action = None;
                    a.ruleset_version = String::new();
                    a.acknowledged_by = None;
                    a.ground_acknowledged_by = None;
                }
                AlarmItem { alarm: a, ops_only }
            })
            .collect())
    }

    /// `alarm_detail`: the why, values and escalation timeline of one alarm in scope.
    pub fn alarm_detail(&mut self, id: AlarmId) -> Result<AlarmDetail, StationError> {
        let user = self.user_for(Command::AlarmDetail)?;
        let alarm = self
            .db
            .alarm_view(&id.to_string())?
            .ok_or_else(|| StationError::Rejected("no such alarm".to_owned()))?;
        if user.role == Role::Astronaut
            && alarm.crew_member_id != user.crew_member_id.map(|c| c.to_string())
        {
            return Err(denied(Command::AlarmDetail));
        }
        let notifications = self.db.alarm_notifications(&alarm.id)?;
        Ok(AlarmDetail {
            alarm,
            notifications,
        })
    }

    /// `alarm_report` (F-ALM-09): history, statistics and fatigue indicators over the last
    /// `window_s` seconds (CR-012 §5).
    pub fn alarm_report(&mut self, window_s: u32) -> Result<AlarmReport, StationError> {
        self.user_for(Command::AlarmReport)?;
        let now = self.now()?;
        let window_s = window_s.max(1);
        let from = now.0 - i64::from(window_s) * NS;
        let history = self.db.alarms_raised_between(from, now.0)?;
        let mut notes: HashMap<String, Vec<NotificationRecord>> = HashMap::new();
        for a in &history {
            notes.insert(a.id.clone(), self.db.alarm_notifications(&a.id)?);
        }
        Ok(report(from, now.0, history, &notes, window_s))
    }

    /// `device_list`: the registry with link health; an astronaut sees their own devices.
    pub fn device_list(&mut self) -> Result<Vec<DeviceItem>, StationError> {
        let user = self.user_for(Command::DeviceList)?;
        let own = user.crew_member_id.map(|c| c.to_string());
        let devices = self
            .db
            .devices()?
            .into_iter()
            .filter(|d| user.role != Role::Astronaut || d.crew_member_id == own);
        let mut out = Vec::new();
        for device in devices {
            let battery = self
                .db
                .latest_device_observation(&device.id, "battery_level")?;
            out.push(DeviceItem { device, battery });
        }
        Ok(out)
    }

    /// `device_assign` (F-ACQ-09): re-authentication required; the crew member must be on the
    /// roster in force. Audited.
    pub fn device_assign(
        &mut self,
        device: DeviceId,
        crew: CrewMemberId,
        device_user: Option<u8>,
    ) -> Result<(), StationError> {
        let user = self.admin_for(Command::DeviceAssign)?;
        let active = self
            .db
            .crew_member(&crew.to_string())?
            .is_some_and(|c| c.active);
        if !active {
            return Err(StationError::Rejected(
                "the crew member is not on the roster in force".to_owned(),
            ));
        }
        Registry::new(&self.db)
            .assign(device, crew, device_user)
            .map_err(|e| StationError::Rejected(e.to_string()))?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::DeviceAssigned,
            Some(&device.to_string()),
            &json!({ "crew_member": crew.to_string(), "device_user": device_user }),
        )?;
        Ok(())
    }

    /// `device_unassign`: re-authentication required. Audited.
    pub fn device_unassign(&mut self, device: DeviceId) -> Result<(), StationError> {
        let user = self.admin_for(Command::DeviceUnassign)?;
        Registry::new(&self.db)
            .unassign(device)
            .map_err(|e| StationError::Rejected(e.to_string()))?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::DeviceUnassigned,
            Some(&device.to_string()),
            &json!({}),
        )?;
        Ok(())
    }
}

/// Builds the F-ALM-09 report from the alarms raised in the window (pure; CR-012 §5).
pub fn report(
    from_tai: i64,
    to_tai: i64,
    history: Vec<AlarmView>,
    notes: &HashMap<String, Vec<NotificationRecord>>,
    window_s: u32,
) -> AlarmReport {
    let mut groups: Vec<((String, String), Vec<&AlarmView>)> = Vec::new();
    for a in &history {
        let key = (a.rule_id.clone(), a.priority.clone());
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(a),
            None => groups.push((key, vec![a])),
        }
    }
    let rules = groups
        .into_iter()
        .map(|((rule_id, priority), alarms)| {
            let acks: Vec<f64> = alarms
                .iter()
                .filter_map(|a| {
                    a.acknowledged_at_tai
                        .map(|t| (t - a.created_at_tai) as f64 / NS as f64)
                })
                .collect();
            let acked_early = alarms
                .iter()
                .filter(|a| {
                    let Some(ack) = a.acknowledged_at_tai else {
                        return false;
                    };
                    let beyond = notes.get(&a.id).and_then(|n| {
                        n.iter()
                            .filter(|s| !matches!(s.audience.as_str(), "astronaut" | "cmo"))
                            .map(|s| s.at_tai)
                            .min()
                    });
                    beyond.is_none_or(|t| ack < t)
                })
                .count();
            let silenced = alarms
                .iter()
                .filter(|a| a.silenced_until_tai.is_some())
                .count();
            let self_resolved = alarms
                .iter()
                .filter(|a| {
                    a.acknowledged_at_tai.is_none()
                        && a.silenced_until_tai.is_none()
                        && a.resolved_at_tai
                            .is_some_and(|r| r - a.created_at_tai <= SELF_RESOLVED_S * NS)
                })
                .count();
            let mut by_crew: HashMap<Option<&String>, Vec<i64>> = HashMap::new();
            for a in &alarms {
                by_crew
                    .entry(a.crew_member_id.as_ref())
                    .or_default()
                    .push(a.created_at_tai);
            }
            let repeat_bursts = by_crew
                .values()
                .map(|times| {
                    let mut bursts = 0;
                    let mut i = 0;
                    while i + 2 < times.len() {
                        if times[i + 2] - times[i] <= 3600 * NS {
                            bursts += 1;
                            i += 3;
                        } else {
                            i += 1;
                        }
                    }
                    bursts
                })
                .sum::<u32>();
            let n = |c: usize| u32::try_from(c).unwrap_or(u32::MAX);
            RuleStats {
                rule_id,
                priority,
                count: n(alarms.len()),
                median_ack_s: median(acks),
                acked_early: n(acked_early),
                silenced: n(silenced),
                self_resolved: n(self_resolved),
                repeat_bursts,
            }
        })
        .collect();
    let days = f64::from(window_s) / 86_400.0;
    let mut per_crew: Vec<(String, f64)> = Vec::new();
    for a in &history {
        if let Some(c) = &a.crew_member_id {
            match per_crew.iter_mut().find(|(k, _)| k == c) {
                Some((_, n)) => *n += 1.0,
                None => per_crew.push((c.clone(), 1.0)),
            }
        }
    }
    for (_, n) in &mut per_crew {
        *n /= days;
    }
    AlarmReport {
        from_tai,
        to_tai,
        history,
        rules,
        per_crew_per_day: per_crew,
    }
}
