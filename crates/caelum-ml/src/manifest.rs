//! The model package manifest (`manifest.toml`, CR-018 §3; F-ML-01).
//!
//! The manifest pins the model file by SHA-256, declares the input features and the outputs with
//! their admissible ranges, the validation status and the model card. It is the signed part of a
//! package: its signature covers the pinned model hash. Unknown fields are refused, every field is
//! required unless stated, and [`Manifest::parse`] rejects anything inconsistent.

use std::collections::BTreeSet;

use caelum_domain::parameters::{Parameter, ValueType};
use serde::{Deserialize, Serialize};

/// Longest per-inference time limit a manifest may declare (CR-018 §4).
pub const MAX_TIME_LIMIT_MS: u32 = 1_000;
/// Largest number of input values a model may take.
pub const MAX_INPUT_LEN: usize = 65_536;
/// Largest number of outputs.
pub const MAX_OUTPUTS: usize = 64;
/// Longest series feature.
pub const MAX_SERIES_LEN: u32 = 4_096;

/// Validation status of a model (F-ML-01, docs/08 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ValidationStatus {
    /// Met its gate on the frozen evaluation set; evidence recorded and medically approved.
    Validated,
    /// Not (yet) validated: shown as INVESTIGATIONAL.
    NotValidated,
    /// Research only: never loaded by release builds.
    Research,
}

impl ValidationStatus {
    /// Stored and displayed name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Validated => "VALIDATED",
            Self::NotValidated => "NOT_VALIDATED",
            Self::Research => "RESEARCH",
        }
    }
}

/// `[model]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSection {
    /// Model ID (`M1` ... `M9`, docs/08 §2).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Package version (semantic version string).
    pub version: String,
    /// File name of the ONNX model inside the package directory.
    pub file: String,
    /// SHA-256 of the model file, lower-case hex.
    pub sha256: String,
}

/// `[validation]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationSection {
    /// Status.
    pub status: ValidationStatus,
    /// The gate the model must meet (docs/08 §2).
    pub gate: String,
    /// Reference to the frozen-set evaluation report; required when `VALIDATED`.
    #[serde(default)]
    pub evidence: Option<String>,
}

/// `[card]`: the model card (docs/08 §1: data, metrics, limits).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCard {
    /// Intended use.
    pub intended_use: String,
    /// Training data (data cards).
    pub training_data: String,
    /// Evaluation: metrics on the frozen evaluation set, as produced by the pipeline.
    pub evaluation: String,
    /// Known limitations.
    pub limitations: String,
    /// Who approved the package.
    pub approver: String,
}

/// `[runtime]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSection {
    /// Hard per-inference time limit, ms (1 ..= [`MAX_TIME_LIMIT_MS`]).
    pub time_limit_ms: u32,
    /// Name of the model's input tensor (shape `[1, n]`, f32).
    pub input_name: String,
    /// Name of the model's output tensor (f32, flattened).
    pub output_name: String,
}

/// How a feature summarises its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggregate {
    /// Arithmetic mean.
    Mean,
    /// Median.
    Median,
    /// Minimum.
    Min,
    /// Maximum.
    Max,
    /// Sample standard deviation (needs 2 samples).
    Std,
    /// Most recent value.
    Last,
    /// Root mean square of successive differences (needs 2 samples).
    Rmssd,
    /// The last `length` values, oldest first (no resampling, no padding).
    Series,
}

/// `[[feature]]`: one input feature built from domain observations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureSpec {
    /// Feature name (unique).
    pub name: String,
    /// Catalog key of the source parameter.
    pub parameter: String,
    /// Summary.
    pub aggregate: Aggregate,
    /// Window ending at the evaluation time, s.
    pub window_s: u32,
    /// Fewest usable samples in the window.
    pub min_samples: u32,
    /// Longest admissible gap between usable samples (and at either end of the window), s.
    pub max_gap_s: u32,
    /// Number of values (series only).
    #[serde(default)]
    pub length: Option<u32>,
    /// Occlusion reference from the model card (training-set median), used only to compute
    /// explanations (F-ML-04); never used as an input value.
    pub reference: f64,
}

impl FeatureSpec {
    /// Number of input values the feature contributes.
    pub fn width(&self) -> usize {
        match self.aggregate {
            Aggregate::Series => self.length.unwrap_or(0) as usize,
            _ => 1,
        }
    }
}

/// Comparison for an advisory notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoticeOp {
    /// Notice when the output is above the threshold.
    #[serde(rename = ">")]
    Above,
    /// Notice when the output is below the threshold.
    #[serde(rename = "<")]
    Below,
}

/// `notice = { ... }` on an output: when the output becomes an observe-level advisory notice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoticeSpec {
    /// Direction.
    pub op: NoticeOp,
    /// Threshold (within the output's range).
    pub threshold: f64,
    /// Notice title.
    pub title: String,
    /// Suggested action.
    pub action: String,
}

