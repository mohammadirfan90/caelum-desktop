//! Alarm service: loads the signed ruleset with the SR-009 safe fallback, evaluates every stored
//! observation synchronously in the ingest path (docs/03 §3 step 5), escalates (F-ALM-04) and
//! persists everything, so a restart resumes where it stood (SR-007).
//!
//! Loading: the bundled ruleset is verified for this build's trust policy. If it verifies, its
//! bytes are kept as the last verified ruleset. If not, a technical alarm is raised and the last
//! verified ruleset from the database is re-verified and used; with none, no physiological rules
//! run and the technical alarm says so ("safe state"). Open alarms from before a restart are
//! restored into the engine (no duplicates) and the escalator (stored stage, acknowledgement and
//! silence).
//!
//! Who may act on an alarm is decided by the caller (`caelum_security::role::alarm_authority`);
//! this service applies the action and returns what must be delivered.

use std::collections::{HashMap, HashSet};

use caelum_alarms::engine::{Alarm, AlarmState, BaselineStats, Engine, Event, Input, Restored};
use caelum_alarms::escalation::{EscalationError, Escalator, Notice, Tracked};
use caelum_alarms::ruleset::{Audience, Priority};
use caelum_alarms::signing::{verify, TrustPolicy, TrustedKey, VerifiedRuleset};
use caelum_domain::ids::{AlarmId, CrewMemberId, DeviceId};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_ingest::{Observation, Value};
use caelum_store::{AlarmRecord, Database, OpenAlarmRecord, StoreError};
use serde_json::json;

/// `config_bundle.kind` of alarm rulesets.
pub const RULESET_BUNDLE_KIND: &str = "alarm-ruleset";

/// Which ruleset is in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RulesetStatus {
    /// The bundled ruleset verified.
    Verified {
        /// Version.
        version: String,
        /// Development-signed: show the DEVELOPMENT RULESET banner.
        development: bool,
    },
    /// The bundled ruleset failed; the last verified one is in force (technical alarm raised).
    Fallback {
        /// Version in force.
        version: String,
        /// Development-signed.
        development: bool,
        /// Why the bundled one failed.
        reason: String,
    },
    /// No verified ruleset: no physiological rules run (technical alarm raised).
    SafeState {
        /// Why.
        reason: String,
    },
}

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum AlarmServiceError {
    /// Storage failure.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Why could not be serialised.
    #[error("alarm why: {0}")]
    Why(String),
    /// The action was refused.
    #[error(transparent)]
    Escalation(#[from] EscalationError),
    /// No such open alarm.
    #[error("no such open alarm")]
    UnknownAlarm,
}

/// What one call produced: engine events (alarm lifecycle) and notices to deliver (stages, sound,
/// OPS records).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Output {
    /// Alarm lifecycle events.
    pub events: Vec<Event>,
    /// Deliveries.
    pub notices: Vec<Notice>,
}

impl Output {
    /// Nothing happened.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.notices.is_empty()
    }
}

/// The alarm service.
pub struct AlarmService {
    engine: Option<Engine>,
    escalator: Option<Escalator>,
    status: RulesetStatus,
    version: String,
    persisted: HashSet<AlarmId>,
    crew_of: HashMap<AlarmId, CrewMemberId>,
    technical: HashMap<String, String>,
    restored: Vec<Notice>,
}

fn why_json(alarm: &Alarm) -> Result<String, AlarmServiceError> {
    serde_json::to_string(&alarm.why).map_err(|e| AlarmServiceError::Why(e.to_string()))
}

fn priority(name: &str) -> Option<Priority> {
    match name {
        "critical" => Some(Priority::Critical),
        "warning" => Some(Priority::Warning),
        "observe" => Some(Priority::Observe),
        _ => None,
    }
}

fn state(name: &str) -> Option<AlarmState> {
    match name {
        "confirming" => Some(AlarmState::Confirming),
        "active" => Some(AlarmState::Active),
        "cleared_awaiting_ack" => Some(AlarmState::ClearedAwaitingAck),
        _ => None,
    }
}

fn state_name(s: AlarmState) -> &'static str {
    match s {
        AlarmState::Confirming => "confirming",
        AlarmState::Active => "active",
        AlarmState::ClearedAwaitingAck => "cleared_awaiting_ack",
    }
}

