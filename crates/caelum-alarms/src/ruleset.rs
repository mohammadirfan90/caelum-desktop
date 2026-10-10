//! The ruleset: parsing and validation of `rules/ruleset.toml` (docs/06 §3, SR-001, SR-003,
//! SR-004, SR-006, SR-018, SR-019).
//!
//! The file is parsed strictly (unknown fields refused) and then validated as a whole; any error
//! rejects the entire ruleset, never a single rule (SR-009). Thresholds are DRAFT clinical content
//! (⚕, CR-003); this module only enforces that they are well-formed.

use std::collections::BTreeSet;

use caelum_domain::parameters::{Category, Parameter, ValueType};
use serde::Deserialize;

/// CAELUM alarm priority (docs/06 §2) for rule-driven alarms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// Low: non-latching notice.
    Observe,
    /// Medium: latched until acknowledged.
    Warning,
    /// High: latched until acknowledged; escalates.
    Critical,
}

impl Priority {
    /// Critical and warning alarms stay until acknowledged (docs/06 §2).
    pub const fn latching(self) -> bool {
        matches!(self, Self::Warning | Self::Critical)
    }

    /// Stored name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

/// Alarm category (docs/05 §2 `alarm.category`, physiological and environment rules).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AlarmCategory {
    /// Heart.
    Cardio,
    /// Breathing and oxygenation.
    Respiratory,
    /// Temperature.
    Thermal,
    /// Cabin environment.
    Environment,
}

impl AlarmCategory {
    /// Stored name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cardio => "CARDIO",
            Self::Respiratory => "RESPIRATORY",
            Self::Thermal => "THERMAL",
            Self::Environment => "ENVIRONMENT",
        }
    }
}

/// Direction of a threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Op {
    /// Breach when the value is below `fire`; clear at or above `clear`.
    #[serde(rename = "<")]
    Below,
    /// Breach when the value is above `fire`; clear at or below `clear`.
    #[serde(rename = ">")]
    Above,
}

impl Op {
    /// Whether `v` breaches `fire`.
    pub fn breached(self, v: f64, fire: f64) -> bool {
        match self {
            Self::Below => v < fire,
            Self::Above => v > fire,
        }
    }

    /// Whether `v` meets the clear threshold.
    pub fn cleared(self, v: f64, clear: f64) -> bool {
        match self {
            Self::Below => v >= clear,
            Self::Above => v <= clear,
        }
    }
}

/// Fire and clear thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Breach threshold.
    pub fire: f64,
    /// Clear threshold (hysteresis).
    pub clear: f64,
}

/// ⚕ Thresholds per thermometer site class (CR-003 §3).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteLimits {
    /// Core-equivalent sites.
    pub core_equivalent: Limits,
    /// Peripheral sites.
    pub peripheral: Limits,
    /// Site not reported: must be the most conservative (SR-018).
    pub unknown: Limits,
}

/// Spot-reading confirmation (SR-019).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotConfirm {
    /// A second breaching reading within this window confirms.
    pub confirm_window_s: u32,
    /// No repeat reading within this time also raises the full alarm.
    pub recheck_timeout_s: u32,
}

/// Context condition: the rule applies at rest (SR-018: unknown context is still evaluated).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtRestDef {
    /// Context parameter (catalog key), e.g. `activity_level`.
    pub parameter: String,
    /// At rest when the context value is at most this.
    pub max: f64,
}

/// Gating signal for absence-of-event rules.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateDef {
    /// Signal-quality parameter (catalog key), e.g. `sqi_ecg`.
    pub parameter: String,
    /// The gate is open (signal valid) when the value is at least this.
    pub min: f64,
}

/// One rule as written in the file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDef {
    /// Unique rule ID.
    pub id: String,
    /// `threshold` or `absence`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Catalog key of the evaluated parameter (threshold rules).
    pub parameter: Option<String>,
    /// Category.
    pub category: AlarmCategory,
    /// Priority.
    pub priority: Priority,
    /// Short title for crew.
    pub title: String,
    /// What to do.
    pub action: String,
    /// Direction (threshold).
    pub op: Option<Op>,
    /// Fire threshold (threshold, fixed limits).
    pub fire: Option<f64>,
    /// Clear threshold (threshold, fixed limits).
    pub clear: Option<f64>,
    /// Persistence before firing, s (continuous).
    pub fire_for_s: u32,
    /// Clear window, s.
    pub clear_for_s: u32,
    /// Data gap that restarts persistence timing, s (SR-004).
    pub max_gap_s: u32,
    /// Supersession group (SR-006); defaults to the parameter.
    pub group: Option<String>,
    /// At-rest condition.
    pub at_rest: Option<AtRestDef>,
    /// Spot confirmation; required when the parameter can come from spot devices.
    pub spot: Option<SpotConfirm>,
    /// Per-site thresholds (body_temp_measured only).
    pub sites: Option<SiteLimits>,
    /// Absence: the expected event's name (e.g. `qrs`).
    pub event: Option<String>,
    /// Absence: the gate.
    pub gate: Option<GateDef>,
}

