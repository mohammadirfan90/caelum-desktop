//! Respiration rate and pattern from respiratory inductance plethysmography (RIP) and from
//! ECG-derived respiration (EDR) (F-SIG-03).
//!
//! Breath detection (`resp-count-adv/1`) is the "advanced counting" method of Schäfer A,
//! Kratky KW, Ann Biomed Eng 2008;36:476–85: band-pass 0.05–1.5 Hz (zero phase), find
//! alternating extrema, then repeatedly remove the extremum pair with the smallest vertical
//! distance while it is below 0.3 × the third quartile of all distances. Each remaining maximum
//! is one breath; the rate is 60 / median breath interval.
//!
//! EDR (`resp-edr-ramp/1`) uses the R-amplitude modulation: the beat-by-beat R amplitudes are
//! interpolated at 4 Hz and counted the same way.
//!
//! Pattern (⚕ DRAFT, CR-013 §6): `Pause` when the window contains ≥ 10 s without a breath,
//! `Irregular` when the breath-interval coefficient of variation exceeds 0.3, else `Regular`.
//! An apnoea therefore shows as a pause pattern with no rate, never as a made-up rate.

use crate::filter::Cascade;
use crate::sqi::{self, SqiResult};
use crate::stats::{median, percentile, resample_uniform, std_dev};
use crate::{check_finite, check_rate, Derived, Quality, SignalError};

/// RIP respiration algorithm.
pub const RIP_ALGORITHM: &str = "resp-count-adv/1";
/// ECG-derived respiration algorithm.
pub const EDR_ALGORITHM: &str = "resp-edr-ramp/1";

/// ⚕ DRAFT: a gap without breaths at least this long is a pause, s.
pub const PAUSE_S: f64 = 10.0;
/// ⚕ DRAFT: coefficient of variation of breath intervals above which breathing is irregular.
pub const IRREGULAR_CV: f64 = 0.3;
/// Shortest analysis window, s.
pub const MIN_WINDOW_S: f64 = 20.0;

const EDR_FS: f64 = 4.0;

/// Breathing pattern (F-SIG-03).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BreathingPattern {
    /// Regular breathing.
    Regular,
    /// Irregular breath intervals.
    Irregular,
    /// At least [`PAUSE_S`] without a detected breath (apnoea or hypopnoea).
    Pause,
}

/// Result for one window.
#[derive(Debug, Clone, PartialEq)]
pub struct RespirationEstimate {
    /// Respiration rate, /min (→ `respiration_rate`); `None` with fewer than two breath
    /// intervals or an unusable signal.
    pub rate: Option<Derived<f64>>,
    /// Median breath amplitude in the input's (uncalibrated) unit; `None` when no breath.
    pub amplitude: Option<f64>,
    /// Pattern; `None` when the signal is unusable.
    pub pattern: Option<Derived<BreathingPattern>>,
    /// Breath times (maxima) relative to the window start, s.
    pub breaths_s: Vec<f64>,
    /// Longest interval without a breath, including the window edges, s.
    pub longest_gap_s: f64,
    /// Signal quality of the respiration source.
    pub sqi: SqiResult,
}

/// Sub-sample position of the extremum at `i` (parabola through its neighbours).
fn refine(y: &[f64], i: usize) -> f64 {
    let (Some(&a), Some(&b), Some(&c)) = (
        i.checked_sub(1).and_then(|k| y.get(k)),
        y.get(i),
        y.get(i + 1),
    ) else {
        return i as f64;
    };
    let den = a - 2.0 * b + c;
    if den.abs() > f64::EPSILON {
        i as f64 + (0.5 * (a - c) / den).clamp(-0.5, 0.5)
    } else {
        i as f64
    }
}

/// Alternating extrema indices and kinds (true = maximum).
fn extrema(y: &[f64]) -> Vec<(usize, bool)> {
    let mut out: Vec<(usize, bool)> = Vec::new();
    for i in 1..y.len().saturating_sub(1) {
        let is_max = y[i] > y[i - 1] && y[i] >= y[i + 1];
        let is_min = y[i] < y[i - 1] && y[i] <= y[i + 1];
        if !(is_max || is_min) {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.1 == is_max => {
                // Same kind twice: keep the more extreme one.
                let better = if is_max {
                    y[i] > y[last.0]
                } else {
                    y[i] < y[last.0]
                };
                if better {
                    last.0 = i;
                }
            }
            _ => out.push((i, is_max)),
        }
    }
    out
}

