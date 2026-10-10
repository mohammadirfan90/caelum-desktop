//! Signal quality indices for ECG, PPG and respiration (F-SIG-02); they drive alarm gating
//! (SR-005, `asystole.critical` gate `sqi_ecg ≥ 0.7`).
//!
//! Design principle (CR-013 §3): an SQI measures **contamination and sensor faults**, not the
//! presence of physiology. A clean isoelectric line during an asystole is a *good* signal with no
//! beats (the absence-of-event rule must fire); broadband noise, a flat line from a disconnected
//! electrode, clipping or motion are *bad* signals (rules pause and a technical alarm is raised).
//!
//! Each index is the minimum of component scores in `[0, 1]`; the components that fall below
//! 0.8 are reported as findings so the UI and alarm "why" can say what is wrong. All thresholds
//! are ⚕ DRAFT engineering values (CR-013 §3).

use crate::filter::Cascade;
use crate::stats::{self, percentile, ramp, rms};
use crate::{check_finite, check_rate, quality_from_sqi, Quality, SignalError};

/// ECG SQI algorithm.
pub const ECG_SQI_ALGORITHM: &str = "sqi-ecg/1";
/// PPG SQI algorithm.
pub const PPG_SQI_ALGORITHM: &str = "sqi-ppg/1";
/// Respiration SQI algorithm.
pub const RESP_SQI_ALGORITHM: &str = "sqi-resp/1";

/// Why a signal scored low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SqiFinding {
    /// No variation at all: electrode or sensor disconnected, amplifier fault.
    FlatLine,
    /// Many samples at the extreme values: amplifier or ADC saturation.
    Clipping,
    /// Broadband noise between beats (muscle, electrode, interference).
    HighFrequencyNoise,
    /// Very large low-frequency excursions (motion, electrode movement).
    BaselineExcursion,
    /// Detected beats do not resemble each other (noise detected as beats).
    InconsistentBeats,
    /// Beats closer together than physiologically possible.
    ImplausibleBeatRate,
    /// Accelerometer shows motion strong enough to corrupt the signal.
    Motion,
    /// No dominant pulse period.
    NotPeriodic,
    /// Pulsatile component too small relative to the DC level.
    LowPerfusion,
}

/// A signal quality index.
#[derive(Debug, Clone, PartialEq)]
pub struct SqiResult {
    /// Score in `[0, 1]` (→ `sqi_ecg`, `sqi_ppg`, `sqi_resp`).
    pub score: f64,
    /// Quality class from [`quality_from_sqi`].
    pub quality: Quality,
    /// Algorithm identifier.
    pub algorithm: &'static str,
    /// Components below 0.8.
    pub findings: Vec<SqiFinding>,
}

struct Scorer {
    score: f64,
    findings: Vec<SqiFinding>,
}

impl Scorer {
    fn new() -> Self {
        Self {
            score: 1.0,
            findings: Vec::new(),
        }
    }