fn audience_name(a: Audience) -> &'static str {
    match a {
        Audience::Astronaut => "astronaut",
        Audience::Cmo => "cmo",
        Audience::Sound => "sound",
        Audience::Crew => "crew",
        Audience::Ground => "ground",
        Audience::Reminder => "reminder",
    }
}

impl AlarmService {
    /// Loads the ruleset (`ruleset`, `signatures`) under `policy`, falling back per SR-009, and
    /// restores the alarms that were open.
    pub fn load(
        db: &Database,
        ruleset: &str,
        signatures: &str,
        keys: &[TrustedKey],
        policy: TrustPolicy,
        now: TaiNs,
    ) -> Result<Self, AlarmServiceError> {
        let mut service = Self {
            engine: None,
            escalator: None,
            status: RulesetStatus::SafeState {
                reason: String::new(),
            },
            version: String::new(),
            persisted: HashSet::new(),
            crew_of: HashMap::new(),
            technical: HashMap::new(),
            restored: Vec::new(),
        };
        let open = db.open_alarm_records()?;
        for r in open.iter().filter(|r| r.kind == "technical") {
            service.technical.insert(
                format!(
                    "{}/{}",
                    r.rule_id,
                    r.crew_member_id.clone().unwrap_or_default()
                ),
                r.id.clone(),
            );
        }
        match verify(ruleset.as_bytes(), signatures, keys, policy) {
            Ok(v) => {
                db.put_config_bundle(
                    &v.sha256,
                    RULESET_BUNDLE_KIND,
                    &v.ruleset.version,
                    signatures,
                    ruleset,
                    now.0,
                )?;
                service.status = RulesetStatus::Verified {
                    version: v.ruleset.version.clone(),
                    development: v.development,
                };
                service.start(v, &open);
            }
            Err(e) => {
                let reason = e.to_string();
                let fallback =
                    db.latest_config_bundle(RULESET_BUNDLE_KIND)?
                        .and_then(|(_, sig, content)| {
                            verify(content.as_bytes(), &sig, keys, policy).ok()
                        });
                let note = match &fallback {
                    Some(v) => format!("using the last verified ruleset {}", v.ruleset.version),
                    None => "no verified ruleset: physiological rules are not running".to_owned(),
                };
                service.raise_technical(
                    db,
                    "technical.ruleset_invalid",
                    None,
                    json!({ "reason": reason, "effect": note }),
                    now,
                )?;
                match fallback {
                    Some(v) => {
                        service.status = RulesetStatus::Fallback {
                            version: v.ruleset.version.clone(),
                            development: v.development,
                            reason,
                        };
                        service.start(v, &open);
                    }
                    None => service.status = RulesetStatus::SafeState { reason },
                }
            }
        }
        Ok(service)
    }

    fn start(&mut self, v: VerifiedRuleset, open: &[OpenAlarmRecord]) {
        self.version = v.ruleset.version.clone();
        let mut escalator = Escalator::new(v.ruleset.escalation.clone());
        let mut engine = Engine::new(v.ruleset);
        for r in open.iter().filter(|r| r.kind == "physiological") {
            let (Ok(id), Some(crew), Some(st), Some(p)) = (
                r.id.parse::<AlarmId>(),
                r.crew_member_id
                    .as_deref()
                    .and_then(|c| c.parse::<CrewMemberId>().ok()),
                state(&r.state),
                priority(&r.priority),
            ) else {
                continue;
            };
            let acknowledged = r.acknowledged_by.is_some();
            engine.restore(&Restored {
                id,
                crew,
                rule_id: r.rule_id.clone(),
                state: st,
                acknowledged,
                created_at: r.created_at_tai,
            });
            self.restored.extend(escalator.restore(
                id,
                Tracked {
                    crew,
                    priority: p,
                    category: category_name(&r.category),
                    state: state_name(st),
                    created_at: r.created_at_tai,
                    next_stage: usize::try_from(r.escalation_stage).unwrap_or(0),
                    acknowledged,
                    silenced_until: r.silenced_until_tai,
                },
            ));
            self.persisted.insert(id);
            self.crew_of.insert(id, crew);
        }
        self.engine = Some(engine);
        self.escalator = Some(escalator);
    }