/// Advanced counting: removes small extremum pairs; returns the maxima kept and the median
/// peak-to-trough amplitude.
fn count_breaths(y: &[f64]) -> (Vec<usize>, Option<f64>) {
    let mut ex = extrema(y);
    let dist = |ex: &[(usize, bool)]| -> Vec<f64> {
        ex.windows(2)
            .map(|w| (y[w[1].0] - y[w[0].0]).abs())
            .collect()
    };
    let d = dist(&ex);
    let Some(q3) = percentile(&d, 75.0) else {
        return (Vec::new(), None);
    };
    let thr = 0.3 * q3;
    loop {
        let d = dist(&ex);
        let Some((k, &smallest)) = d.iter().enumerate().min_by(|a, b| a.1.total_cmp(b.1)) else {
            break;
        };
        if smallest >= thr {
            break;
        }
        // Remove the pair k, k+1; neighbours stay alternating.
        ex.drain(k..(k + 2).min(ex.len()));
        // Merge same-kind neighbours produced at the junction.
        let mut merged: Vec<(usize, bool)> = Vec::with_capacity(ex.len());
        for e in ex {
            match merged.last_mut() {
                Some(last) if last.1 == e.1 => {
                    let better = if e.1 {
                        y[e.0] > y[last.0]
                    } else {
                        y[e.0] < y[last.0]
                    };
                    if better {
                        *last = e;
                    }
                }
                _ => merged.push(e),
            }
        }
        ex = merged;
    }
    let amp = median(&dist(&ex));
    let maxima = ex.iter().filter(|e| e.1).map(|e| e.0).collect();
    (maxima, amp)
}

fn estimate(
    y: &[f64],
    fs: f64,
    sqi: SqiResult,
    algorithm: &'static str,
    quality_cap: Quality,
) -> Result<RespirationEstimate, SignalError> {
    let duration = y.len() as f64 / fs;
    let filtered = Cascade::bandpass(fs, 0.05, 1.5, 2)?.filtfilt(y);
    let (maxima, amplitude) = count_breaths(&filtered);
    let breaths_s: Vec<f64> = maxima.iter().map(|&i| refine(&filtered, i) / fs).collect();
    let intervals: Vec<f64> = breaths_s.windows(2).map(|w| w[1] - w[0]).collect();
    let mut gaps = intervals.clone();
    gaps.push(breaths_s.first().copied().unwrap_or(duration));
    gaps.push(duration - breaths_s.last().copied().unwrap_or(0.0));
    let longest_gap_s = gaps.iter().fold(0.0_f64, |a, &b| a.max(b));
    let quality = sqi.quality.max(quality_cap);
    if quality == Quality::Unusable {
        return Ok(RespirationEstimate {
            rate: None,
            amplitude: None,
            pattern: None,
            breaths_s,
            longest_gap_s,
            sqi,
        });
    }
    let pattern = if longest_gap_s >= PAUSE_S {
        BreathingPattern::Pause
    } else {
        match (std_dev(&intervals), median(&intervals)) {
            (Some(sd), Some(m)) if m > 0.0 && sd / m > IRREGULAR_CV => BreathingPattern::Irregular,
            _ => BreathingPattern::Regular,
        }
    };
    let rate = if intervals.len() >= 2 {
        median(&intervals).filter(|m| *m > 0.0).map(|m| Derived {
            value: 60.0 / m,
            quality,
            algorithm,
        })
    } else {
        None
    };
    Ok(RespirationEstimate {
        rate,
        amplitude: if maxima.is_empty() { None } else { amplitude },
        pattern: Some(Derived {
            value: pattern,
            quality,
            algorithm,
        }),
        breaths_s,
        longest_gap_s,
        sqi,
    })
}