/// Baseline notices (F-ALM-08): observe only.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineDef {
    /// Robust z at or above which the notice fires.
    pub z_fire: f64,
    /// Robust z at or below which it clears.
    pub z_clear: f64,
    /// Persistence and clear window, s.
    pub for_s: u32,
    /// Spot readings: deviation in `k` of the last `n`.
    pub spot_k: u32,
    /// Spot readings: window length.
    pub spot_n: u32,
    /// Gap that restarts persistence, s.
    pub max_gap_s: u32,
    /// Parameters with baselines.
    pub parameters: Vec<BaselineParamDef>,
}

/// One baseline parameter.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineParamDef {
    /// Catalog key.
    pub parameter: String,
    /// Evaluated at rest only (unknown activity: evaluated, SR-018).
    pub at_rest: bool,
}

/// Ruleset header.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    /// `RULESET_VERSION` (CLAUDE.md rule 4).
    pub version: String,
    /// `DRAFT` until countersigned (CR-003).
    pub status: String,
    /// Who approved the content.
    pub approved_by: String,
}

/// The file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesetFile {
    /// Header.
    pub ruleset: Header,
    /// Rules.
    #[serde(rename = "rule")]
    pub rules: Vec<RuleDef>,
    /// Baseline notices.
    pub baseline: BaselineDef,
}

/// Validated threshold limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThresholdLimits {
    /// The same limits for every reading.
    Fixed(Limits),
    /// By thermometer site class.
    BySite(SiteLimits),
}

/// A validated threshold condition.
#[derive(Debug, Clone, PartialEq)]
pub struct Threshold {
    /// Direction.
    pub op: Op,
    /// Limits.
    pub limits: ThresholdLimits,
    /// Context condition.
    pub at_rest: Option<(Parameter, f64)>,
    /// Spot confirmation.
    pub spot: Option<SpotConfirm>,
}

/// A validated absence-of-event condition.
#[derive(Debug, Clone, PartialEq)]
pub struct Absence {
    /// Expected event.
    pub event: String,
    /// Gating parameter and minimum.
    pub gate: (Parameter, f64),
}

/// What a rule evaluates.
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// Threshold (absolute limit).
    Threshold(Threshold),
    /// Absence of an expected event.
    Absence(Absence),
}

/// A validated rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    /// ID.
    pub id: String,
    /// Evaluated parameter (for absence rules: the gate parameter).
    pub parameter: Parameter,
    /// Category.
    pub category: AlarmCategory,
    /// Priority.
    pub priority: Priority,
    /// Title.
    pub title: String,
    /// Action.
    pub action: String,
    /// Persistence, s.
    pub fire_for_s: u32,
    /// Clear window, s.
    pub clear_for_s: u32,
    /// Gap restart, s.
    pub max_gap_s: u32,
    /// Supersession group.
    pub group: String,
    /// Condition.
    pub condition: Condition,
}

/// A validated baseline parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BaselineParam {
    /// Parameter.
    pub parameter: Parameter,
    /// At rest only.
    pub at_rest: bool,
}

/// Validated baseline settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Baseline {
    /// Fire z.
    pub z_fire: f64,
    /// Clear z.
    pub z_clear: f64,
    /// Window, s.
    pub for_s: u32,
    /// k of n (spot).
    pub spot_k: u32,
    /// n (spot).
    pub spot_n: u32,
    /// Gap restart, s.
    pub max_gap_s: u32,
    /// Parameters.
    pub parameters: Vec<BaselineParam>,
}

/// A validated ruleset.
#[derive(Debug, Clone, PartialEq)]
pub struct Ruleset {
    /// Version.
    pub version: String,
    /// Status (`DRAFT`).
    pub status: String,
    /// Approver.
    pub approved_by: String,
    /// Rules.
    pub rules: Vec<Rule>,
    /// Baseline notices.
    pub baseline: Baseline,
}

/// Why a ruleset was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RulesetError {
    /// The TOML is malformed or has unknown fields.
    #[error("ruleset does not parse: {0}")]
    Parse(String),
    /// One or more rules are invalid.
    #[error("ruleset is invalid: {}", .0.join("; "))]
    Invalid(Vec<String>),
}