    /// The ruleset in force.
    pub fn status(&self) -> &RulesetStatus {
        &self.status
    }

    /// The crew member of an open alarm (for authority checks).
    pub fn crew_of(&self, id: AlarmId) -> Option<CrewMemberId> {
        self.crew_of.get(&id).copied()
    }

    /// Sounds to resume after a restart (delivered once).
    pub fn take_restored(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.restored)
    }

    fn raise_technical(
        &mut self,
        db: &Database,
        rule_id: &str,
        crew: Option<CrewMemberId>,
        why: serde_json::Value,
        now: TaiNs,
    ) -> Result<(), AlarmServiceError> {
        let key = format!(
            "{rule_id}/{}",
            crew.map(|c| c.to_string()).unwrap_or_default()
        );
        if self.technical.contains_key(&key) {
            return Ok(());
        }
        let id = uuid::Uuid::new_v4().hyphenated().to_string();
        db.insert_alarm(&AlarmRecord {
            id: id.clone(),
            crew_member_id: crew.map(|c| c.to_string()),
            kind: "technical".to_owned(),
            category: "TECHNICAL".to_owned(),
            rule_id: rule_id.to_owned(),
            ruleset_version: self.version.clone(),
            priority: "advisory".to_owned(),
            state: "active".to_owned(),
            confirming_readings: None,
            latched: false,
            trigger_value: None,
            why: why.to_string(),
            action: None,
            created_at_tai: now.0,
        })?;
        self.technical.insert(key, id);
        Ok(())
    }

    fn clear_technical(
        &mut self,
        db: &Database,
        rule_id: &str,
        crew: Option<CrewMemberId>,
        now: TaiNs,
    ) -> Result<(), AlarmServiceError> {
        let key = format!(
            "{rule_id}/{}",
            crew.map(|c| c.to_string()).unwrap_or_default()
        );
        if let Some(id) = self.technical.remove(&key) {
            db.update_alarm(&id, "resolved", None, Some(now.0))?;
        }
        Ok(())
    }

    /// Raises (once) or clears a station technical alarm (F-ALM-06); `why` explains the cause.
    pub fn technical(
        &mut self,
        db: &Database,
        rule_id: &str,
        crew: Option<CrewMemberId>,
        present: Option<serde_json::Value>,
        now: TaiNs,
    ) -> Result<(), AlarmServiceError> {
        match present {
            Some(why) => self.raise_technical(db, rule_id, crew, why, now),
            None => self.clear_technical(db, rule_id, crew, now),
        }
    }

    /// Whether a technical alarm is open.
    pub fn technical_open(&self, rule_id: &str, crew: Option<CrewMemberId>) -> bool {
        self.technical.contains_key(&format!(
            "{rule_id}/{}",
            crew.map(|c| c.to_string()).unwrap_or_default()
        ))
    }

    /// Evaluates one stored observation (numeric values only) and persists the results.
    pub fn observe(
        &mut self,
        db: &Database,
        obs: &Observation,
        baseline: Option<BaselineStats>,
    ) -> Result<Output, AlarmServiceError> {
        let Value::Number(value) = obs.value else {
            return Ok(Output::default());
        };
        let Some(engine) = self.engine.as_mut() else {
            return Ok(Output::default());
        };
        let input = Input {
            crew: obs.crew_member_id,
            device: Some(obs.device_id),
            parameter: obs.parameter,
            value,
            at: obs.measured_at,
            quality: obs.quality,
            mode: obs.mode,
            site: obs.provenance.site.map(|s| s.class()),
        };
        let events = engine.evaluate(&input, baseline);
        self.persist(db, events, obs.measured_at)
    }

    /// The 4 Hz tick: absence rules, spot recheck timeouts, escalation stages, silence expiry.
    pub fn tick(&mut self, db: &Database, now: TaiNs) -> Result<Output, AlarmServiceError> {
        let Some(engine) = self.engine.as_mut() else {
            return Ok(Output::default());
        };
        let events = engine.tick(now);
        let mut out = self.persist(db, events, now)?;
        if let Some(esc) = self.escalator.as_mut() {
            let notices = esc.tick(now);
            self.record(db, &notices, now)?;
            out.notices.extend(notices);
        }
        Ok(out)
    }

    /// An expected event (e.g. QRS) for absence rules.
    pub fn event(
        &mut self,
        db: &Database,
        crew: CrewMemberId,
        name: &str,
        at: TaiNs,
    ) -> Result<Output, AlarmServiceError> {
        let Some(engine) = self.engine.as_mut() else {
            return Ok(Output::default());
        };
        let events = engine.event(crew, name, at);
        self.persist(db, events, at)
    }

    /// Onboard acknowledgement by `user` (F-ALM-05): stops escalation and sound; a latched alarm
    /// whose condition cleared ends.
    pub fn acknowledge(
        &mut self,
        db: &Database,
        id: AlarmId,
        user: &str,
        now: TaiNs,
    ) -> Result<Output, AlarmServiceError> {
        let (Some(engine), Some(esc)) = (self.engine.as_mut(), self.escalator.as_mut()) else {
            return Err(AlarmServiceError::UnknownAlarm);
        };
        let notices = esc.acknowledge(id)?;
        db.set_alarm_acknowledged(&id.to_string(), user, now.0)?;
        let events = engine.acknowledge(id);
        let mut out = self.persist(db, events, now)?;
        out.notices.splice(0..0, notices);
        Ok(out)
    }

    /// Ground-stage acknowledgement by the flight surgeon (ROLE_MATRIX §5.4): recorded only; the
    /// alarm keeps escalating and sounding onboard.
    pub fn acknowledge_ground(
        &mut self,
        db: &Database,
        id: AlarmId,
        user: &str,
        now: TaiNs,
    ) -> Result<(), AlarmServiceError> {
        if !self.persisted.contains(&id) {
            return Err(AlarmServiceError::UnknownAlarm);
        }
        db.set_alarm_ground_acknowledged(&id.to_string(), user, now.0)?;
        Ok(())
    }

    /// Silences the sound for up to the SR-008 limit; returns the notices and the applied seconds.
    pub fn silence(
        &mut self,
        db: &Database,
        id: AlarmId,
        requested_s: u32,
        now: TaiNs,
    ) -> Result<(Output, u32), AlarmServiceError> {
        let esc = self
            .escalator
            .as_mut()
            .ok_or(AlarmServiceError::UnknownAlarm)?;
        let (notices, applied) = esc.silence(id, requested_s, now)?;
        db.set_alarm_silenced(&id.to_string(), now.0 + i64::from(applied) * 1_000_000_000)?;
        Ok((
            Output {
                events: Vec::new(),
                notices,
            },
            applied,
        ))
    }

    /// Manual escalation (F-ALM-05): the next stage now.
    pub fn escalate(
        &mut self,
        db: &Database,
        id: AlarmId,
        now: TaiNs,
    ) -> Result<Output, AlarmServiceError> {
        let esc = self
            .escalator
            .as_mut()
            .ok_or(AlarmServiceError::UnknownAlarm)?;
        let notices = esc.escalate_now(id, now)?;
        self.record(db, &notices, now)?;
        Ok(Output {
            events: Vec::new(),
            notices,
        })
    }

    /// Records delivered stages (stage index for restart, notification log).
    fn record(
        &self,
        db: &Database,
        notices: &[Notice],
        now: TaiNs,
    ) -> Result<(), AlarmServiceError> {
        for n in notices {
            if let Notice::Stage {
                alarm,
                stage,
                audience,
                next_stage,
            } = n
            {
                let id = alarm.to_string();
                db.insert_alarm_notification(
                    &id,
                    audience_name(*audience),
                    i64::try_from(*stage).unwrap_or(i64::MAX),
                    now.0,
                )?;
                db.set_alarm_escalation_stage(&id, i64::try_from(*next_stage).unwrap_or(i64::MAX))?;
            }
        }
        Ok(())
    }

    fn insert(&mut self, db: &Database, a: &Alarm, state: &str) -> Result<(), AlarmServiceError> {
        db.insert_alarm(&AlarmRecord {
            id: a.id.to_string(),
            crew_member_id: Some(a.crew.to_string()),
            kind: "physiological".to_owned(),
            category: a.category.to_owned(),
            rule_id: a.rule_id.clone(),
            ruleset_version: a.why.ruleset_version.clone(),
            priority: a.priority.to_owned(),
            state: state.to_owned(),
            confirming_readings: None,
            latched: a.latched,
            trigger_value: a.trigger_value,
            why: why_json(a)?,
            action: Some(a.action.clone()),
            created_at_tai: a.created_at,
        })?;
        self.persisted.insert(a.id);
        self.crew_of.insert(a.id, a.crew);
        Ok(())
    }

    fn track(&mut self, a: &Alarm, now: TaiNs) -> Vec<Notice> {
        let (Some(esc), Some(p)) = (self.escalator.as_mut(), priority(a.priority)) else {
            return Vec::new();
        };
        esc.track(
            a.id,
            Tracked {
                crew: a.crew,
                priority: p,
                category: a.category,
                state: state_name(a.state),
                created_at: a.created_at,
                next_stage: 0,
                acknowledged: false,
                silenced_until: None,
            },
            now,
        )
    }

    fn ended(&mut self, id: AlarmId) -> Vec<Notice> {
        self.persisted.remove(&id);
        self.crew_of.remove(&id);
        self.escalator
            .as_mut()
            .map(|e| e.ended(id))
            .unwrap_or_default()
    }

    fn persist(
        &mut self,
        db: &Database,
        events: Vec<Event>,
        at: TaiNs,
    ) -> Result<Output, AlarmServiceError> {
        let mut notices = Vec::new();
        for ev in &events {
            match ev {
                Event::Confirming(a) => {
                    self.insert(db, a, "confirming")?;
                    notices.extend(self.track(a, at));
                }
                Event::Raised(a) | Event::Reactivated(a) => {
                    if self.persisted.contains(&a.id) {
                        db.update_alarm(&a.id.to_string(), "active", Some(&why_json(a)?), None)?;
                        if let Some(esc) = self.escalator.as_mut() {
                            esc.set_state(a.id, "active");
                        }
                    } else {
                        self.insert(db, a, "active")?;
                        notices.extend(self.track(a, at));
                    }
                }
                Event::ClearedAwaitingAck(a) => {
                    db.update_alarm(
                        &a.id.to_string(),
                        "cleared_awaiting_ack",
                        Some(&why_json(a)?),
                        None,
                    )?;
                    if let Some(esc) = self.escalator.as_mut() {
                        esc.set_state(a.id, "cleared_awaiting_ack");
                    }
                }
                Event::NotConfirmed(a) => {
                    db.update_alarm(
                        &a.id.to_string(),
                        "not_confirmed",
                        Some(&why_json(a)?),
                        Some(at.0),
                    )?;
                    notices.extend(self.ended(a.id));
                }
                Event::Resolved { alarm, .. } => {
                    db.update_alarm(&alarm.to_string(), "resolved", None, Some(at.0))?;
                    notices.extend(self.ended(*alarm));
                }
                Event::Superseded { alarm, .. } => {
                    db.update_alarm(&alarm.to_string(), "superseded", None, Some(at.0))?;
                    notices.extend(self.ended(*alarm));
                }
                Event::SignalUnusable {
                    crew,
                    device,
                    parameter,
                    quality,
                } => {
                    self.raise_technical(
                        db,
                        &signal_rule(*parameter, *device),
                        Some(*crew),
                        json!({ "parameter": parameter.spec().key, "quality": format!("{quality:?}").to_lowercase(), "effect": "rules for this signal are paused (SR-005)" }),
                        at,
                    )?;
                }
                Event::SignalRecovered {
                    crew,
                    device,
                    parameter,
                } => {
                    self.clear_technical(db, &signal_rule(*parameter, *device), Some(*crew), at)?;
                }
            }
        }
        self.record(db, &notices, at)?;
        Ok(Output { events, notices })
    }
}

fn category_name(c: &str) -> &'static str {
    match c {
        "CARDIO" => "CARDIO",
        "RESPIRATORY" => "RESPIRATORY",
        "THERMAL" => "THERMAL",
        "ENVIRONMENT" => "ENVIRONMENT",
        _ => "TECHNICAL",
    }
}

fn signal_rule(parameter: Parameter, device: Option<DeviceId>) -> String {
    format!(
        "technical.signal_unusable.{}.{}",
        parameter.spec().key,
        device.map(|d| d.to_string()).unwrap_or_default()
    )
}
