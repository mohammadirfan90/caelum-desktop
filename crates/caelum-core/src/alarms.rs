//! Alarm service: loads the signed ruleset with the SR-009 safe fallback, evaluates every stored
//! observation synchronously in the ingest path (docs/03 §3 step 5) and persists alarm events.
//!
//! Loading: the bundled ruleset is verified for this build's trust policy. If it verifies, its
//! bytes are kept as the last verified ruleset. If not, a technical alarm is raised and the last
//! verified ruleset from the database is re-verified and used; with none, no physiological rules
//! run and the technical alarm says so ("safe state").

use std::collections::{HashMap, HashSet};

use caelum_alarms::engine::{Alarm, BaselineStats, Engine, Event, Input};
use caelum_alarms::signing::{verify, TrustPolicy, TrustedKey, VerifiedRuleset};
use caelum_domain::ids::{AlarmId, CrewMemberId, DeviceId};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_ingest::{Observation, Value};
use caelum_store::{AlarmRecord, Database, StoreError};
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

/// Errors persisting alarms.
#[derive(Debug, thiserror::Error)]
pub enum AlarmServiceError {
    /// Storage failure.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Why could not be serialised.
    #[error("alarm why: {0}")]
    Why(String),
}

/// The alarm service.
pub struct AlarmService {
    engine: Option<Engine>,
    status: RulesetStatus,
    version: String,
    persisted: HashSet<AlarmId>,
    technical: HashMap<String, String>,
}

fn why_json(alarm: &Alarm) -> Result<String, AlarmServiceError> {
    serde_json::to_string(&alarm.why).map_err(|e| AlarmServiceError::Why(e.to_string()))
}

impl AlarmService {
    /// Loads the ruleset (`ruleset`, `signatures`) under `policy`, falling back per SR-009.
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
            status: RulesetStatus::SafeState {
                reason: String::new(),
            },
            version: String::new(),
            persisted: HashSet::new(),
            technical: HashMap::new(),
        };
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
                service.start(v);
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
                        service.start(v);
                    }
                    None => service.status = RulesetStatus::SafeState { reason },
                }
            }
        }
        Ok(service)
    }

    fn start(&mut self, v: VerifiedRuleset) {
        self.version = v.ruleset.version.clone();
        self.engine = Some(Engine::new(v.ruleset));
    }

    /// The ruleset in force.
    pub fn status(&self) -> &RulesetStatus {
        &self.status
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

    /// Evaluates one stored observation (numeric values only) and persists the events.
    pub fn observe(
        &mut self,
        db: &Database,
        obs: &Observation,
        baseline: Option<BaselineStats>,
    ) -> Result<Vec<Event>, AlarmServiceError> {
        let Value::Number(value) = obs.value else {
            return Ok(Vec::new());
        };
        let Some(engine) = self.engine.as_mut() else {
            return Ok(Vec::new());
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
        self.persist(db, &events, obs.measured_at)?;
        Ok(events)
    }

    /// The 4 Hz tick (absence rules, spot recheck timeouts).
    pub fn tick(&mut self, db: &Database, now: TaiNs) -> Result<Vec<Event>, AlarmServiceError> {
        let Some(engine) = self.engine.as_mut() else {
            return Ok(Vec::new());
        };
        let events = engine.tick(now);
        self.persist(db, &events, now)?;
        Ok(events)
    }

    /// An expected event (e.g. QRS) for absence rules.
    pub fn event(
        &mut self,
        db: &Database,
        crew: CrewMemberId,
        name: &str,
        at: TaiNs,
    ) -> Result<Vec<Event>, AlarmServiceError> {
        let Some(engine) = self.engine.as_mut() else {
            return Ok(Vec::new());
        };
        let events = engine.event(crew, name, at);
        self.persist(db, &events, at)?;
        Ok(events)
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
        Ok(())
    }

    fn persist(
        &mut self,
        db: &Database,
        events: &[Event],
        at: TaiNs,
    ) -> Result<(), AlarmServiceError> {
        for ev in events {
            match ev {
                Event::Confirming(a) => self.insert(db, a, "confirming")?,
                Event::Raised(a) | Event::Reactivated(a) => {
                    if self.persisted.contains(&a.id) {
                        db.update_alarm(&a.id.to_string(), "active", Some(&why_json(a)?), None)?;
                    } else {
                        self.insert(db, a, "active")?;
                    }
                }
                Event::ClearedAwaitingAck(a) => {
                    db.update_alarm(
                        &a.id.to_string(),
                        "cleared_awaiting_ack",
                        Some(&why_json(a)?),
                        None,
                    )?;
                }
                Event::NotConfirmed(a) => {
                    db.update_alarm(
                        &a.id.to_string(),
                        "not_confirmed",
                        Some(&why_json(a)?),
                        Some(at.0),
                    )?;
                    self.persisted.remove(&a.id);
                }
                Event::Resolved { alarm, .. } => {
                    db.update_alarm(&alarm.to_string(), "resolved", None, Some(at.0))?;
                    self.persisted.remove(alarm);
                }
                Event::Superseded { alarm, .. } => {
                    db.update_alarm(&alarm.to_string(), "superseded", None, Some(at.0))?;
                    self.persisted.remove(alarm);
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
        Ok(())
    }
}

fn signal_rule(parameter: Parameter, device: Option<DeviceId>) -> String {
    format!(
        "technical.signal_unusable.{}.{}",
        parameter.spec().key,
        device.map(|d| d.to_string()).unwrap_or_default()
    )
}
