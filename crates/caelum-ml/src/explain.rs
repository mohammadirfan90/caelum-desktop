//! Explanations: top contributing features for every model output (F-ML-04).
//!
//! Occlusion over the runtime: for each feature, the inference is repeated with that feature's
//! values replaced by its reference (the training-set median from the model card) and the change
//! of every output is its contribution. Each repeat runs under the same hard time limit. If any
//! repeat fails, times out or gives a non-finite value, there is no explanation and therefore no
//! output (an output is never shown without its explanation).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Serialize;

use crate::features::FeatureVector;
use crate::runtime::{run_limited, ModelRuntime, RuntimeError};

/// One feature's contribution to one output.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contribution {
    /// Feature name.
    pub feature: String,
    /// The feature's value (scalar features); `None` for a series.
    pub value: Option<f64>,
    /// The occlusion reference.
    pub reference: f64,
    /// Output with the feature minus output with the feature at its reference.
    pub delta: f64,
}

/// Why an explanation could not be computed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExplainError {
    /// An occlusion inference failed.
    #[error("occlusion of `{feature}`: {source}")]
    Runtime {
        /// Feature.
        feature: String,
        /// Error.
        source: RuntimeError,
    },
    /// An occlusion inference gave the wrong number of values or a non-finite value.
    #[error("occlusion of `{feature}` gave an unusable output")]
    Unusable {
        /// Feature.
        feature: String,
    },
}

/// Per-output contributions, sorted by magnitude (largest first), at most `top_k` each.
pub fn occlusion(
    runtime: &Arc<dyn ModelRuntime>,
    vector: &FeatureVector,
    baseline_outputs: &[f32],
    output_indices: &[usize],
    top_k: usize,
    limit_ms: u32,
    in_flight: &Arc<AtomicBool>,
) -> Result<Vec<Vec<Contribution>>, ExplainError> {
    let mut per_output: Vec<Vec<Contribution>> = vec![Vec::new(); output_indices.len()];
    for slot in &vector.slots {
        let mut occluded = vector.values.clone();
        let reference = slot.reference as f32;
        for v in occluded.iter_mut().skip(slot.start).take(slot.width) {
            *v = reference;
        }
        let out = run_limited(runtime, occluded, limit_ms, in_flight).map_err(|source| {
            ExplainError::Runtime {
                feature: slot.name.clone(),
                source,
            }
        })?;
        if out.len() != baseline_outputs.len() {
            return Err(ExplainError::Unusable {
                feature: slot.name.clone(),
            });
        }
        for (k, &index) in output_indices.iter().enumerate() {
            let (Some(with), Some(without)) = (baseline_outputs.get(index), out.get(index)) else {
                return Err(ExplainError::Unusable {
                    feature: slot.name.clone(),
                });
            };
            let delta = f64::from(*with) - f64::from(*without);
            if !delta.is_finite() {
                return Err(ExplainError::Unusable {
                    feature: slot.name.clone(),
                });
            }
            if let Some(list) = per_output.get_mut(k) {
                list.push(Contribution {
                    feature: slot.name.clone(),
                    value: slot.value,
                    reference: slot.reference,
                    delta,
                });
            }
        }
    }
    for list in &mut per_output {
        // Largest magnitude first; ties keep manifest order (stable sort): deterministic.
        list.sort_by(|a, b| b.delta.abs().total_cmp(&a.delta.abs()));
        list.truncate(top_k);
    }
    Ok(per_output)
}
