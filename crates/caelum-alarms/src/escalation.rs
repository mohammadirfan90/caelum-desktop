//! Escalation, acknowledgement and silencing (F-ALM-04, F-ALM-05; SR-007, SR-008, SR-015; CR-010).
//!
//! Sans-I/O, like the engine: alarms are tracked from creation, `tick` delivers every stage that
//! falls due, and the caller persists each delivered stage (`stage` is the next stage index), so a
//! restart resumes exactly where escalation stood: stages that fell due while CAELUM was down are
//! delivered at once on the first tick (SR-007).
//!
//! - Acknowledgement stops escalation and the sound; the visual alarm stays until it ends.
//! - Silencing stops the sound for at most 2 min (critical) or 10 min (warning) (SR-008), never
//!   the visual alarm and never the escalation timers; when it expires the alarm sounds again.
//! - Sound: the stations notified so far sound the priority's pattern; from the `sound` stage every
//!   crew station sounds and the wristband alert is sent in addition. Crew stations always sound:
//!   no wristband, or a failed wristband alert, changes nothing for them (SR-015).
//! - The ground stage emits [`OpsEscalation`], which can carry only the docs/06 §3a fields.

use std::collections::BTreeMap;

use caelum_domain::ids::{AlarmId, CrewMemberId};
use caelum_domain::time::TaiNs;
use serde::Serialize;

use crate::ruleset::{Audience, Escalation, Priority};

const NS_PER_S: i64 = 1_000_000_000;

/// SR-008: longest silence for a critical (high-priority) alarm, s.
pub const MAX_SILENCE_CRITICAL_S: u32 = 120;
/// SR-008: longest silence for a warning (medium-priority) alarm, s.
pub const MAX_SILENCE_WARNING_S: u32 = 600;

/// The sound pattern (docs/06 §2; IEC 60601-1-8 priorities).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundPattern {
    /// High-priority pattern, repeating.
    High,
    /// Medium-priority pattern.
    Medium,
    /// Single low tone.
    Low,
}

/// Where an alarm sounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundScope {
    /// The stations of the people notified so far (astronaut, CMO).
    Notified,
    /// Every crew station.
    AllCrewStations,
}

/// The OPS-channel escalation record (docs/06 §3a): only these fields exist. No measured value,
/// parameter name, rule id or free text can be put in it (R-SEC-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OpsEscalation {
    /// Crew member.
    pub crew_id: CrewMemberId,
    /// Priority.
    pub priority: &'static str,
    /// Alarm category.
    pub category: &'static str,
    /// Alarm state.
    pub state: &'static str,
    /// Escalation stage index.
    pub stage: u8,
    /// Alarm creation (TAI ns).
    pub created_at_tai: i64,
    /// This escalation (TAI ns).
    pub at_tai: i64,
}

/// What the caller must do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// Deliver a stage to its audience; persist `next_stage` as the alarm's escalation stage.
    Stage {
        /// Alarm.
        alarm: AlarmId,
        /// Index of the delivered stage.
        stage: usize,
        /// Audience.
        audience: Audience,
        /// The stage index to persist (next one due).
        next_stage: usize,
    },
    /// Start (or widen) the alarm sound.
    SoundOn {
        /// Alarm.
        alarm: AlarmId,
        /// Pattern.
        pattern: SoundPattern,
        /// Where.
        scope: SoundScope,
        /// Send the wristband Immediate Alert in addition (never instead).
        wristband: bool,
    },
    /// Stop the alarm sound (acknowledged, silenced, or ended); the visual alarm is unaffected.
    SoundOff {
        /// Alarm.
        alarm: AlarmId,
        /// Why.
        reason: SoundOffReason,
    },
    /// Send on the OPS channel (path B).
    Ops(OpsEscalation),
}

/// Why the sound stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundOffReason {
    /// Acknowledged.
    Acknowledged,
    /// Silenced until this TAI time.
    SilencedUntil(i64),
    /// The alarm ended.
    Ended,
}

/// Refusals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EscalationError {
    /// Not a tracked alarm.
    #[error("no such open alarm")]
    UnknownAlarm,
    /// Observe-level alarms have no sound to silence.
    #[error("only critical and warning alarms can be silenced")]
    NotSilenceable,
    /// Zero-length silence.
    #[error("silence duration must be positive")]
    ZeroSilence,
}