/// Respiration from a RIP channel (F-SIG-03): window ≥ [`MIN_WINDOW_S`] (60 s recommended so
/// rates down to 4/min give two intervals), any rate ≥ 5 Hz. `motion_g` from the same garment's
/// IMU, when available, feeds the SQI.
pub fn respiration_from_rip(
    x: &[f64],
    fs: f64,
    motion_g: Option<f64>,
) -> Result<RespirationEstimate, SignalError> {
    check_rate(fs, 5.0, 2_000.0)?;
    check_finite(x)?;
    if (x.len() as f64) < MIN_WINDOW_S * fs {
        return Err(SignalError::Invalid("respiration window shorter than 20 s"));
    }
    let sqi = sqi::resp_sqi(x, fs, motion_g)?;
    estimate(x, fs, sqi, RIP_ALGORITHM, Quality::Good)
}

/// One beat for EDR: time in s from the window start and R amplitude in mV.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdrBeat {
    /// Beat time, s from the window start.
    pub t_s: f64,
    /// R amplitude, mV ([`crate::ecg::QrsEvent::amplitude_mv`]).
    pub amplitude_mv: f64,
}

/// Respiration from ECG R-amplitude modulation over a window of `duration_s` (F-SIG-03).
/// `ecg_quality` is the worst ECG quality over the window; an RR gap over 3 s caps the result at
/// POOR (the amplitude series is interpolated across it).
pub fn respiration_from_edr(
    beats: &[EdrBeat],
    duration_s: f64,
    ecg_quality: Quality,
) -> Result<RespirationEstimate, SignalError> {
    if !(duration_s.is_finite() && duration_s >= MIN_WINDOW_S) {
        return Err(SignalError::Invalid("respiration window shorter than 20 s"));
    }
    if beats
        .iter()
        .any(|b| !(b.t_s.is_finite() && b.amplitude_mv.is_finite()))
    {
        return Err(SignalError::NonFinite);
    }
    if beats.windows(2).any(|w| w[1].t_s <= w[0].t_s) {
        return Err(SignalError::Invalid("beat times not increasing"));
    }
    let t: Vec<f64> = beats.iter().map(|b| b.t_s).collect();
    let v: Vec<f64> = beats.iter().map(|b| b.amplitude_mv).collect();
    let series = resample_uniform(&t, &v, EDR_FS);
    let max_rr = t.windows(2).map(|w| w[1] - w[0]).fold(0.0_f64, f64::max);
    let coverage_gap =
        t.first().copied().unwrap_or(duration_s) + (duration_s - t.last().copied().unwrap_or(0.0));
    let cap = if beats.len() < 10 || (series.len() as f64) < 0.8 * duration_s * EDR_FS {
        Quality::Unusable
    } else if max_rr > 3.0 || coverage_gap > 3.0 {
        Quality::Poor
    } else {
        Quality::Good
    };
    // EDR has no index of its own: the score is NaN (not a number, never a made-up one) and the
    // quality comes from the ECG and the beat coverage.
    let sqi = SqiResult {
        score: f64::NAN,
        quality: ecg_quality.max(cap),
        algorithm: EDR_ALGORITHM,
        findings: Vec::new(),
    };
    if cap == Quality::Unusable {
        return Ok(RespirationEstimate {
            rate: None,
            amplitude: None,
            pattern: None,
            breaths_s: Vec::new(),
            longest_gap_s: duration_s,
            sqi,
        });
    }
    let offset = t.first().copied().unwrap_or(0.0);
    let mut est = estimate(&series, EDR_FS, sqi, EDR_ALGORITHM, ecg_quality.max(cap))?;
    for b in &mut est.breaths_s {
        *b += offset;
    }
    Ok(est)
}