/// `[[output]]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSpec {
    /// Output name (unique).
    pub name: String,
    /// Label shown in the UI.
    pub label: String,
    /// Index in the flattened output tensor.
    pub index: u32,
    /// Lowest admissible value; anything outside `[min, max]` rejects the inference.
    pub min: f64,
    /// Highest admissible value.
    pub max: f64,
    /// Catalog key of a model output parameter this output is reported as (e.g.
    /// `af_probability`, which rule `af.observe` evaluates). Admitted to the rule engine only
    /// through the observe-only gate (SR-002).
    #[serde(default)]
    pub parameter: Option<String>,
    /// Advisory notice condition.
    #[serde(default)]
    pub notice: Option<NoticeSpec>,
}

/// `[explanation]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplanationSection {
    /// Number of top contributing features reported per output (F-ML-04).
    pub top_k: u32,
}

/// The parsed manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Identity and model pin.
    pub model: ModelSection,
    /// Validation status.
    pub validation: ValidationSection,
    /// Model card.
    pub card: ModelCard,
    /// Runtime settings.
    pub runtime: RuntimeSection,
    /// Input features, in input order.
    #[serde(rename = "feature")]
    pub features: Vec<FeatureSpec>,
    /// Outputs.
    #[serde(rename = "output")]
    pub outputs: Vec<OutputSpec>,
    /// Explanation settings.
    pub explanation: ExplanationSection,
}

/// Why a manifest was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// Not valid TOML for the format (missing or unknown fields, wrong types).
    #[error("manifest does not parse: {0}")]
    Parse(String),
    /// A field is inconsistent.
    #[error("manifest field `{field}`: {reason}")]
    Invalid {
        /// Field.
        field: String,
        /// Reason.
        reason: String,
    },
}

fn invalid(field: impl Into<String>, reason: impl Into<String>) -> ManifestError {
    ManifestError::Invalid {
        field: field.into(),
        reason: reason.into(),
    }
}

fn required(field: &str, value: &str) -> Result<(), ManifestError> {
    if value.trim().is_empty() {
        return Err(invalid(field, "must not be empty"));
    }
    Ok(())
}

