//! The model registry: verified packages, their runtimes and their status (F-ML-01).
//!
//! Only [`VerifiedPackage`]s are installed. A package whose runtime cannot be created is kept
//! with status UNAVAILABLE and the reason, so the UI can show it; it produces no output. One
//! version per model ID is installed; a newer package replaces it only with a higher version
//! (a rollback removes the model first). [`Registry::evaluate`] builds the features, runs the
//! model under its hard time limit, rejects non-finite or out-of-range outputs, explains every
//! output (F-ML-04) and returns advisory results; any failure means no output.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use caelum_domain::ids::CrewMemberId;
use caelum_domain::time::TaiNs;
use serde::Serialize;

use crate::advisory::{AdvisoryResult, DisplayStatus};
use crate::explain::{self, ExplainError};
use crate::features::{self, MissingData, Sample};
use crate::manifest::{Manifest, ValidationStatus};
use crate::package::VerifiedPackage;
use crate::runtime::{run_limited, ModelRuntime, RuntimeError, RuntimeFactory};

/// What the UI shows about one model (docs/08 §1).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModelStatusView {
    /// Model ID.
    pub id: String,
    /// Name.
    pub name: String,
    /// Package version.
    pub version: String,
    /// Validation status from the signed manifest.
    pub validation: ValidationStatus,
    /// Display status.
    pub status: DisplayStatus,
    /// Why it is DISABLED or UNAVAILABLE.
    pub reason: Option<String>,
    /// Signed with development keys only.
    pub development_signed: bool,
    /// Gate the model must meet.
    pub gate: String,
    /// Model card: intended use.
    pub intended_use: String,
    /// Model card: limitations.
    pub limitations: String,
    /// SHA-256 of the pinned model file.
    pub model_sha256: String,
}

/// Why an installation was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    /// The same or a newer version is installed.
    #[error("model {id} version {installed} is installed; {offered} is not newer")]
    NotNewer {
        /// Model.
        id: String,
        /// Installed version.
        installed: String,
        /// Offered version.
        offered: String,
    },
}