/// Chooses the respiration source (F-SIG-03): RIP when its rate is usable for rules, else EDR
/// when usable, else whichever has a rate (so its POOR quality pauses the rules), else none.
pub fn fuse<'a>(
    rip: Option<&'a RespirationEstimate>,
    edr: Option<&'a RespirationEstimate>,
) -> Option<&'a RespirationEstimate> {
    let usable = |e: &&RespirationEstimate| {
        e.rate
            .as_ref()
            .is_some_and(|r| r.quality.usable_for_rules())
    };
    rip.filter(usable)
        .or(edr.filter(usable))
        .or(rip.filter(|e| e.rate.is_some()))
        .or(edr.filter(|e| e.rate.is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn breathing(fs: f64, seconds: f64, per_min: f64) -> Vec<f64> {
        (0..(fs * seconds) as usize)
            .map(|i| 1_000.0 + 200.0 * (2.0 * PI * per_min / 60.0 * i as f64 / fs).sin())
            .collect()
    }

    /// F-SIG-03: RIP rate within 0.5 /min from 4 to 40 /min.
    #[test]
    fn f_sig_03_rip_rate_across_the_range() {
        for rate in [4.0, 6.0, 12.0, 20.0, 30.0, 40.0] {
            let e = respiration_from_rip(&breathing(25.0, 60.0, rate), 25.0, None).unwrap();
            let r = e.rate.unwrap();
            assert!((r.value - rate).abs() < 0.5, "{rate}: {}", r.value);
            assert_eq!(r.algorithm, RIP_ALGORITHM);
            assert_eq!(r.quality, Quality::Good);
        }
    }

    /// F-SIG-03: small ripples (cardiogenic oscillation) are not counted as breaths.
    #[test]
    fn f_sig_03_small_ripples_are_not_breaths() {
        let fs = 25.0;
        let x: Vec<f64> = breathing(fs, 60.0, 15.0)
            .iter()
            .enumerate()
            .map(|(i, v)| v + 15.0 * (2.0 * PI * 1.2 * i as f64 / fs).sin())
            .collect();
        let r = respiration_from_rip(&x, fs, None).unwrap().rate.unwrap();
        assert!((r.value - 15.0).abs() < 0.5, "{}", r.value);
    }

    /// F-SIG-03: a 20 s apnoea is reported as a pause pattern.
    #[test]
    fn f_sig_03_apnoea_is_a_pause() {
        let fs = 25.0;
        let mut x = breathing(fs, 40.0, 15.0);
        x.extend(
            std::iter::repeat_n(1_000.0, (20.0 * fs) as usize)
                .enumerate()
                .map(|(i, v)| v + 0.5 * (2.0 * PI * 1.1 * i as f64 / fs).sin()),
        );
        let e = respiration_from_rip(&x, fs, None).unwrap();
        assert_eq!(e.pattern.unwrap().value, BreathingPattern::Pause);
        assert!(e.longest_gap_s >= 18.0, "{}", e.longest_gap_s);
    }

    #[test]
    fn f_sig_03_short_or_flat_input() {
        assert!(respiration_from_rip(&[1.0; 100], 25.0, None).is_err());
        let flat = respiration_from_rip(&[1.0; 1_500], 25.0, None).unwrap();
        assert_eq!(flat.rate, None);
        assert_eq!(flat.pattern, None);
        assert_eq!(flat.sqi.quality, Quality::Unusable);
    }

    /// F-SIG-03: EDR recovers the rate from R-amplitude modulation.
    #[test]
    fn f_sig_03_edr_rate() {
        let beats: Vec<EdrBeat> = (0..75)
            .map(|k| {
                let t = 0.4 + k as f64 * 0.8;
                EdrBeat {
                    t_s: t,
                    amplitude_mv: 1.0 + 0.1 * (2.0 * PI * 0.2 * t).sin(),
                }
            })
            .collect();
        let e = respiration_from_edr(&beats, 60.0, Quality::Good).unwrap();
        let r = e.rate.unwrap();
        assert!((r.value - 12.0).abs() < 0.6, "{}", r.value);
        assert_eq!(r.algorithm, EDR_ALGORITHM);
        // Too few beats: no rate.
        let few = respiration_from_edr(&beats[..5], 60.0, Quality::Good).unwrap();
        assert_eq!(few.rate, None);
        // Fusion prefers a usable RIP, falls back to EDR.
        let rip_bad = respiration_from_rip(&[1.0; 1_500], 25.0, None).unwrap();
        assert!(std::ptr::eq(fuse(Some(&rip_bad), Some(&e)).unwrap(), &e));
        assert!(fuse(Some(&rip_bad), None).is_none());
    }
}
