//! Personal baseline estimation (F-ALM-08, CR-012 §6): median and median absolute deviation
//! over a window, with a minimum sample count and a method version, stored by the caller in the
//! `baseline` table (docs/05 §2) and given to the alarm engine as `BaselineStats`.
//!
//! - Method `baseline-median-mad/1`. MAD is unscaled; the engine applies 1.4826 for the robust
//!   z-score.
//! - Only samples inside `[now − window, now]`, finite, with quality usable for rules (SR-005).
//! - "At rest" baselines (heart rate, SpO₂) use only samples whose activity context says
//!   [`ActivityContext::Rest`]. A sample with unknown activity is **excluded**, never assumed to
//!   be at rest (SR-018). Too few samples → no baseline; nothing is made up.
//! - A zero MAD (all samples equal) gives no baseline either: a z-score against it would be
//!   meaningless.

use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;

use crate::stats::{mad, median};
use crate::Quality;

/// Method identifier stored as `baseline.method_version`.
pub const BASELINE_METHOD_VERSION: &str = "baseline-median-mad/1";

const NS_PER_S: i64 = 1_000_000_000;

/// Activity context of a sample (SR-018).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActivityContext {
    /// Activity level known and at most the at-rest limit.
    Rest,
    /// Activity level known and above the limit.
    Active,
    /// No current activity level.
    Unknown,
}

impl ActivityContext {
    /// Context from the current `activity_level` (if any) and the ruleset's at-rest maximum
    /// (`[baseline] at_rest.max`, 0.3 in the draft ruleset).
    pub fn from_activity_level(level: Option<f64>, at_rest_max: f64) -> Self {
        match level {
            Some(v) if v.is_finite() && v <= at_rest_max => Self::Rest,
            Some(v) if v.is_finite() => Self::Active,
            _ => Self::Unknown,
        }
    }
}

/// One observation offered to the estimator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BaselineSample {
    /// Measurement time.
    pub at: TaiNs,
    /// Value in the parameter's canonical unit.
    pub value: f64,
    /// Observation quality.
    pub quality: Quality,
    /// Activity context at that time.
    pub activity: ActivityContext,
}

/// How a parameter's baseline is estimated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaselineSpec {
    /// Window length, s.
    pub window_s: i64,
    /// Minimum number of qualifying samples.
    pub min_samples: usize,
    /// Only samples at rest qualify.
    pub at_rest: bool,
}

/// ⚕ DRAFT (CR-013 §11) window and minimum count for the parameters with baseline rules in the
/// ruleset (`[baseline] parameters`); `at_rest` matches the ruleset. `None` for other parameters.
/// Continuous parameters assume the caller offers one value per minute (e.g. minute medians), so
/// 1 440 samples ≈ one day of qualifying data.
pub fn draft_spec(parameter: Parameter) -> Option<BaselineSpec> {
    const WEEK: i64 = 7 * 86_400;
    let spec = |window_s, min_samples, at_rest| BaselineSpec {
        window_s,
        min_samples,
        at_rest,
    };
    match parameter {
        Parameter::HeartRate => Some(spec(WEEK, 1_440, true)),
        Parameter::HrvRmssd => Some(spec(WEEK, 288, false)),
        Parameter::RespirationRate => Some(spec(WEEK, 1_440, false)),
        Parameter::CoreTemp => Some(spec(WEEK, 1_440, false)),
        Parameter::Spo2 => Some(spec(WEEK, 1_440, true)),
        Parameter::Weight => Some(spec(30 * 86_400, 5, false)),
        _ => None,
    }
}

/// An established baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Baseline {
    /// Median.
    pub median: f64,
    /// Median absolute deviation (unscaled).
    pub mad: f64,
    /// Samples used.
    pub n: usize,
    /// Window start.
    pub window_start: TaiNs,
    /// Window end.
    pub window_end: TaiNs,
    /// [`BASELINE_METHOD_VERSION`].
    pub method_version: &'static str,
}

/// Outcome of an estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BaselineOutcome {
    /// A baseline exists.
    Established(Baseline),
    /// Too few qualifying samples: no baseline (the Body trend says so).
    Insufficient {
        /// Qualifying samples found.
        n: usize,
        /// Samples needed.
        needed: usize,
    },
    /// All qualifying samples are equal (MAD 0): no usable baseline.
    NoSpread {
        /// Qualifying samples found.
        n: usize,
    },
}

