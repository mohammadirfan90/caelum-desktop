//! RR intervals, heart rate and heart-rate variability (F-SIG-01).
//!
//! - Heart rate (`hr-median-rr/1`): 60 000 / median RR over the window; at least 3 intervals.
//!   The median tolerates one missed or extra beat.
//! - NN series (`hrv-time-malik20/1`): an RR interval is excluded when it lies outside
//!   250–3000 ms or deviates by more than 20 % from the median of up to 5 intervals on each side
//!   (Malik-style rule); successive differences only use two accepted, adjacent intervals.
//!   RMSSD and SDNN need a span ≥ [`HRV_TIME_MIN_SPAN_S`] and ≥ 80 % accepted intervals.
//! - Frequency domain (`hrv-lomb-scargle/1`): Lomb–Scargle periodogram of the NN series
//!   (no resampling), scaled so that its integral up to the mean Nyquist frequency equals the NN
//!   variance (Parseval); LF 0.04–0.15 Hz, HF 0.15–0.40 Hz (Task Force of the ESC/NASPE 1996).
//!   Needs a span ≥ [`HRV_FREQ_MIN_SPAN_S`] and a mean heart rate high enough that the mean
//!   Nyquist frequency covers the HF band.
//!
//! The catalog has `hrv_rmssd` and `hrv_sdnn`; LF, HF and LF/HF have no catalog parameter yet
//! (open item in docs/integration/M7.md).

use crate::stats::{median, std_dev};
use crate::{Derived, Quality};

/// Heart-rate algorithm.
pub const HR_ALGORITHM: &str = "hr-median-rr/1";
/// Time-domain HRV algorithm.
pub const HRV_TIME_ALGORITHM: &str = "hrv-time-malik20/1";
/// Frequency-domain HRV algorithm.
pub const HRV_FREQ_ALGORITHM: &str = "hrv-lomb-scargle/1";

/// ⚕ DRAFT (CR-013 §5): shortest span for RMSSD/SDNN, s (ultra-short-term HRV).
pub const HRV_TIME_MIN_SPAN_S: f64 = 60.0;
/// ⚕ DRAFT (CR-013 §5): shortest span for LF/HF, s (Task Force: ≥ 2 min for LF).
pub const HRV_FREQ_MIN_SPAN_S: f64 = 120.0;

const RR_MIN_MS: f64 = 250.0;
const RR_MAX_MS: f64 = 3_000.0;

/// RR intervals in ms between successive beat times (s).
pub fn rr_intervals_ms(beat_times_s: &[f64]) -> Vec<f64> {
    beat_times_s
        .windows(2)
        .map(|w| (w[1] - w[0]) * 1_000.0)
        .collect()
}

/// Heart rate from the beats in a window (F-SIG-01). `None` with fewer than three intervals.
/// Quality is the ECG quality of the window.
pub fn heart_rate(beat_times_s: &[f64], quality: Quality) -> Option<Derived<f64>> {
    let rr = rr_intervals_ms(beat_times_s);
    if rr.len() < 3 || rr.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return None;
    }
    let m = median(&rr)?;
    Some(Derived {
        value: 60_000.0 / m,
        quality,
        algorithm: HR_ALGORITHM,
    })
}

/// The cleaned NN series.
#[derive(Debug, Clone, PartialEq)]
pub struct NnSeries {
    /// Time of the beat ending each accepted interval, s.
    pub times_s: Vec<f64>,
    /// Accepted intervals, ms.
    pub nn_ms: Vec<f64>,
    /// Successive differences of accepted, adjacent intervals, ms.
    pub successive_ms: Vec<f64>,
    /// Accepted / all intervals.
    pub accepted_fraction: f64,
}

/// Artefact and ectopic rejection (see module docs).
pub fn nn_series(beat_times_s: &[f64]) -> NnSeries {
    let rr = rr_intervals_ms(beat_times_s);
    let n = rr.len();
    let mut ok = vec![false; n];
    for i in 0..n {
        let v = rr[i];
        if !(RR_MIN_MS..=RR_MAX_MS).contains(&v) {
            continue;
        }
        let lo = i.saturating_sub(5);
        let hi = (i + 6).min(n);
        let neighbours: Vec<f64> = (lo..hi)
            .filter(|&j| j != i)
            .map(|j| rr[j])
            .filter(|v| (RR_MIN_MS..=RR_MAX_MS).contains(v))
            .collect();
        ok[i] = match median(&neighbours) {
            Some(m) => (v - m).abs() <= 0.2 * m,
            None => false,
        };
    }
    let mut times_s = Vec::new();
    let mut nn_ms = Vec::new();
    let mut successive_ms = Vec::new();
    for i in 0..n {
        if ok[i] {
            times_s.push(beat_times_s[i + 1]);
            nn_ms.push(rr[i]);
            if i > 0 && ok[i - 1] {
                successive_ms.push(rr[i] - rr[i - 1]);
            }
        }
    }
    let accepted = ok.iter().filter(|&&b| b).count();
    NnSeries {
        times_s,
        nn_ms,
        successive_ms,
        accepted_fraction: if n == 0 {
            0.0
        } else {
            accepted as f64 / n as f64
        },
    }
}