fn parameter(key: &str, errors: &mut Vec<String>, rule: &str) -> Option<Parameter> {
    let p = Parameter::from_key(key);
    if p.is_none() {
        errors.push(format!("{rule}: unknown parameter {key:?}"));
    }
    p
}

fn numeric_range(p: Parameter) -> Option<(f64, f64)> {
    match p.spec().value {
        ValueType::Numeric { min, max } => Some((min, max)),
        ValueType::Enum(_) | ValueType::EnumPending(_) => None,
    }
}

fn check_limits(rule: &str, op: Op, l: Limits, p: Parameter, errors: &mut Vec<String>) {
    if !(l.fire.is_finite() && l.clear.is_finite()) {
        errors.push(format!("{rule}: thresholds must be finite"));
        return;
    }
    // Hysteresis: the clear threshold is on the safe side of the fire threshold.
    let ordered = match op {
        Op::Below => l.clear >= l.fire,
        Op::Above => l.clear <= l.fire,
    };
    if !ordered {
        errors.push(format!(
            "{rule}: clear threshold is on the wrong side of fire"
        ));
    }
    if let Some((min, max)) = numeric_range(p) {
        if !(min..=max).contains(&l.fire) || !(min..=max).contains(&l.clear) {
            errors.push(format!(
                "{rule}: thresholds outside the plausible range of {}",
                p.spec().key
            ));
        }
    }
}

fn validate_rule(def: &RuleDef, errors: &mut Vec<String>) -> Option<Rule> {
    let id = def.id.as_str();
    let before = errors.len();
    if def.max_gap_s == 0 {
        errors.push(format!("{id}: max_gap_s must be positive"));
    }
    if def.title.trim().is_empty() || def.action.trim().is_empty() {
        errors.push(format!("{id}: title and action are required"));
    }
    let (param, condition) = match def.kind.as_str() {
        "threshold" => {
            let Some(p) = def
                .parameter
                .as_deref()
                .and_then(|k| parameter(k, errors, id))
            else {
                if def.parameter.is_none() {
                    errors.push(format!("{id}: threshold rules need a parameter"));
                }
                return None;
            };
            if numeric_range(p).is_none() {
                errors.push(format!("{id}: {} is not numeric", p.spec().key));
            }
            if p.spec().category == Category::Technical {
                errors.push(format!(
                    "{id}: technical parameters raise technical alarms, not rules"
                ));
            }
            let Some(op) = def.op else {
                errors.push(format!("{id}: threshold rules need op"));
                return None;
            };
            let limits = match (def.fire, def.clear, def.sites) {
                (Some(fire), Some(clear), None) => {
                    let l = Limits { fire, clear };
                    check_limits(id, op, l, p, errors);
                    ThresholdLimits::Fixed(l)
                }
                (None, None, Some(sites)) => {
                    if p != Parameter::BodyTempMeasured {
                        errors.push(format!(
                            "{id}: per-site limits are only for body_temp_measured"
                        ));
                    }
                    for l in [sites.core_equivalent, sites.peripheral, sites.unknown] {
                        check_limits(id, op, l, p, errors);
                    }
                    // SR-018: an unknown site uses the most conservative threshold.
                    let most_conservative = match op {
                        Op::Above => {
                            sites.unknown.fire <= sites.core_equivalent.fire
                                && sites.unknown.fire <= sites.peripheral.fire
                        }
                        Op::Below => {
                            sites.unknown.fire >= sites.core_equivalent.fire
                                && sites.unknown.fire >= sites.peripheral.fire
                        }
                    };
                    if !most_conservative {
                        errors.push(format!(
                            "{id}: the unknown-site threshold must be the most conservative"
                        ));
                    }
                    ThresholdLimits::BySite(sites)
                }
                _ => {
                    errors.push(format!("{id}: give either fire and clear, or sites"));
                    return None;
                }
            };
            if p == Parameter::BodyTempMeasured && def.sites.is_none() {
                errors.push(format!(
                    "{id}: body_temp_measured rules need per-site limits"
                ));
            }
            // Vital signs can arrive as spot readings (spot devices, manual entry): SR-019 needs
            // confirmation settings. Derived and environment values are continuous.
            if p.spec().category == Category::VitalSigns && def.spot.is_none() {
                errors.push(format!(
                    "{id}: {} can arrive as spot readings: spot confirmation is required (SR-019)",
                    p.spec().key
                ));
            }
            if let Some(s) = def.spot {
                if s.confirm_window_s == 0 || s.recheck_timeout_s == 0 {
                    errors.push(format!("{id}: spot windows must be positive"));
                }
            }
            let at_rest = match &def.at_rest {
                None => None,
                Some(a) => {
                    let cp = parameter(&a.parameter, errors, id);
                    if cp.is_some_and(|c| c != Parameter::ActivityLevel) {
                        errors.push(format!("{id}: at_rest uses activity_level"));
                    }
                    cp.map(|c| (c, a.max))
                }
            };
            if def.event.is_some() || def.gate.is_some() {
                errors.push(format!("{id}: event and gate are only for absence rules"));
            }
            (
                p,
                Condition::Threshold(Threshold {
                    op,
                    limits,
                    at_rest,
                    spot: def.spot,
                }),
            )
        }
        "absence" => {
            let (Some(event), Some(gate)) = (&def.event, &def.gate) else {
                errors.push(format!("{id}: absence rules need event and gate"));
                return None;
            };
            let gp = parameter(&gate.parameter, errors, id)?;
            if def.parameter.is_some()
                || def.op.is_some()
                || def.fire.is_some()
                || def.sites.is_some()
                || def.spot.is_some()
                || def.at_rest.is_some()
            {
                errors.push(format!(
                    "{id}: threshold fields are not allowed on absence rules"
                ));
            }
            if def.fire_for_s == 0 {
                errors.push(format!("{id}: the absence window must be positive"));
            }
            (
                gp,
                Condition::Absence(Absence {
                    event: event.clone(),
                    gate: (gp, gate.min),
                }),
            )
        }
        other => {
            errors.push(format!("{id}: unknown rule type {other:?}"));
            return None;
        }
    };
    // Model-driven parameters stay at observe level until validated (docs/06 §3).
    if param == Parameter::AfProbability && def.priority != Priority::Observe {
        errors.push(format!(
            "{id}: af_probability rules are observe only until model M2 is VALIDATED (CR)"
        ));
    }
    if errors.len() != before {
        return None;
    }
    Some(Rule {
        id: def.id.clone(),
        parameter: param,
        category: def.category,
        priority: def.priority,
        title: def.title.clone(),
        action: def.action.clone(),
        fire_for_s: def.fire_for_s,
        clear_for_s: def.clear_for_s,
        max_gap_s: def.max_gap_s,
        group: def
            .group
            .clone()
            .unwrap_or_else(|| param.spec().key.to_owned()),
        condition,
    })
}

