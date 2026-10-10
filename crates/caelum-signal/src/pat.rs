//! Pulse arrival time and cuff-calibrated systolic BP trend (F-SIG-06).
//!
//! - PAT per beat (`pat-maxslope/1`): time from the ECG R peak to the steepest upstroke of the
//!   PPG pulse (maximum first derivative of the 0.5–8 Hz component, oriented so that blood
//!   volume rises), searched 50–600 ms after the R peak and before the next one. The PPG
//!   polarity is a property of the sensor (its ICD), never guessed.
//! - PAT over a window: median of ≥ 5 beats; quality falls with the beat-to-beat spread.
//! - BP trend (`bp-trend-pat-inverse/1`, → `bp_trend_systolic`): `SBP = a + b / PAT` fitted by
//!   least squares to cuff readings paired with PAT. It is a **trend**, labelled as estimated, and
//!   needs a valid calibration: ≥ 2 cuff points spanning ≥ [`MIN_PAT_SPAN_MS`] of PAT, `b > 0`,
//!   no point older than [`MAX_CALIBRATION_AGE_S`], and PAT within the calibrated range ±20 %.
//!   There is no population default slope.
//!
//! All limits ⚕ DRAFT (CR-013 §10).

use caelum_domain::time::TaiNs;

use crate::filter::Cascade;
use crate::stats::{median, percentile};
use crate::{check_finite, check_rate, Derived, Quality, SignalError};

/// Per-beat PAT algorithm.
pub const PAT_ALGORITHM: &str = "pat-maxslope/1";
/// BP trend algorithm.
pub const BP_TREND_ALGORITHM: &str = "bp-trend-pat-inverse/1";

/// ⚕ DRAFT: minimum PAT span across calibration points, ms.
pub const MIN_PAT_SPAN_MS: f64 = 10.0;
/// ⚕ DRAFT: calibration expiry, s (24 h).
pub const MAX_CALIBRATION_AGE_S: i64 = 86_400;

/// Which way the sensor's PPG moves when blood volume rises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PpgPolarity {
    /// The value rises with blood volume (absorbance-style output).
    RisesWithVolume,
    /// The value falls with blood volume (transmitted/reflected light counts).
    FallsWithVolume,
}

/// PAT of one beat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PatBeat {
    /// R-peak time, s (caller's time base).
    pub r_s: f64,
    /// Pulse arrival time, ms.
    pub pat_ms: f64,
}

/// PAT for each R peak with a valid upstroke (F-SIG-06). `r_peaks_s` and `ppg_start_s` share a
/// time base (seconds); the PPG is sampled at `fs` from `ppg_start_s`.
pub fn pulse_arrival_times(
    r_peaks_s: &[f64],
    ppg: &[f64],
    fs: f64,
    ppg_start_s: f64,
    polarity: PpgPolarity,
) -> Result<Vec<PatBeat>, SignalError> {
    check_rate(fs, 50.0, 2_000.0)?;
    check_finite(ppg)?;
    if !ppg_start_s.is_finite() || r_peaks_s.iter().any(|t| !t.is_finite()) {
        return Err(SignalError::NonFinite);
    }
    let sign = match polarity {
        PpgPolarity::RisesWithVolume => 1.0,
        PpgPolarity::FallsWithVolume => -1.0,
    };
    let y: Vec<f64> = Cascade::bandpass(fs, 0.5, 8.0, 2)?
        .filtfilt(ppg)
        .iter()
        .map(|v| sign * v)
        .collect();
    let d: Vec<f64> = y.windows(2).map(|w| (w[1] - w[0]) * fs).collect();
    let mut out = Vec::new();
    for (k, &r) in r_peaks_s.iter().enumerate() {
        let next = r_peaks_s.get(k + 1).copied().unwrap_or(f64::INFINITY);
        let lo_t = r + 0.05;
        let hi_t = (r + 0.6).min(next);
        if hi_t <= lo_t {
            continue;
        }
        let lo = ((lo_t - ppg_start_s) * fs).ceil();
        let hi = ((hi_t - ppg_start_s) * fs).floor();
        if lo < 1.0 || hi >= d.len() as f64 - 1.0 {
            continue;
        }
        let (lo, hi) = (lo as usize, hi as usize);
        let Some((i, &peak)) = d
            .get(lo..=hi)
            .and_then(|s| s.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)))
        else {
            continue;
        };
        let i = lo + i;
        // A real upstroke is a local maximum of the slope inside the window, and positive.
        let interior = i > lo && i < hi;
        if peak <= 0.0 || !interior {
            continue;
        }
        // Parabolic refinement of the slope maximum; d[i] is the slope between samples i, i+1.
        let (a, c) = (d[i - 1], d[i + 1]);
        let den = a - 2.0 * peak + c;
        let shift = if den.abs() > f64::EPSILON {
            (0.5 * (a - c) / den).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let t = ppg_start_s + (i as f64 + 0.5 + shift) / fs;
        out.push(PatBeat {
            r_s: r,
            pat_ms: (t - r) * 1_000.0,
        });
    }
    Ok(out)
}