fn hrv_quality(signal: Quality, accepted: f64) -> Option<Quality> {
    let own = if accepted >= 0.95 {
        Quality::Good
    } else if accepted >= 0.8 {
        Quality::Degraded
    } else {
        return None;
    };
    Some(own.max(signal))
}

fn span(beat_times_s: &[f64]) -> f64 {
    match (beat_times_s.first(), beat_times_s.last()) {
        (Some(a), Some(b)) => b - a,
        _ => 0.0,
    }
}

/// Time-domain HRV.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HrvTime {
    /// RMSSD, ms (→ `hrv_rmssd`).
    pub rmssd: Derived<f64>,
    /// SDNN, ms (→ `hrv_sdnn`).
    pub sdnn: Derived<f64>,
    /// Accepted intervals.
    pub n: usize,
}

/// RMSSD and SDNN over the beats of a window (F-SIG-01). `None` when the span is too short, too
/// many intervals were rejected, or the signal quality is POOR/UNUSABLE.
pub fn hrv_time(beat_times_s: &[f64], signal: Quality) -> Option<HrvTime> {
    if !signal.usable_for_rules() || span(beat_times_s) < HRV_TIME_MIN_SPAN_S {
        return None;
    }
    let nn = nn_series(beat_times_s);
    let quality = hrv_quality(signal, nn.accepted_fraction)?;
    if nn.successive_ms.len() < 10 {
        return None;
    }
    let rmssd = (nn.successive_ms.iter().map(|d| d * d).sum::<f64>()
        / nn.successive_ms.len() as f64)
        .sqrt();
    let sdnn = std_dev(&nn.nn_ms)?;
    let d = |value| Derived {
        value,
        quality,
        algorithm: HRV_TIME_ALGORITHM,
    };
    Some(HrvTime {
        rmssd: d(rmssd),
        sdnn: d(sdnn),
        n: nn.nn_ms.len(),
    })
}

/// Frequency-domain HRV.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HrvFrequency {
    /// LF power 0.04–0.15 Hz, ms².
    pub lf_ms2: Derived<f64>,
    /// HF power 0.15–0.40 Hz, ms².
    pub hf_ms2: Derived<f64>,
    /// LF/HF ratio.
    pub lf_hf: Derived<f64>,
}

/// Lomb–Scargle periodogram (unnormalised) at angular frequency `w`.
fn lomb(t: &[f64], y: &[f64], w: f64) -> f64 {
    let (mut s2, mut c2) = (0.0, 0.0);
    for &ti in t {
        let (s, c) = (2.0 * w * ti).sin_cos();
        s2 += s;
        c2 += c;
    }
    let tau = s2.atan2(c2) / (2.0 * w);
    let (mut yc, mut ys, mut cc, mut ss) = (0.0, 0.0, 0.0, 0.0);
    for (&ti, &yi) in t.iter().zip(y) {
        let (s, c) = (w * (ti - tau)).sin_cos();
        yc += yi * c;
        ys += yi * s;
        cc += c * c;
        ss += s * s;
    }
    let mut p = 0.0;
    if cc > 0.0 {
        p += yc * yc / cc;
    }
    if ss > 0.0 {
        p += ys * ys / ss;
    }
    0.5 * p
}