impl Ruleset {
    /// Parses and validates a ruleset file. Any problem rejects the whole ruleset.
    pub fn parse(text: &str) -> Result<Self, RulesetError> {
        let file: RulesetFile =
            toml::from_str(text).map_err(|e| RulesetError::Parse(e.to_string()))?;
        Self::validate(file)
    }

    /// Validates a parsed file.
    pub fn validate(file: RulesetFile) -> Result<Self, RulesetError> {
        let mut errors = Vec::new();
        if file.ruleset.version.trim().is_empty() {
            errors.push("ruleset: version is required".to_owned());
        }
        let mut ids = BTreeSet::new();
        for r in &file.rules {
            if !ids.insert(r.id.as_str()) {
                errors.push(format!("{}: duplicate rule id", r.id));
            }
        }
        let rules: Vec<Rule> = file
            .rules
            .iter()
            .filter_map(|r| validate_rule(r, &mut errors))
            .collect();
        let b = &file.baseline;
        if !(b.z_fire.is_finite()
            && b.z_clear.is_finite()
            && b.z_clear > 0.0
            && b.z_fire > b.z_clear)
        {
            errors.push("baseline: need 0 < z_clear < z_fire".to_owned());
        }
        if b.spot_k == 0 || b.spot_k > b.spot_n || b.for_s == 0 || b.max_gap_s == 0 {
            errors.push("baseline: need 0 < spot_k <= spot_n and positive windows".to_owned());
        }
        let parameters: Vec<BaselineParam> = b
            .parameters
            .iter()
            .filter_map(|p| {
                parameter(&p.parameter, &mut errors, "baseline").map(|parameter| BaselineParam {
                    parameter,
                    at_rest: p.at_rest,
                })
            })
            .collect();
        if !errors.is_empty() {
            return Err(RulesetError::Invalid(errors));
        }
        Ok(Self {
            version: file.ruleset.version,
            status: file.ruleset.status,
            approved_by: file.ruleset.approved_by,
            rules,
            baseline: Baseline {
                z_fire: b.z_fire,
                z_clear: b.z_clear,
                for_s: b.for_s,
                spot_k: b.spot_k,
                spot_n: b.spot_n,
                max_gap_s: b.max_gap_s,
                parameters,
            },
        })
    }

    /// Rules evaluating `parameter`.
    pub fn rules_for(&self, parameter: Parameter) -> impl Iterator<Item = &Rule> {
        self.rules.iter().filter(move |r| r.parameter == parameter)
    }
}