/// Why an evaluation produced no output.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvaluationError {
    /// No such model.
    #[error("no model {0}")]
    UnknownModel(String),
    /// The model is disabled.
    #[error("model {0} is disabled")]
    Disabled(String),
    /// No runtime for the model.
    #[error("model {id} is unavailable: {reason}")]
    Unavailable {
        /// Model.
        id: String,
        /// Reason.
        reason: String,
    },
    /// Missing input data: NO DATA, nothing imputed.
    #[error(transparent)]
    NoData(#[from] MissingData),
    /// The runtime failed, timed out or is busy.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    /// The output tensor has too few values.
    #[error("model output has {found} values; output index {index} is declared")]
    OutputShape {
        /// Values found.
        found: usize,
        /// Declared index.
        index: usize,
    },
    /// An output is non-finite or outside its declared range: the whole inference is rejected.
    #[error("output `{output}` = {value} is not finite or outside [{min}, {max}]")]
    Implausible {
        /// Output.
        output: String,
        /// Value (as text, so NaN compares).
        value: String,
        /// Range.
        min: String,
        /// Range.
        max: String,
    },
    /// No explanation: no output (F-ML-04).
    #[error(transparent)]
    Explanation(#[from] ExplainError),
}

enum State {
    Ready(Arc<dyn ModelRuntime>),
    Unavailable(String),
    Disabled(String),
}

struct Entry {
    package: VerifiedPackage,
    state: State,
    in_flight: Arc<AtomicBool>,
}

/// Compares dotted version strings numerically where possible (`1.10.0` > `1.9.0`).
fn version_newer(offered: &str, installed: &str) -> bool {
    let parse = |v: &str| -> Vec<(u64, String)> {
        v.split(['.', '-', '+'])
            .map(|p| (p.parse::<u64>().unwrap_or(u64::MAX), p.to_owned()))
            .collect()
    };
    parse(offered) > parse(installed)
}

/// Verified models and their runtimes.
#[derive(Default)]
pub struct Registry {
    entries: BTreeMap<String, Entry>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs a verified package, creating its runtime with `factory`. A runtime that cannot be
    /// created leaves the model UNAVAILABLE (with the reason); it is still listed.
    pub fn install(
        &mut self,
        package: VerifiedPackage,
        factory: &dyn RuntimeFactory,
    ) -> Result<DisplayStatus, InstallError> {
        let id = package.manifest().model.id.clone();
        if let Some(existing) = self.entries.get(&id) {
            let installed = &existing.package.manifest().model.version;
            let offered = &package.manifest().model.version;
            if !version_newer(offered, installed) {
                return Err(InstallError::NotNewer {
                    id,
                    installed: installed.clone(),
                    offered: offered.clone(),
                });
            }
        }
        let state = match factory.load(&package) {
            Ok(runtime) => State::Ready(runtime),
            Err(e) => State::Unavailable(e.to_string()),
        };
        let entry = Entry {
            package,
            state,
            in_flight: Arc::new(AtomicBool::new(false)),
        };
        let status = Self::display(&entry);
        self.entries.insert(id, entry);
        Ok(status)
    }

    /// Removes a model (rollback goes through removal). Returns whether it was installed.
    pub fn remove(&mut self, id: &str) -> bool {
        self.entries.remove(id).is_some()
    }

    /// Disables a model; it produces no output until re-enabled.
    pub fn disable(&mut self, id: &str, reason: &str) -> bool {
        match self.entries.get_mut(id) {
            Some(e) => {
                if let State::Ready(_) = e.state {
                    e.state = State::Disabled(reason.to_owned());
                }
                true
            }
            None => false,
        }
    }

    /// Re-enables a disabled model by reloading it with `factory`.
    pub fn enable(&mut self, id: &str, factory: &dyn RuntimeFactory) -> Option<DisplayStatus> {
        let e = self.entries.get_mut(id)?;
        if !matches!(e.state, State::Ready(_)) {
            e.state = match factory.load(&e.package) {
                Ok(runtime) => State::Ready(runtime),
                Err(err) => State::Unavailable(err.to_string()),
            };
        }
        Some(Self::display(e))
    }

    fn display(e: &Entry) -> DisplayStatus {
        match e.state {
            State::Ready(_) => {
                DisplayStatus::for_validation(e.package.manifest().validation.status)
            }
            State::Unavailable(_) => DisplayStatus::Unavailable,
            State::Disabled(_) => DisplayStatus::Disabled,
        }
    }

    /// Status of every installed model, by ID (for the UI).
    pub fn statuses(&self) -> Vec<ModelStatusView> {
        self.entries
            .values()
            .map(|e| {
                let m = e.package.manifest();
                ModelStatusView {
                    id: m.model.id.clone(),
                    name: m.model.name.clone(),
                    version: m.model.version.clone(),
                    validation: m.validation.status,
                    status: Self::display(e),
                    reason: match &e.state {
                        State::Ready(_) => None,
                        State::Unavailable(r) | State::Disabled(r) => Some(r.clone()),
                    },
                    development_signed: e.package.development(),
                    gate: m.validation.gate.clone(),
                    intended_use: m.card.intended_use.clone(),
                    limitations: m.card.limitations.clone(),
                    model_sha256: m.model.sha256.clone(),
                }
            })
            .collect()
    }

    /// The manifest of an installed model.
    pub fn manifest(&self, id: &str) -> Option<&Manifest> {
        self.entries.get(id).map(|e| e.package.manifest())
    }

    /// Evaluates one model for one crew member on that member's samples at `now`.
    pub fn evaluate(
        &self,
        id: &str,
        crew: CrewMemberId,
        samples: &[Sample],
        now: TaiNs,
    ) -> Result<Vec<AdvisoryResult>, EvaluationError> {
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| EvaluationError::UnknownModel(id.to_owned()))?;
        let runtime = match &entry.state {
            State::Ready(r) => r,
            State::Unavailable(reason) => {
                return Err(EvaluationError::Unavailable {
                    id: id.to_owned(),
                    reason: reason.clone(),
                })
            }
            State::Disabled(_) => return Err(EvaluationError::Disabled(id.to_owned())),
        };
        let manifest = entry.package.manifest();
        let limit = manifest.runtime.time_limit_ms;
        let vector = features::build(&manifest.features, samples, now)?;
        let outputs = run_limited(runtime, vector.values.clone(), limit, &entry.in_flight)?;
        let mut indices = Vec::with_capacity(manifest.outputs.len());
        let mut values = Vec::with_capacity(manifest.outputs.len());
        for spec in &manifest.outputs {
            let index = spec.index as usize;
            let value =
                outputs
                    .get(index)
                    .map(|v| f64::from(*v))
                    .ok_or(EvaluationError::OutputShape {
                        found: outputs.len(),
                        index,
                    })?;
            if !(value.is_finite() && value >= spec.min && value <= spec.max) {
                return Err(EvaluationError::Implausible {
                    output: spec.name.clone(),
                    value: value.to_string(),
                    min: spec.min.to_string(),
                    max: spec.max.to_string(),
                });
            }
            indices.push(index);
            values.push(value);
        }
        let explanations = explain::occlusion(
            runtime,
            &vector,
            &outputs,
            &indices,
            manifest.explanation.top_k as usize,
            limit,
            &entry.in_flight,
        )?;
        let status = DisplayStatus::for_validation(manifest.validation.status);
        Ok(manifest
            .outputs
            .iter()
            .zip(values)
            .zip(explanations)
            .map(|((spec, value), contributions)| AdvisoryResult {
                model_id: manifest.model.id.clone(),
                model_version: manifest.model.version.clone(),
                validation: manifest.validation.status,
                status,
                development_signed: entry.package.development(),
                crew,
                output: spec.name.clone(),
                label: spec.label.clone(),
                value,
                at: vector.at,
                quality: vector.quality,
                contributions,
                parameter: Manifest::output_parameter(spec),
                notice: spec.notice.clone(),
            })
            .collect())
    }
}