/// LF, HF and LF/HF over the beats of a window (F-SIG-01).
pub fn hrv_frequency(beat_times_s: &[f64], signal: Quality) -> Option<HrvFrequency> {
    let total_span = span(beat_times_s);
    if !signal.usable_for_rules() || total_span < HRV_FREQ_MIN_SPAN_S {
        return None;
    }
    let nn = nn_series(beat_times_s);
    let quality = hrv_quality(signal, nn.accepted_fraction)?;
    let n = nn.nn_ms.len();
    if n < 30 {
        return None;
    }
    let mean = nn.nn_ms.iter().sum::<f64>() / n as f64;
    let y: Vec<f64> = nn.nn_ms.iter().map(|v| v - mean).collect();
    let var = y.iter().map(|v| v * v).sum::<f64>() / n as f64;
    let nyquist = 0.5 / (mean / 1_000.0);
    if nyquist < 0.4 || var <= 0.0 {
        return None;
    }
    let df = 1.0 / (4.0 * total_span);
    let k_max = (nyquist / df).floor() as usize;
    let t0 = nn.times_s.first().copied().unwrap_or(0.0);
    let t: Vec<f64> = nn.times_s.iter().map(|v| v - t0).collect();
    let mut total = 0.0;
    let (mut lf, mut hf) = (0.0, 0.0);
    for k in 1..=k_max {
        let f = k as f64 * df;
        let p = lomb(&t, &y, 2.0 * std::f64::consts::PI * f) * df;
        total += p;
        if f > 0.04 && f <= 0.15 {
            lf += p;
        } else if f > 0.15 && f <= 0.4 {
            hf += p;
        }
    }
    if total <= 0.0 || hf <= 0.0 {
        return None;
    }
    let scale = var / total;
    let d = |value| Derived {
        value,
        quality,
        algorithm: HRV_FREQ_ALGORITHM,
    };
    Some(HrvFrequency {
        lf_ms2: d(lf * scale),
        hf_ms2: d(hf * scale),
        lf_hf: d(lf / hf),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Beat times whose RR follows `rr(t)` ms.
    fn beats(duration_s: f64, rr: impl Fn(f64) -> f64) -> Vec<f64> {
        let mut t = 0.0;
        let mut out = vec![0.0];
        while t < duration_s {
            t += rr(t) / 1_000.0;
            out.push(t);
        }
        out
    }

    /// F-SIG-01: heart rate is 60000/median RR; one missed beat does not move it.
    #[test]
    fn f_sig_01_heart_rate_from_median_rr() {
        let b = beats(10.0, |_| 750.0);
        let hr = heart_rate(&b, Quality::Good).unwrap();
        assert!((hr.value - 80.0).abs() < 1e-9);
        assert_eq!(hr.algorithm, HR_ALGORITHM);
        let mut missed = b.clone();
        missed.remove(5);
        assert!((heart_rate(&missed, Quality::Good).unwrap().value - 80.0).abs() < 1e-9);
        assert_eq!(heart_rate(&[0.0, 1.0, 2.0], Quality::Good), None);
    }

    /// F-SIG-01: RMSSD and SDNN of an alternating series are exact.
    #[test]
    fn f_sig_01_rmssd_and_sdnn() {
        let mut t = 0.0;
        let mut b = vec![0.0];
        for i in 0..100 {
            t += if i % 2 == 0 { 0.82 } else { 0.78 };
            b.push(t);
        }
        let h = hrv_time(&b, Quality::Good).unwrap();
        assert!((h.rmssd.value - 40.0).abs() < 1e-6, "{}", h.rmssd.value);
        assert!((h.sdnn.value - 20.1).abs() < 0.2, "{}", h.sdnn.value);
        assert_eq!(h.rmssd.quality, Quality::Good);
        assert_eq!(hrv_time(&b[..50], Quality::Good), None, "span below 60 s");
        assert_eq!(hrv_time(&b, Quality::Poor), None);
    }

    /// F-SIG-01: an ectopic beat is excluded and does not inflate RMSSD.
    #[test]
    fn f_sig_01_ectopic_beats_are_excluded() {
        let mut b = beats(90.0, |_| 800.0);
        // Premature beat 300 ms early with compensatory pause.
        b[40] -= 0.3;
        let nn = nn_series(&b);
        assert_eq!(nn.nn_ms.len(), b.len() - 3);
        let h = hrv_time(&b, Quality::Good).unwrap();
        assert!(h.rmssd.value < 1e-6);
        assert_eq!(h.rmssd.quality, Quality::Good);
    }

    /// F-SIG-01: LF and HF powers of a two-tone RR series match A²/2 within 15 %.
    #[test]
    fn f_sig_01_lomb_scargle_band_powers() {
        let b = beats(300.0, |t| {
            800.0 + 30.0 * (2.0 * PI * 0.1 * t).sin() + 20.0 * (2.0 * PI * 0.25 * t).sin()
        });
        let f = hrv_frequency(&b, Quality::Good).unwrap();
        assert!(
            (f.lf_ms2.value - 450.0).abs() / 450.0 < 0.15,
            "LF {}",
            f.lf_ms2.value
        );
        assert!(
            (f.hf_ms2.value - 200.0).abs() / 200.0 < 0.15,
            "HF {}",
            f.hf_ms2.value
        );
        assert!(
            (f.lf_hf.value - 2.25).abs() / 2.25 < 0.2,
            "{}",
            f.lf_hf.value
        );
        assert_eq!(f.lf_hf.algorithm, HRV_FREQ_ALGORITHM);
        assert_eq!(
            hrv_frequency(&b[..100], Quality::Good),
            None,
            "span below 120 s"
        );
        // 40 bpm: the HF band is above the mean Nyquist frequency.
        assert_eq!(
            hrv_frequency(&beats(300.0, |_| 1_500.0), Quality::Good),
            None
        );
    }
}