fn is_lower_hex(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A numeric catalog parameter by key, with its plausible range.
fn numeric_parameter(field: &str, key: &str) -> Result<(Parameter, f64, f64), ManifestError> {
    let parameter = Parameter::from_key(key)
        .ok_or_else(|| invalid(field, format!("unknown parameter `{key}`")))?;
    match parameter.spec().value {
        ValueType::Numeric { min, max } => Ok((parameter, min, max)),
        _ => Err(invalid(field, format!("parameter `{key}` is not numeric"))),
    }
}

impl Manifest {
    /// Parses and validates a manifest.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let manifest: Self =
            toml::from_str(text).map_err(|e| ManifestError::Parse(e.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Total number of input values.
    pub fn input_len(&self) -> usize {
        self.features.iter().map(FeatureSpec::width).sum()
    }

    /// The model-derived parameter an output reports, if any.
    pub fn output_parameter(output: &OutputSpec) -> Option<Parameter> {
        output.parameter.as_deref().and_then(Parameter::from_key)
    }

    fn validate(&self) -> Result<(), ManifestError> {
        let m = &self.model;
        if m.id.is_empty()
            || !m
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        {
            return Err(invalid("model.id", "must be non-empty [A-Za-z0-9._-]"));
        }
        required("model.name", &m.name)?;
        required("model.version", &m.version)?;
        if m.file.is_empty() || m.file.contains(['/', '\\', ':']) || m.file == "." || m.file == ".."
        {
            return Err(invalid("model.file", "must be a plain file name"));
        }
        if !is_lower_hex(&m.sha256, 64) {
            return Err(invalid("model.sha256", "must be 64 lower-case hex digits"));
        }
        required("validation.gate", &self.validation.gate)?;
        if self.validation.status == ValidationStatus::Validated
            && self
                .validation
                .evidence
                .as_deref()
                .is_none_or(|e| e.trim().is_empty())
        {
            return Err(invalid(
                "validation.evidence",
                "a VALIDATED model must reference its frozen-set evaluation report",
            ));
        }
        let c = &self.card;
        required("card.intended_use", &c.intended_use)?;
        required("card.training_data", &c.training_data)?;
        required("card.evaluation", &c.evaluation)?;
        required("card.limitations", &c.limitations)?;
        required("card.approver", &c.approver)?;
        let r = &self.runtime;
        if r.time_limit_ms == 0 || r.time_limit_ms > MAX_TIME_LIMIT_MS {
            return Err(invalid(
                "runtime.time_limit_ms",
                format!("must be 1 ..= {MAX_TIME_LIMIT_MS}"),
            ));
        }
        required("runtime.input_name", &r.input_name)?;
        required("runtime.output_name", &r.output_name)?;
        self.validate_features()?;
        self.validate_outputs()?;
        if self.explanation.top_k == 0 {
            return Err(invalid("explanation.top_k", "must be at least 1"));
        }
        Ok(())
    }

    fn validate_features(&self) -> Result<(), ManifestError> {
        if self.features.is_empty() {
            return Err(invalid("feature", "a model needs at least one feature"));
        }
        let mut names = BTreeSet::new();
        for (i, f) in self.features.iter().enumerate() {
            let field = format!("feature[{i}]");
            required(&format!("{field}.name"), &f.name)?;
            if !names.insert(f.name.as_str()) {
                return Err(invalid(format!("{field}.name"), "duplicate feature name"));
            }
            numeric_parameter(&format!("{field}.parameter"), &f.parameter)?;
            if f.window_s == 0 {
                return Err(invalid(format!("{field}.window_s"), "must be positive"));
            }
            if f.max_gap_s == 0 || f.max_gap_s > f.window_s {
                return Err(invalid(
                    format!("{field}.max_gap_s"),
                    "must be positive and at most window_s",
                ));
            }
            if f.min_samples == 0 {
                return Err(invalid(
                    format!("{field}.min_samples"),
                    "must be at least 1",
                ));
            }
            if matches!(f.aggregate, Aggregate::Std | Aggregate::Rmssd) && f.min_samples < 2 {
                return Err(invalid(
                    format!("{field}.min_samples"),
                    "std and rmssd need at least 2 samples",
                ));
            }
            match (f.aggregate, f.length) {
                (Aggregate::Series, Some(n)) if (1..=MAX_SERIES_LEN).contains(&n) => {
                    if f.min_samples < n {
                        return Err(invalid(
                            format!("{field}.min_samples"),
                            "a series needs at least `length` samples",
                        ));
                    }
                }
                (Aggregate::Series, _) => {
                    return Err(invalid(
                        format!("{field}.length"),
                        format!("a series needs a length 1 ..= {MAX_SERIES_LEN}"),
                    ))
                }
                (_, Some(_)) => {
                    return Err(invalid(
                        format!("{field}.length"),
                        "only a series has a length",
                    ))
                }
                (_, None) => {}
            }
            if !f.reference.is_finite() {
                return Err(invalid(format!("{field}.reference"), "must be finite"));
            }
        }
        let n = self.input_len();
        if n > MAX_INPUT_LEN {
            return Err(invalid(
                "feature",
                format!("more than {MAX_INPUT_LEN} input values"),
            ));
        }
        Ok(())
    }

    fn validate_outputs(&self) -> Result<(), ManifestError> {
        if self.outputs.is_empty() || self.outputs.len() > MAX_OUTPUTS {
            return Err(invalid(
                "output",
                format!("a model needs 1 ..= {MAX_OUTPUTS} outputs"),
            ));
        }
        let mut names = BTreeSet::new();
        let mut indices = BTreeSet::new();
        let mut parameters = BTreeSet::new();
        for (i, o) in self.outputs.iter().enumerate() {
            let field = format!("output[{i}]");
            required(&format!("{field}.name"), &o.name)?;
            required(&format!("{field}.label"), &o.label)?;
            if !names.insert(o.name.as_str()) {
                return Err(invalid(format!("{field}.name"), "duplicate output name"));
            }
            if o.index as usize >= MAX_OUTPUTS || !indices.insert(o.index) {
                return Err(invalid(
                    format!("{field}.index"),
                    format!("must be unique and below {MAX_OUTPUTS}"),
                ));
            }
            if !(o.min.is_finite() && o.max.is_finite() && o.min < o.max) {
                return Err(invalid(format!("{field}.min"), "need finite min < max"));
            }
            if let Some(key) = &o.parameter {
                let pfield = format!("{field}.parameter");
                let (parameter, pmin, pmax) = numeric_parameter(&pfield, key)?;
                if !crate::advisory::MODEL_OUTPUT_PARAMETERS.contains(&parameter) {
                    return Err(invalid(
                        pfield,
                        "a model may only report a model output parameter (af_probability, bp_trend_systolic)",
                    ));
                }
                if o.min < pmin || o.max > pmax {
                    return Err(invalid(
                        pfield,
                        "output range must lie within the parameter's plausible range",
                    ));
                }
                if !parameters.insert(parameter) {
                    return Err(invalid(pfield, "two outputs report the same parameter"));
                }
            }
            if let Some(n) = &o.notice {
                let nfield = format!("{field}.notice");
                if !(n.threshold.is_finite() && n.threshold >= o.min && n.threshold <= o.max) {
                    return Err(invalid(
                        format!("{nfield}.threshold"),
                        "must lie within the output range",
                    ));
                }
                required(&format!("{nfield}.title"), &n.title)?;
                required(&format!("{nfield}.action"), &n.action)?;
            }
        }
        Ok(())
    }
}
