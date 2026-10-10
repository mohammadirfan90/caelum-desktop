//! Deterministic signal processing: ECG filtering and QRS detection, HRV, signal quality indices,
//! respiration, SpO₂, core-temperature estimate, activity and sleep/wake, pulse arrival time and
//! BP trend, and personal baselines.
//!
//! Implemented in M7: F-SIG-01..07, F-ALM-08 baseline estimation (CR-012 §6). Algorithm choices
//! and draft constants are recorded in CR-013.
//!
//! Rules for this crate:
//! - **Pure and sans-I/O.** Every function is a deterministic function of its inputs (no clock,
//!   no randomness, no global state); streaming processors hold only the state they are given.
//! - **No fabricated values** (CLAUDE.md rule 2). When an output cannot be computed from the data
//!   (too short, too noisy, missing calibration or context) the result says so and carries no
//!   number. Calibrations (SpO₂ curve, BP cuff points, core-temperature start value) are inputs;
//!   no generic default is ever substituted.
//! - **Every derived output carries an algorithm version** ([`Derived::algorithm`]), stored as
//!   the observation's `algorithm_version` (docs/05 §2).
//! - Constants marked ⚕ DRAFT are engineering values pending the provisional medical lead
//!   (CR-003, CR-013).

pub mod activity;
pub mod baseline;
pub mod core_temp;
pub mod ecg;
pub mod filter;
pub mod hrv;
pub mod pat;
pub mod ppg;
pub mod respiration;
pub mod sqi;
mod stats;

pub use caelum_domain::observation::Quality;

/// A derived value with its quality and the version of the algorithm that produced it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Derived<T> {
    /// The value, in the catalog's canonical unit for the parameter it feeds.
    pub value: T,
    /// Quality of the value (SR-005: POOR and UNUSABLE pause physiological rules).
    pub quality: Quality,
    /// Algorithm identifier and version, e.g. `qrs-pan-tompkins/1`.
    pub algorithm: &'static str,
}

/// Errors for inputs a function cannot work with. Ordinary "not enough signal" outcomes are not
/// errors: they are `None` or a dedicated outcome variant.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SignalError {
    /// The sample rate is not finite or outside the range the algorithm supports.
    #[error("sample rate {got} Hz outside the supported range {min}..={max} Hz")]
    SampleRate {
        /// The rate given.
        got: f64,
        /// Lowest supported rate.
        min: f64,
        /// Highest supported rate.
        max: f64,
    },
    /// A sample is NaN or infinite. Callers treat this like a data gap.
    #[error("non-finite sample")]
    NonFinite,
    /// Channel arrays that must be the same length are not.
    #[error("channel lengths differ")]
    LengthMismatch,
    /// A parameter or calibration value is invalid.
    #[error("invalid {0}")]
    Invalid(&'static str),
}

/// Checks a sample rate against an algorithm's supported range.
pub(crate) fn check_rate(fs: f64, min: f64, max: f64) -> Result<(), SignalError> {
    if fs.is_finite() && fs >= min && fs <= max {
        Ok(())
    } else {
        Err(SignalError::SampleRate { got: fs, min, max })
    }
}

/// Fails if any sample is not finite.
pub(crate) fn check_finite(x: &[f64]) -> Result<(), SignalError> {
    if x.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(SignalError::NonFinite)
    }
}

/// ⚕ DRAFT (CR-013 §3): maps a signal-quality index in `[0, 1]` to the observation quality.
/// GOOD ≥ 0.8, DEGRADED ≥ 0.6, POOR ≥ 0.3, otherwise UNUSABLE. The `asystole.critical` gate
/// (`sqi_ecg` ≥ 0.7) therefore opens only for GOOD and the upper part of DEGRADED.
pub fn quality_from_sqi(sqi: f64) -> Quality {
    if !sqi.is_finite() {
        Quality::Unusable
    } else if sqi >= 0.8 {
        Quality::Good
    } else if sqi >= 0.6 {
        Quality::Degraded
    } else if sqi >= 0.3 {
        Quality::Poor
    } else {
        Quality::Unusable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F-SIG-02: SQI bands map to qualities; NaN is unusable, never good.
    #[test]
    fn f_sig_02_sqi_bands_map_to_quality() {
        assert_eq!(quality_from_sqi(1.0), Quality::Good);
        assert_eq!(quality_from_sqi(0.8), Quality::Good);
        assert_eq!(quality_from_sqi(0.7), Quality::Degraded);
        assert_eq!(quality_from_sqi(0.5), Quality::Poor);
        assert_eq!(quality_from_sqi(0.1), Quality::Unusable);
        assert_eq!(quality_from_sqi(f64::NAN), Quality::Unusable);
    }

    #[test]
    fn rate_and_finiteness_checks() {
        assert!(check_rate(250.0, 50.0, 2000.0).is_ok());
        assert!(check_rate(f64::NAN, 50.0, 2000.0).is_err());
        assert!(check_rate(10.0, 50.0, 2000.0).is_err());
        assert_eq!(
            check_finite(&[1.0, f64::INFINITY]),
            Err(SignalError::NonFinite)
        );
    }
}