    fn add(&mut self, component: f64, finding: SqiFinding) {
        let c = if component.is_finite() {
            component.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if c < 0.8 {
            self.findings.push(finding);
        }
        self.score = self.score.min(c);
    }

    fn finish(self, algorithm: &'static str) -> SqiResult {
        SqiResult {
            score: self.score,
            quality: quality_from_sqi(self.score),
            algorithm,
            findings: self.findings,
        }
    }
}

/// Fraction of samples in flat runs at the window's minimum or maximum value lasting at least
/// 50 ms (and 3 samples): saturation shows as flat tops, whereas a waveform peak that merely
/// repeats the same value once per cycle does not.
fn clipped_fraction(x: &[f64], fs: f64) -> f64 {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for &v in x {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if x.is_empty() || lo == hi {
        return 0.0;
    }
    let min_run = ((0.05 * fs).round() as usize).max(3);
    let mut clipped = 0;
    let mut run = 0;
    let mut prev: Option<f64> = None;
    for &v in x {
        let extreme = v == lo || v == hi;
        if extreme && prev == Some(v) {
            run += 1;
        } else {
            if run >= min_run {
                clipped += run;
            }
            run = usize::from(extreme);
        }
        prev = Some(v);
    }
    if run >= min_run {
        clipped += run;
    }
    clipped as f64 / x.len() as f64
}

/// Robust range p99 − p1.
fn robust_range(x: &[f64]) -> f64 {
    let s = stats::sorted(x);
    match (
        stats::percentile_sorted(&s, 99.0),
        stats::percentile_sorted(&s, 1.0),
    ) {
        (Some(a), Some(b)) => a - b,
        _ => 0.0,
    }
}

/// ECG SQI over a window of at least 2 s in millivolts; `beats` are R-peak indices into `x`
/// (from the QRS detector). Components (⚕ DRAFT):
/// - flat line: p99 − p1 of the 0.5–40 Hz ECG < 0.01 mV → 0;
/// - clipping: ≥ 5 % of samples at the extreme values → 0 (linear from 1 %);
/// - noise: RMS of the 10–40 Hz band outside ±80 ms of each beat, 1 at ≤ 0.03 mV, 0 at
///   ≥ 0.12 mV;
/// - baseline excursion: p99 − p1 of the < 1 Hz component, 1 at ≤ 3 mV, 0 at ≥ 8 mV;
/// - with ≥ 3 beats: beat self-consistency, each beat's best correlation with another beat
///   (±100 ms), median 1 at ≥ 0.9, 0 at ≤ 0.5 (ectopic beats still match each other);
/// - with ≥ 3 beats: fraction of RR < 250 ms, 1 at ≤ 5 %, 0 at ≥ 30 %.
pub fn ecg_sqi(x: &[f64], fs: f64, beats: &[usize]) -> Result<SqiResult, SignalError> {
    check_rate(fs, 100.0, 2_000.0)?;
    check_finite(x)?;
    let mut s = Scorer::new();
    if (x.len() as f64) < 2.0 * fs {
        s.add(0.0, SqiFinding::FlatLine);
        return Ok(s.finish(ECG_SQI_ALGORITHM));
    }
    let clean = Cascade::bandpass(fs, 0.5, 40.0, 2)?.filtfilt(x);
    if robust_range(&clean) < 0.01 {
        s.add(0.0, SqiFinding::FlatLine);
        return Ok(s.finish(ECG_SQI_ALGORITHM));
    }
    s.add(
        ramp(clipped_fraction(x, fs), 0.01, 0.05),
        SqiFinding::Clipping,
    );

    let hi = 40.0_f64.min(0.45 * fs);
    let hf = Cascade::bandpass(fs, 10.0, hi, 2)?.filtfilt(x);
    let guard = (0.08 * fs).round() as usize;
    let mut keep = vec![true; x.len()];
    for &b in beats {
        let lo = b.saturating_sub(guard);
        let hi = (b + guard + 1).min(x.len());
        for k in keep.iter_mut().take(hi).skip(lo) {
            *k = false;
        }
    }
    let outside: Vec<f64> = hf
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(v, _)| *v)
        .collect();
    let noise = if outside.len() * 10 >= x.len() * 3 {
        rms(&outside)
    } else {
        rms(&hf) // beats everywhere: the noise is everywhere too
    }
    .unwrap_or(f64::INFINITY);
    s.add(ramp(noise, 0.03, 0.12), SqiFinding::HighFrequencyNoise);

    let low = Cascade::butter_lowpass(fs, 1.0, 2)?.filtfilt(x);
    s.add(
        ramp(robust_range(&low), 3.0, 8.0),
        SqiFinding::BaselineExcursion,
    );

    if beats.len() >= 3 {
        let h = (0.1 * fs).round() as usize;
        let segs: Vec<&[f64]> = beats
            .iter()
            .filter(|&&b| b >= h && b + h < clean.len())
            .filter_map(|&b| clean.get(b - h..=b + h))
            .collect();
        if segs.len() >= 3 {
            let best: Vec<f64> = (0..segs.len())
                .map(|i| {
                    (0..segs.len())
                        .filter(|&j| j != i)
                        .filter_map(|j| stats::correlation(segs[i], segs[j]))
                        .fold(-1.0_f64, f64::max)
                })
                .collect();
            let m = stats::median(&best).unwrap_or(-1.0);
            s.add(ramp(m, 0.9, 0.5), SqiFinding::InconsistentBeats);
        }
        let mut sorted = beats.to_vec();
        sorted.sort_unstable();
        let rr: Vec<usize> = sorted.windows(2).map(|w| w[1] - w[0]).collect();
        let short = rr.iter().filter(|&&r| (r as f64) < 0.25 * fs).count();
        s.add(
            ramp(short as f64 / rr.len().max(1) as f64, 0.05, 0.3),
            SqiFinding::ImplausibleBeatRate,
        );
    }
    Ok(s.finish(ECG_SQI_ALGORITHM))
}

/// ⚕ DRAFT motion score from the high-pass acceleration magnitude (g, see
/// [`crate::activity::motion_g`]): 1 at ≤ 0.02 g, 0 at ≥ 0.1 g.
fn motion_score(motion_g: f64) -> f64 {
    ramp(motion_g, 0.02, 0.1)
}

/// PPG SQI over a window of at least 4 s of raw counts (one channel, usually infrared).
/// Components (⚕ DRAFT): flat line; clipping; periodicity (largest autocorrelation peak of the
/// 0.5–5 Hz signal at a 0.3–2 s lag, 1 at ≥ 0.8, 0 at ≤ 0.4); perfusion index
/// (p95 − p5 of the pulsatile part / mean, %, 1 at ≥ 0.3 %, 0 at ≤ 0.05 %; only when the DC
/// level is positive); motion, when an accelerometer reading is given.
pub fn ppg_sqi(x: &[f64], fs: f64, motion_g: Option<f64>) -> Result<SqiResult, SignalError> {
    check_rate(fs, 20.0, 2_000.0)?;
    check_finite(x)?;
    let mut s = Scorer::new();
    if (x.len() as f64) < 4.0 * fs || robust_range(x) <= 0.0 {
        s.add(0.0, SqiFinding::FlatLine);
        return Ok(s.finish(PPG_SQI_ALGORITHM));
    }
    s.add(
        ramp(clipped_fraction(x, fs), 0.01, 0.05),
        SqiFinding::Clipping,
    );
    let ac = Cascade::bandpass(fs, 0.5, 5.0, 2)?.filtfilt(x);
    let min_lag = (0.3 * fs).round() as usize;
    let max_lag = (2.0 * fs).round() as usize;
    let rho = stats::dominant_period(&ac, min_lag, max_lag).map_or(0.0, |(_, r)| r);
    s.add(ramp(rho, 0.8, 0.4), SqiFinding::NotPeriodic);
    let dc = stats::mean(x).unwrap_or(0.0);
    if dc > 0.0 {
        let amp = percentile(&ac, 95.0).unwrap_or(0.0) - percentile(&ac, 5.0).unwrap_or(0.0);
        s.add(ramp(100.0 * amp / dc, 0.3, 0.05), SqiFinding::LowPerfusion);
    }
    if let Some(m) = motion_g {
        s.add(motion_score(m), SqiFinding::Motion);
    }
    Ok(s.finish(PPG_SQI_ALGORITHM))
}

/// Respiration (RIP) SQI over a window of at least 10 s at ≥ 5 Hz. Components (⚕ DRAFT): flat
/// line; clipping; artefact ratio (RMS above 2 Hz / RMS of 0.05–2 Hz, 1 at ≤ 0.15, 0 at
/// ≥ 0.6); motion, when given. Breathing regularity is *not* a quality component: an apnoea or
/// irregular breathing is physiology, reported by the pattern (F-SIG-03).
pub fn resp_sqi(x: &[f64], fs: f64, motion_g: Option<f64>) -> Result<SqiResult, SignalError> {
    check_rate(fs, 5.0, 2_000.0)?;
    check_finite(x)?;
    let mut s = Scorer::new();
    if (x.len() as f64) < 10.0 * fs || robust_range(x) <= 0.0 {
        s.add(0.0, SqiFinding::FlatLine);
        return Ok(s.finish(RESP_SQI_ALGORITHM));
    }
    s.add(
        ramp(clipped_fraction(x, fs), 0.01, 0.05),
        SqiFinding::Clipping,
    );
    let band = Cascade::bandpass(fs, 0.05, 2.0, 2)?.filtfilt(x);
    let high = Cascade::butter_highpass(fs, 2.0, 2)?.filtfilt(x);
    let ratio = match (rms(&high), rms(&band)) {
        (Some(h), Some(b)) if b > 0.0 => h / b,
        _ => f64::INFINITY,
    };
    s.add(ramp(ratio, 0.15, 0.6), SqiFinding::HighFrequencyNoise);
    if let Some(m) = motion_g {
        s.add(motion_score(m), SqiFinding::Motion);
    }
    Ok(s.finish(RESP_SQI_ALGORITHM))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f_sig_02_flat_and_short_signals_are_unusable() {
        let r = ecg_sqi(&[0.0; 1_000], 250.0, &[]).unwrap();
        assert_eq!(r.quality, Quality::Unusable);
        assert_eq!(r.findings, vec![SqiFinding::FlatLine]);
        assert_eq!(ecg_sqi(&[0.0; 100], 250.0, &[]).unwrap().score, 0.0);
        assert_eq!(ppg_sqi(&[5.0; 1_000], 100.0, None).unwrap().score, 0.0);
        assert_eq!(resp_sqi(&[5.0; 1_000], 25.0, None).unwrap().score, 0.0);
        assert!(ecg_sqi(&[f64::NAN; 1_000], 250.0, &[]).is_err());
    }

    #[test]
    fn f_sig_02_clipping_is_detected() {
        let x: Vec<f64> = (0..1_000)
            .map(|i| (i as f64 * 0.05).sin().clamp(-0.5, 0.5))
            .collect();
        let r = ecg_sqi(&x, 250.0, &[]).unwrap();
        assert!(r.findings.contains(&SqiFinding::Clipping), "{r:?}");
        assert_eq!(r.quality, Quality::Unusable);
    }

    #[test]
    fn f_sig_02_ppg_sine_with_dc_is_good_and_motion_rejects_it() {
        let fs = 100.0;
        let x: Vec<f64> = (0..1_000)
            .map(|i| 10_000.0 + 50.0 * (2.0 * std::f64::consts::PI * 1.2 * i as f64 / fs).sin())
            .collect();
        let good = ppg_sqi(&x, fs, Some(0.0)).unwrap();
        assert_eq!(good.quality, Quality::Good, "{good:?}");
        let moving = ppg_sqi(&x, fs, Some(0.2)).unwrap();
        assert_eq!(moving.quality, Quality::Unusable);
        assert_eq!(moving.findings, vec![SqiFinding::Motion]);
        let weak: Vec<f64> = x.iter().map(|v| v - 10_000.0 + 10_000_000.0).collect();
        let r = ppg_sqi(&weak, fs, None).unwrap();
        assert!(r.findings.contains(&SqiFinding::LowPerfusion), "{r:?}");
    }

    #[test]
    fn f_sig_02_resp_sqi_flags_high_frequency_artefact() {
        let fs = 25.0;
        let breathing: Vec<f64> = (0..1_500)
            .map(|i| (2.0 * std::f64::consts::PI * 0.25 * i as f64 / fs).sin())
            .collect();
        assert_eq!(
            resp_sqi(&breathing, fs, None).unwrap().quality,
            Quality::Good
        );
        let noisy: Vec<f64> = breathing
            .iter()
            .enumerate()
            .map(|(i, v)| v + 2.0 * (2.0 * std::f64::consts::PI * 5.0 * i as f64 / fs).sin())
            .collect();
        let r = resp_sqi(&noisy, fs, None).unwrap();
        assert!(r.findings.contains(&SqiFinding::HighFrequencyNoise));
        assert_eq!(r.quality, Quality::Unusable);
    }
}