/// Median PAT over a window (→ `pat`). Needs ≥ 5 beats; quality is the worse of `signal` and
/// the spread (IQR/median ≤ 10 % GOOD, ≤ 20 % DEGRADED, else POOR).
pub fn pat_estimate(beats: &[PatBeat], signal: Quality) -> Option<Derived<f64>> {
    if beats.len() < 5 {
        return None;
    }
    let v: Vec<f64> = beats.iter().map(|b| b.pat_ms).collect();
    let m = median(&v)?;
    let iqr = percentile(&v, 75.0)? - percentile(&v, 25.0)?;
    let spread = if m > 0.0 { iqr / m } else { f64::INFINITY };
    let own = if spread <= 0.1 {
        Quality::Good
    } else if spread <= 0.2 {
        Quality::Degraded
    } else {
        Quality::Poor
    };
    Some(Derived {
        value: m,
        quality: own.max(signal),
        algorithm: PAT_ALGORITHM,
    })
}

/// A cuff reading paired with the PAT measured at the same time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CuffPoint {
    /// When the cuff reading was taken.
    pub at: TaiNs,
    /// PAT at that time, ms.
    pub pat_ms: f64,
    /// Cuff systolic pressure, mmHg.
    pub systolic_mmhg: f64,
}

/// A fitted BP-trend calibration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BpCalibration {
    a: f64,
    b: f64,
    newest: TaiNs,
    pat_range: (f64, f64),
    /// Cuff points used.
    pub n: usize,
}

/// Why a calibration could not be fitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BpCalibrationError {
    /// Fewer than two usable points.
    #[error("at least two cuff points are needed")]
    TooFewPoints,
    /// The points do not span enough PAT to fit a slope.
    #[error("cuff points span too little PAT")]
    PatSpanTooSmall,
    /// The fitted slope is not physiological (pressure must fall as PAT rises).
    #[error("fitted slope is not physiological")]
    ImplausibleSlope,
}

impl BpCalibration {
    /// Fits `SBP = a + b / PAT` to the cuff points (non-finite or non-positive PAT ignored).
    pub fn fit(points: &[CuffPoint]) -> Result<Self, BpCalibrationError> {
        let pts: Vec<&CuffPoint> = points
            .iter()
            .filter(|p| p.pat_ms.is_finite() && p.pat_ms > 0.0 && p.systolic_mmhg.is_finite())
            .collect();
        if pts.len() < 2 {
            return Err(BpCalibrationError::TooFewPoints);
        }
        let lo = pts.iter().map(|p| p.pat_ms).fold(f64::INFINITY, f64::min);
        let hi = pts
            .iter()
            .map(|p| p.pat_ms)
            .fold(f64::NEG_INFINITY, f64::max);
        if hi - lo < MIN_PAT_SPAN_MS {
            return Err(BpCalibrationError::PatSpanTooSmall);
        }
        let n = pts.len() as f64;
        let xs: Vec<f64> = pts.iter().map(|p| 1_000.0 / p.pat_ms).collect(); // 1/s
        let mx = xs.iter().sum::<f64>() / n;
        let my = pts.iter().map(|p| p.systolic_mmhg).sum::<f64>() / n;
        let sxy: f64 = xs
            .iter()
            .zip(&pts)
            .map(|(x, p)| (x - mx) * (p.systolic_mmhg - my))
            .sum();
        let sxx: f64 = xs.iter().map(|x| (x - mx) * (x - mx)).sum();
        let b = sxy / sxx;
        if !(b.is_finite() && b > 0.0) {
            return Err(BpCalibrationError::ImplausibleSlope);
        }
        let newest = pts.iter().map(|p| p.at).max().unwrap_or(TaiNs(i64::MIN));
        Ok(Self {
            a: my - b * mx,
            b,
            newest,
            pat_range: (lo, hi),
            n: pts.len(),
        })
    }