/// Estimates a baseline at `now` (F-ALM-08, SR-018).
pub fn estimate_baseline(
    samples: &[BaselineSample],
    spec: BaselineSpec,
    now: TaiNs,
) -> BaselineOutcome {
    let start = TaiNs(now.0.saturating_sub(spec.window_s.saturating_mul(NS_PER_S)));
    let values: Vec<f64> = samples
        .iter()
        .filter(|s| s.at >= start && s.at <= now)
        .filter(|s| s.value.is_finite() && s.quality.usable_for_rules())
        .filter(|s| !spec.at_rest || s.activity == ActivityContext::Rest)
        .map(|s| s.value)
        .collect();
    let n = values.len();
    if n < spec.min_samples.max(1) {
        return BaselineOutcome::Insufficient {
            n,
            needed: spec.min_samples.max(1),
        };
    }
    let Some(m) = median(&values) else {
        return BaselineOutcome::Insufficient { n, needed: 1 };
    };
    match mad(&values, m) {
        Some(d) if d > 0.0 => BaselineOutcome::Established(Baseline {
            median: m,
            mad: d,
            n,
            window_start: start,
            window_end: now,
            method_version: BASELINE_METHOD_VERSION,
        }),
        _ => BaselineOutcome::NoSpread { n },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(t: i64, v: f64, activity: ActivityContext) -> BaselineSample {
        BaselineSample {
            at: TaiNs(t * NS_PER_S),
            value: v,
            quality: Quality::Good,
            activity,
        }
    }

    const SPEC: BaselineSpec = BaselineSpec {
        window_s: 1_000,
        min_samples: 5,
        at_rest: true,
    };

    /// F-ALM-08: median and MAD over the window, with the method version.
    #[test]
    fn f_alm_08_median_and_mad() {
        let samples: Vec<_> = [60.0, 62.0, 58.0, 61.0, 59.0, 200.0]
            .iter()
            .enumerate()
            .map(|(i, &v)| s(100 + i as i64, v, ActivityContext::Rest))
            .collect();
        let BaselineOutcome::Established(b) =
            estimate_baseline(&samples, SPEC, TaiNs(1_000 * NS_PER_S))
        else {
            panic!("expected a baseline");
        };
        assert_eq!(b.median, 60.5);
        assert_eq!(b.mad, 1.5);
        assert_eq!(b.n, 6);
        assert_eq!(b.method_version, BASELINE_METHOD_VERSION);
    }

    /// SR-018: unknown activity is never counted as rest; too few samples → no baseline.
    #[test]
    fn sr_018_unknown_context_gives_no_rest_baseline() {
        let samples: Vec<_> = (0..10)
            .map(|i| s(i, 60.0 + i as f64, ActivityContext::Unknown))
            .collect();
        assert_eq!(
            estimate_baseline(&samples, SPEC, TaiNs(100 * NS_PER_S)),
            BaselineOutcome::Insufficient { n: 0, needed: 5 }
        );
        let any = BaselineSpec {
            at_rest: false,
            ..SPEC
        };
        assert!(matches!(
            estimate_baseline(&samples, any, TaiNs(100 * NS_PER_S)),
            BaselineOutcome::Established(_)
        ));
        assert_eq!(
            ActivityContext::from_activity_level(None, 0.3),
            ActivityContext::Unknown
        );
        assert_eq!(
            ActivityContext::from_activity_level(Some(0.3), 0.3),
            ActivityContext::Rest
        );
        assert_eq!(
            ActivityContext::from_activity_level(Some(0.31), 0.3),
            ActivityContext::Active
        );
        assert_eq!(
            ActivityContext::from_activity_level(Some(f64::NAN), 0.3),
            ActivityContext::Unknown
        );
    }

    /// F-ALM-08: samples outside the window, poor quality or active are excluded; equal values
    /// give no baseline.
    #[test]
    fn f_alm_08_exclusions_and_no_spread() {
        let mut samples: Vec<_> = (0..5)
            .map(|i| s(900 + i, 70.0, ActivityContext::Rest))
            .collect();
        assert_eq!(
            estimate_baseline(&samples, SPEC, TaiNs(1_000 * NS_PER_S)),
            BaselineOutcome::NoSpread { n: 5 }
        );
        samples.push(s(-10, 90.0, ActivityContext::Rest)); // outside the window
        samples.push(s(950, 90.0, ActivityContext::Active));
        samples.push(BaselineSample {
            quality: Quality::Poor,
            ..s(960, 90.0, ActivityContext::Rest)
        });
        assert_eq!(
            estimate_baseline(&samples, SPEC, TaiNs(1_000 * NS_PER_S)),
            BaselineOutcome::NoSpread { n: 5 }
        );
        assert_eq!(draft_spec(Parameter::Spo2).map(|s| s.at_rest), Some(true));
        assert_eq!(draft_spec(Parameter::Glucose), None);
    }
}
