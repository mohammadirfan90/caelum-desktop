//! Model outputs as advisory information only (SR-002, F-ML-01, F-ML-04).
//!
//! The guarantee is structural:
//! - An [`AdvisoryResult`] and an [`AdvisoryNotice`] are display types. They have no conversion
//!   into an engine alarm, a priority above observe, an acknowledgement, a suppression or a
//!   timing input; [`AdvisoryLevel`] has a single variant, so a notice cannot be anything but an
//!   observe-level notice.
//! - The only path from a model to the rule engine is [`ObserveOnlyInput::apply`], which feeds a
//!   model-derived parameter (e.g. `af_probability` for rule `af.observe`) and refuses unless, in
//!   the engine's own ruleset, every rule on that parameter is an observe-level threshold rule
//!   whose supersession group holds no rule on any other parameter, the parameter gates no
//!   absence rule, is no rule's at-rest context and is not a baseline parameter. Such an input can
//!   therefore only open, keep or clear observe-level alarms on that parameter: it cannot raise,
//!   lower, delay or suppress an absolute-limit alarm.
//! - Model M2 (AF) is reported as `af_probability`, which rule `af.observe` evaluates at observe
//!   level; promoting it needs a CR, a ruleset change and a change here (docs/08 §1).

use std::collections::BTreeSet;

use caelum_alarms::engine::{Engine, Event, Input};
use caelum_alarms::ruleset::{Condition, Priority, Ruleset};
use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::observation::{MeasurementMode, Quality};
use caelum_domain::parameters::{Parameter, ValueType};
use caelum_domain::time::TaiNs;
use serde::Serialize;

use crate::explain::Contribution;
use crate::manifest::{NoticeOp, NoticeSpec, ValidationStatus};

/// Catalog parameters a model may report (model outputs by definition, docs/05 §1): M2's AF
/// probability and M6's BP trend. Every other parameter is measured, derived by a versioned
/// algorithm, or context for rules, and a model can never report it.
pub const MODEL_OUTPUT_PARAMETERS: &[Parameter] =
    &[Parameter::AfProbability, Parameter::BpTrendSystolic];

/// Display status of a model (docs/08 §1: validation status shown in the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DisplayStatus {
    /// Validated against its gate; outputs still advisory.
    Validated,
    /// Not validated (or research, development builds only): outputs labelled investigational.
    Investigational,
    /// Turned off by a user or refused at load.
    Disabled,
    /// No runtime (library absent, not the pinned binary, or the model would not load).
    Unavailable,
}

impl DisplayStatus {
    /// The display status of a loaded package of this validation status.
    pub const fn for_validation(status: ValidationStatus) -> Self {
        match status {
            ValidationStatus::Validated => Self::Validated,
            ValidationStatus::NotValidated | ValidationStatus::Research => Self::Investigational,
        }
    }
}

/// One model output with its explanation (every output carries one, F-ML-04).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdvisoryResult {
    /// Model ID.
    pub model_id: String,
    /// Model package version.
    pub model_version: String,
    /// Validation status from the signed manifest.
    pub validation: ValidationStatus,
    /// Display status.
    pub status: DisplayStatus,
    /// Package signed with development keys only.
    pub development_signed: bool,
    /// Crew member.
    pub crew: CrewMemberId,
    /// Output name.
    pub output: String,
    /// Output label.
    pub label: String,
    /// Value (finite and inside the manifest's range).
    pub value: f64,
    /// Evaluation time (end of the feature windows).
    pub at: TaiNs,
    /// Worst input quality (GOOD or DEGRADED).
    pub quality: Quality,
    /// Top contributing features, largest first.
    pub contributions: Vec<Contribution>,
    /// Model-derived parameter this output reports, if any.
    #[serde(skip)]
    pub parameter: Option<Parameter>,
    /// Notice condition from the manifest, if any.
    #[serde(skip)]
    pub notice: Option<NoticeSpec>,
}

/// The only level a model notice can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvisoryLevel {
    /// Observe-level notice: non-latching, never escalated beyond the observe plan (SR-002).
    Observe,
}

impl AdvisoryLevel {
    /// The ruleset priority this level is shown and escalated as: always observe.
    pub const fn priority(self) -> Priority {
        match self {
            Self::Observe => Priority::Observe,
        }
    }
}

/// An advisory notice from a model output (SR-002: observe level only; F-ML-04: explained).
/// Fields are private: a notice is built only by [`AdvisoryNotice::from_result`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdvisoryNotice {
    level: AdvisoryLevel,
    title: String,
    action: String,
    result: AdvisoryResult,
}

impl AdvisoryNotice {
    /// The notice for a result whose manifest notice condition holds; `None` otherwise.
    pub fn from_result(result: &AdvisoryResult) -> Option<Self> {
        let spec = result.notice.as_ref()?;
        let holds = match spec.op {
            NoticeOp::Above => result.value > spec.threshold,
            NoticeOp::Below => result.value < spec.threshold,
        };
        holds.then(|| Self {
            level: AdvisoryLevel::Observe,
            title: format!(
                "{} (advisory model {}, {})",
                spec.title,
                result.model_id,
                match result.status {
                    DisplayStatus::Validated => "validated",
                    _ => "investigational",
                }
            ),
            action: spec.action.clone(),
            result: result.clone(),
        })
    }