/// An alarm as the escalator tracks it (also what a restart restores from the database).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracked {
    /// Crew member.
    pub crew: CrewMemberId,
    /// Priority.
    pub priority: Priority,
    /// Category name.
    pub category: &'static str,
    /// State name (for the OPS record).
    pub state: &'static str,
    /// Creation (TAI ns).
    pub created_at: i64,
    /// Next stage index due.
    pub next_stage: usize,
    /// Acknowledged.
    pub acknowledged: bool,
    /// Silenced until (TAI ns).
    pub silenced_until: Option<i64>,
}

/// The escalator.
pub struct Escalator {
    plans: Escalation,
    alarms: BTreeMap<AlarmId, Tracked>,
}

fn pattern(p: Priority) -> SoundPattern {
    match p {
        Priority::Critical => SoundPattern::High,
        Priority::Warning => SoundPattern::Medium,
        Priority::Observe => SoundPattern::Low,
    }
}

impl Escalator {
    /// An escalator for the plans of the verified ruleset.
    pub fn new(plans: Escalation) -> Self {
        Self {
            plans,
            alarms: BTreeMap::new(),
        }
    }

    /// Tracked alarms.
    pub fn tracked(&self, id: AlarmId) -> Option<&Tracked> {
        self.alarms.get(&id)
    }

    /// Starts tracking a new alarm (confirming or full) and delivers the stages due at `now`.
    pub fn track(&mut self, id: AlarmId, alarm: Tracked, now: TaiNs) -> Vec<Notice> {
        self.alarms.insert(id, alarm);
        self.due(id, now.0)
    }

    /// Restores an alarm after a restart (stage and flags from the database) and returns the sound
    /// it was making; then [`Self::tick`] delivers the stages that fell due meanwhile.
    pub fn restore(&mut self, id: AlarmId, alarm: Tracked) -> Vec<Notice> {
        let sound = Self::sound(&self.plans, id, &alarm);
        self.alarms.insert(id, alarm);
        sound.into_iter().collect()
    }

    /// Updates the state name reported to OPS.
    pub fn set_state(&mut self, id: AlarmId, state: &'static str) {
        if let Some(a) = self.alarms.get_mut(&id) {
            a.state = state;
        }
    }

    /// Delivers every due stage, ends expired silences and restores sound after a restart.
    pub fn tick(&mut self, now: TaiNs) -> Vec<Notice> {
        let ids: Vec<AlarmId> = self.alarms.keys().copied().collect();
        let mut out = Vec::new();
        for id in ids {
            if let Some(a) = self.alarms.get_mut(&id) {
                if a.silenced_until.is_some_and(|u| now.0 >= u) {
                    a.silenced_until = None;
                    if let Some(on) = Self::sound(&self.plans, id, a) {
                        out.push(on);
                    }
                }
            }
            out.extend(self.due(id, now.0));
        }
        out
    }

    /// The sound an alarm should make now, if any.
    fn sound(plans: &Escalation, id: AlarmId, a: &Tracked) -> Option<Notice> {
        if a.acknowledged || a.silenced_until.is_some() || a.next_stage == 0 {
            return None;
        }
        let reached = &plans.plan(a.priority)[..a.next_stage];
        let all = reached
            .iter()
            .any(|s| matches!(s.audience, Audience::Sound | Audience::Crew));
        Some(Notice::SoundOn {
            alarm: id,
            pattern: pattern(a.priority),
            scope: if all {
                SoundScope::AllCrewStations
            } else {
                SoundScope::Notified
            },
            wristband: reached.iter().any(|s| s.audience == Audience::Sound),
        })
    }

