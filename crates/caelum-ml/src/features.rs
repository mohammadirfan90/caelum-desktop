//! Feature builders: domain observations to a model input vector (F-ML-01, CLAUDE.md rule 2).
//!
//! Each feature summarises the observations of one parameter in a window ending at the
//! evaluation time. Readings of POOR or UNUSABLE quality (SR-005) and non-finite values are not
//! used and count as gaps. A feature with too few usable samples, a gap longer than its
//! `max_gap_s` (including at either end of the window) or a value that does not fit an f32 is
//! missing, and a vector with any missing feature is not built: the model produces no output.
//! Nothing is ever imputed, padded or defaulted.

use caelum_domain::observation::Quality;
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;

use crate::manifest::{Aggregate, FeatureSpec};

const NS_PER_S: i64 = 1_000_000_000;

/// One numeric observation of one crew member.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Parameter.
    pub parameter: Parameter,
    /// Value in the parameter's canonical unit.
    pub value: f64,
    /// Measurement time.
    pub at: TaiNs,
    /// Quality.
    pub quality: Quality,
}

/// Why a feature is missing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MissingData {
    /// The feature's parameter key is not in the catalog (manifests are validated, so only a
    /// hand-built spec can cause this).
    #[error("feature `{feature}`: unknown parameter")]
    UnknownParameter {
        /// Feature.
        feature: String,
    },
    /// Too few usable samples in the window.
    #[error("feature `{feature}`: {found} usable samples, {needed} needed")]
    TooFewSamples {
        /// Feature.
        feature: String,
        /// Found.
        found: usize,
        /// Needed.
        needed: usize,
    },
    /// A gap longer than `max_gap_s`.
    #[error("feature `{feature}`: gap of {gap_ms} ms exceeds {max_gap_s} s")]
    Gap {
        /// Feature.
        feature: String,
        /// Gap, ms.
        gap_ms: i64,
        /// Limit, s.
        max_gap_s: u32,
    },
    /// The value is not representable as a finite f32.
    #[error("feature `{feature}`: value not representable")]
    NotRepresentable {
        /// Feature.
        feature: String,
    },
}

/// Where a feature sits in the input vector, with its value for explanations.
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureSlot {
    /// Feature name.
    pub name: String,
    /// First input index.
    pub start: usize,
    /// Number of input values.
    pub width: usize,
    /// The feature value (scalar features) shown with explanations; `None` for a series.
    pub value: Option<f64>,
    /// Occlusion reference (from the model card).
    pub reference: f64,
}

/// A complete input vector.
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureVector {
    /// Input values, in manifest order.
    pub values: Vec<f32>,
    /// Feature layout.
    pub slots: Vec<FeatureSlot>,
    /// Worst quality among the samples used (GOOD or DEGRADED).
    pub quality: Quality,
    /// Evaluation time (end of every window).
    pub at: TaiNs,
}

fn gap_check(feature: &str, max_gap_s: u32, a: i64, b: i64) -> Result<(), MissingData> {
    let gap = b - a;
    if gap > i64::from(max_gap_s) * NS_PER_S {
        return Err(MissingData::Gap {
            feature: feature.to_owned(),
            gap_ms: gap / 1_000_000,
            max_gap_s,
        });
    }
    Ok(())
}

fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// Builds one feature. `samples` are this crew member's observations, in any order.
pub fn build_feature(
    spec: &FeatureSpec,
    samples: &[Sample],
    now: TaiNs,
) -> Result<(Vec<f64>, Quality), MissingData> {
    let parameter =
        Parameter::from_key(&spec.parameter).ok_or_else(|| MissingData::UnknownParameter {
            feature: spec.name.clone(),
        })?;
    let start = now.0.saturating_sub(i64::from(spec.window_s) * NS_PER_S);
    let mut used: Vec<&Sample> = samples
        .iter()
        .filter(|s| s.parameter == parameter && s.at.0 >= start && s.at.0 <= now.0)
        .filter(|s| s.quality.usable_for_rules() && s.value.is_finite())
        .collect();
    used.sort_by_key(|s| s.at);
    let needed = spec.min_samples as usize;
    if used.len() < needed {
        return Err(MissingData::TooFewSamples {
            feature: spec.name.clone(),
            found: used.len(),
            needed,
        });
    }
    let (Some(first), Some(last)) = (used.first(), used.last()) else {
        return Err(MissingData::TooFewSamples {
            feature: spec.name.clone(),
            found: 0,
            needed,
        });
    };
    gap_check(&spec.name, spec.max_gap_s, start, first.at.0)?;
    gap_check(&spec.name, spec.max_gap_s, last.at.0, now.0)?;
    for pair in used.windows(2) {
        gap_check(&spec.name, spec.max_gap_s, pair[0].at.0, pair[1].at.0)?;
    }
    let quality = used
        .iter()
        .map(|s| s.quality)
        .max()
        .unwrap_or(Quality::Good);
    let values: Vec<f64> = used.iter().map(|s| s.value).collect();
    let n = values.len() as f64;
    let out = match spec.aggregate {
        Aggregate::Mean => vec![values.iter().sum::<f64>() / n],
        Aggregate::Median => {
            let mut sorted = values.clone();
            sorted.sort_by(f64::total_cmp);
            vec![median(&sorted)]
        }
        Aggregate::Min => vec![values.iter().copied().fold(f64::INFINITY, f64::min)],
        Aggregate::Max => vec![values.iter().copied().fold(f64::NEG_INFINITY, f64::max)],
        Aggregate::Std => {
            let mean = values.iter().sum::<f64>() / n;
            let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
            vec![var.sqrt()]
        }
        Aggregate::Last => vec![last.value],
        Aggregate::Rmssd => {
            let diffs: Vec<f64> = values.windows(2).map(|w| (w[1] - w[0]).powi(2)).collect();
            vec![(diffs.iter().sum::<f64>() / diffs.len() as f64).sqrt()]
        }
        Aggregate::Series => {
            let len = spec.length.unwrap_or(0) as usize;
            if len == 0 || values.len() < len {
                return Err(MissingData::TooFewSamples {
                    feature: spec.name.clone(),
                    found: values.len(),
                    needed: len.max(1),
                });
            }
            values[values.len() - len..].to_vec()
        }
    };
    if out
        .iter()
        .any(|v| !v.is_finite() || !(*v as f32).is_finite())
    {
        return Err(MissingData::NotRepresentable {
            feature: spec.name.clone(),
        });
    }
    Ok((out, quality))
}

/// Builds the full input vector, or reports the first missing feature (no output then).
pub fn build(
    specs: &[FeatureSpec],
    samples: &[Sample],
    now: TaiNs,
) -> Result<FeatureVector, MissingData> {
    let mut values = Vec::new();
    let mut slots = Vec::with_capacity(specs.len());
    let mut quality = Quality::Good;
    for spec in specs {
        let (v, q) = build_feature(spec, samples, now)?;
        quality = quality.max(q);
        slots.push(FeatureSlot {
            name: spec.name.clone(),
            start: values.len(),
            width: v.len(),
            value: if spec.aggregate == Aggregate::Series {
                None
            } else {
                v.first().copied()
            },
            reference: spec.reference,
        });
        values.extend(v.iter().map(|x| *x as f32));
    }
    Ok(FeatureVector {
        values,
        slots,
        quality,
        at: now,
    })
}