    /// Level: always observe.
    pub fn level(&self) -> AdvisoryLevel {
        self.level
    }

    /// Title, naming the model and its validation status.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Suggested action.
    pub fn action(&self) -> &str {
        &self.action
    }

    /// The result behind it, with its explanation.
    pub fn result(&self) -> &AdvisoryResult {
        &self.result
    }
}

/// Why a model output may not be fed to the rule engine.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GateRefusal {
    /// The output reports no parameter.
    #[error("output reports no parameter")]
    NoParameter,
    /// The parameter is not one a model may report ([`MODEL_OUTPUT_PARAMETERS`]).
    #[error("`{0}` is not a model output parameter")]
    NotModelOutput(&'static str),
    /// A rule on the parameter is not an observe-level threshold rule.
    #[error("rule `{rule}` on `{parameter}` is not an observe-level threshold rule")]
    NotObserveOnly {
        /// Parameter key.
        parameter: &'static str,
        /// Rule.
        rule: String,
    },
    /// A rule's supersession group holds rules on other parameters.
    #[error("group `{group}` of rule `{rule}` is shared with other parameters")]
    SharedGroup {
        /// Rule.
        rule: String,
        /// Group.
        group: String,
    },
    /// The parameter is context for other rules (gate, at-rest or baseline).
    #[error("`{0}` is context for other rules")]
    Context(&'static str),
    /// The value is not usable.
    #[error("value is not finite or outside the parameter's plausible range")]
    Value,
}

/// Checks that feeding `parameter` can only affect observe-level rules on it (SR-002).
pub fn admissible(ruleset: &Ruleset, parameter: Parameter) -> Result<(), GateRefusal> {
    let key = parameter.spec().key;
    if !MODEL_OUTPUT_PARAMETERS.contains(&parameter) {
        return Err(GateRefusal::NotModelOutput(key));
    }
    let mut groups = BTreeSet::new();
    for rule in ruleset.rules_for(parameter) {
        let observe_threshold = rule.priority == Priority::Observe
            && matches!(&rule.condition, Condition::Threshold(t) if t.at_rest.is_none());
        if !observe_threshold {
            return Err(GateRefusal::NotObserveOnly {
                parameter: key,
                rule: rule.id.clone(),
            });
        }
        groups.insert(rule.group.as_str());
    }
    for rule in &ruleset.rules {
        if rule.parameter != parameter && groups.contains(rule.group.as_str()) {
            return Err(GateRefusal::SharedGroup {
                rule: rule.id.clone(),
                group: rule.group.clone(),
            });
        }
        match &rule.condition {
            Condition::Absence(a) if a.gate.0 == parameter => {
                return Err(GateRefusal::Context(key))
            }
            Condition::Threshold(t) if t.at_rest.is_some_and(|(p, _)| p == parameter) => {
                return Err(GateRefusal::Context(key))
            }
            _ => {}
        }
    }
    let b = &ruleset.baseline;
    if b.at_rest.0 == parameter
        || b.parameters.iter().any(|p| p.parameter == parameter)
        || b.parameters
            .iter()
            .any(|p| groups.contains(p.parameter.spec().key))
    {
        return Err(GateRefusal::Context(key));
    }
    Ok(())
}

/// A model output admitted towards the rule engine as an observe-only input (SR-002).
#[derive(Debug, Clone, PartialEq)]
pub struct ObserveOnlyInput {
    input: Input,
}

impl ObserveOnlyInput {
    /// Wraps a result that reports a model-derived parameter.
    pub fn new(result: &AdvisoryResult, device: Option<DeviceId>) -> Result<Self, GateRefusal> {
        let parameter = result.parameter.ok_or(GateRefusal::NoParameter)?;
        if !MODEL_OUTPUT_PARAMETERS.contains(&parameter) {
            return Err(GateRefusal::NotModelOutput(parameter.spec().key));
        }
        let in_range = match parameter.spec().value {
            ValueType::Numeric { min, max } => result.value >= min && result.value <= max,
            _ => false,
        };
        if !result.value.is_finite() || !in_range {
            return Err(GateRefusal::Value);
        }
        Ok(Self {
            input: Input {
                crew: result.crew,
                device,
                parameter,
                value: result.value,
                at: result.at,
                quality: result.quality,
                mode: MeasurementMode::Continuous,
                site: None,
            },
        })
    }

    /// The parameter.
    pub fn parameter(&self) -> Parameter {
        self.input.parameter
    }

    /// Feeds the engine, after checking admissibility against the engine's own ruleset.
    pub fn apply(self, engine: &mut Engine) -> Result<Vec<Event>, GateRefusal> {
        admissible(engine.ruleset(), self.input.parameter)?;
        Ok(engine.evaluate(&self.input, None))
    }
}