    fn deliver(&mut self, id: AlarmId, now: i64, out: &mut Vec<Notice>) -> bool {
        let Some(a) = self.alarms.get_mut(&id) else {
            return false;
        };
        let plan = self.plans.plan(a.priority);
        let Some(stage) = plan.get(a.next_stage).copied() else {
            return false;
        };
        let index = a.next_stage;
        a.next_stage += 1;
        out.push(Notice::Stage {
            alarm: id,
            stage: index,
            audience: stage.audience,
            next_stage: a.next_stage,
        });
        if stage.audience == Audience::Ground {
            out.push(Notice::Ops(OpsEscalation {
                crew_id: a.crew,
                priority: a.priority.as_str(),
                category: a.category,
                state: a.state,
                stage: u8::try_from(index).unwrap_or(u8::MAX),
                created_at_tai: a.created_at,
                at_tai: now,
            }));
        }
        true
    }

    /// Whether the next stage of an unacknowledged alarm is due at `now`.
    fn stage_due(&self, id: AlarmId, now: i64) -> bool {
        self.alarms.get(&id).is_some_and(|a| {
            !a.acknowledged
                && self
                    .plans
                    .plan(a.priority)
                    .get(a.next_stage)
                    .is_some_and(|stage| now >= a.created_at + i64::from(stage.after_s) * NS_PER_S)
        })
    }

    fn due(&mut self, id: AlarmId, now: i64) -> Vec<Notice> {
        let mut out = Vec::new();
        let before = self.alarms.get(&id).map(|a| a.next_stage);
        while self.stage_due(id, now) {
            self.deliver(id, now, &mut out);
        }
        // Sound follows the stages reached.
        if self.alarms.get(&id).map(|a| a.next_stage) != before {
            if let Some(on) = self
                .alarms
                .get(&id)
                .and_then(|a| Self::sound(&self.plans, id, a))
            {
                out.push(on);
            }
        }
        out
    }

    /// Manual escalation (F-ALM-05): delivers the next stage now.
    pub fn escalate_now(
        &mut self,
        id: AlarmId,
        now: TaiNs,
    ) -> Result<Vec<Notice>, EscalationError> {
        if !self.alarms.contains_key(&id) {
            return Err(EscalationError::UnknownAlarm);
        }
        let mut out = Vec::new();
        if self.deliver(id, now.0, &mut out) {
            if let Some(a) = self.alarms.get(&id) {
                out.extend(Self::sound(&self.plans, id, a));
            }
        }
        Ok(out)
    }

    /// Acknowledgement (F-ALM-05): escalation and sound stop; the alarm stays visible.
    pub fn acknowledge(&mut self, id: AlarmId) -> Result<Vec<Notice>, EscalationError> {
        let a = self
            .alarms
            .get_mut(&id)
            .ok_or(EscalationError::UnknownAlarm)?;
        a.acknowledged = true;
        a.silenced_until = None;
        Ok(vec![Notice::SoundOff {
            alarm: id,
            reason: SoundOffReason::Acknowledged,
        }])
    }

    /// Silences an alarm for `requested_s`, clamped to the SR-008 limit; returns the notices and the
    /// applied duration. Escalation continues.
    pub fn silence(
        &mut self,
        id: AlarmId,
        requested_s: u32,
        now: TaiNs,
    ) -> Result<(Vec<Notice>, u32), EscalationError> {
        if requested_s == 0 {
            return Err(EscalationError::ZeroSilence);
        }
        let a = self
            .alarms
            .get_mut(&id)
            .ok_or(EscalationError::UnknownAlarm)?;
        let max = match a.priority {
            Priority::Critical => MAX_SILENCE_CRITICAL_S,
            Priority::Warning => MAX_SILENCE_WARNING_S,
            Priority::Observe => return Err(EscalationError::NotSilenceable),
        };
        let applied = requested_s.min(max);
        let until = now.0 + i64::from(applied) * NS_PER_S;
        a.silenced_until = Some(until);
        Ok((
            vec![Notice::SoundOff {
                alarm: id,
                reason: SoundOffReason::SilencedUntil(until),
            }],
            applied,
        ))
    }

    /// The alarm ended (resolved, superseded, not confirmed): stop tracking and sound.
    pub fn ended(&mut self, id: AlarmId) -> Vec<Notice> {
        if self.alarms.remove(&id).is_some() {
            vec![Notice::SoundOff {
                alarm: id,
                reason: SoundOffReason::Ended,
            }]
        } else {
            Vec::new()
        }
    }
}