    /// Systolic trend for a window PAT at time `now` (→ `bp_trend_systolic`). `None` when the
    /// calibration has expired or the PAT is outside the calibrated range ±20 %.
    pub fn trend(&self, pat: &Derived<f64>, now: TaiNs) -> Option<Derived<f64>> {
        let age_s = (now.0 - self.newest.0) / 1_000_000_000;
        if !(0..=MAX_CALIBRATION_AGE_S).contains(&age_s) {
            return None;
        }
        let (lo, hi) = self.pat_range;
        if !(pat.value.is_finite() && pat.value >= 0.8 * lo && pat.value <= 1.2 * hi) {
            return None;
        }
        Some(Derived {
            value: self.a + self.b * 1_000.0 / pat.value,
            quality: pat.quality.max(Quality::Degraded),
            algorithm: BP_TREND_ALGORITHM,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PPG pulses whose steepest upstroke is `pat_s` after each R peak.
    fn ppg_with_pat(
        fs: f64,
        rr: f64,
        pat_s: f64,
        n_beats: usize,
        sign: f64,
    ) -> (Vec<f64>, Vec<f64>) {
        let n = ((n_beats as f64 + 1.0) * rr * fs) as usize;
        let r: Vec<f64> = (0..n_beats).map(|k| 0.5 + k as f64 * rr).collect();
        let x = (0..n)
            .map(|i| {
                let t = i as f64 / fs;
                let v: f64 = r
                    .iter()
                    .map(|&rk| {
                        // Logistic upstroke centred at rk + pat_s, then slow decay.
                        let u = (t - rk - pat_s) / 0.03;
                        let rise = 1.0 / (1.0 + (-u).exp());
                        let decay = if t > rk + pat_s {
                            (-(t - rk - pat_s) / 0.4).exp()
                        } else {
                            1.0
                        };
                        rise * decay
                    })
                    .sum();
                sign * v
            })
            .collect();
        (r, x)
    }

    /// F-SIG-06: PAT per beat within 10 ms, for both sensor polarities.
    #[test]
    fn f_sig_06_pat_per_beat() {
        for (sign, pol) in [
            (1.0, PpgPolarity::RisesWithVolume),
            (-1.0, PpgPolarity::FallsWithVolume),
        ] {
            let (r, x) = ppg_with_pat(250.0, 0.8, 0.22, 20, sign);
            let beats = pulse_arrival_times(&r, &x, 250.0, 0.0, pol).unwrap();
            assert!(beats.len() >= 15, "{}", beats.len());
            let est = pat_estimate(&beats, Quality::Good).unwrap();
            assert!((est.value - 220.0).abs() < 10.0, "{}", est.value);
            assert_eq!(est.algorithm, PAT_ALGORITHM);
        }
        assert_eq!(pat_estimate(&[], Quality::Good), None);
    }

    fn t(s: i64) -> TaiNs {
        TaiNs(s * 1_000_000_000)
    }

    /// F-SIG-06: the trend reproduces the cuff points, expires and refuses extrapolation.
    #[test]
    fn f_sig_06_cuff_calibrated_trend() {
        // SBP = 20 + 25 000 / PAT_ms  →  (250 ms, 120), (200 ms, 145).
        let pts = [
            CuffPoint {
                at: t(0),
                pat_ms: 250.0,
                systolic_mmhg: 120.0,
            },
            CuffPoint {
                at: t(600),
                pat_ms: 200.0,
                systolic_mmhg: 145.0,
            },
        ];
        let cal = BpCalibration::fit(&pts).unwrap();
        let pat = |v| Derived {
            value: v,
            quality: Quality::Good,
            algorithm: PAT_ALGORITHM,
        };
        let s = cal.trend(&pat(225.0), t(1_000)).unwrap();
        assert!(
            (s.value - (20.0 + 25_000.0 / 225.0)).abs() < 1e-6,
            "{}",
            s.value
        );
        assert_eq!(s.algorithm, BP_TREND_ALGORITHM);
        assert_eq!(
            s.quality,
            Quality::Degraded,
            "a trend is never better than degraded"
        );
        assert_eq!(cal.trend(&pat(225.0), t(600 + 86_401)), None, "expired");
        assert_eq!(cal.trend(&pat(400.0), t(1_000)), None, "outside the range");
        assert_eq!(
            BpCalibration::fit(&pts[..1]),
            Err(BpCalibrationError::TooFewPoints)
        );
        let close = [
            pts[0],
            CuffPoint {
                pat_ms: 245.0,
                ..pts[1]
            },
        ];
        assert_eq!(
            BpCalibration::fit(&close),
            Err(BpCalibrationError::PatSpanTooSmall)
        );
        let wrong = [
            pts[0],
            CuffPoint {
                systolic_mmhg: 100.0,
                ..pts[1]
            },
        ];
        assert_eq!(
            BpCalibration::fit(&wrong),
            Err(BpCalibrationError::ImplausibleSlope)
        );
    }
}
