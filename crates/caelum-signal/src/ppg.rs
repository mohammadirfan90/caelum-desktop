//! SpO₂ and pulse rate from red/infrared PPG with motion rejection (F-SIG-04).
//!
//! - SpO₂ (`spo2-ratio-of-ratios/1`): `R = (AC_red/DC_red) / (AC_ir/DC_ir)` with DC the window
//!   mean and AC the RMS of the 0.5–5 Hz component (zero phase); SpO₂ = `a + b·R + c·R²` with the
//!   **device's own calibration** ([`Spo2Calibration`], from the sensor's Info characteristic or
//!   the device registry). There is no generic default curve: without a calibration, no SpO₂
//!   (CLAUDE.md rule 2).
//! - Pulse rate (`pr-ppg-acf/1`): 60 / dominant autocorrelation period (0.3–2 s) of the
//!   infrared 0.5–5 Hz component, parabolic refinement.
//! - Motion rejection (⚕ DRAFT, CR-013 §7): the window is rejected when the accelerometer motion
//!   exceeds [`MOTION_REJECT_G`], the PPG SQI is POOR or UNUSABLE, the red and infrared pulsatile
//!   signals correlate below [`MIN_CHANNEL_CORRELATION`], or `R` varies across 2 s sub-windows
//!   by more than [`MAX_R_CV`]. Rejected windows carry no value.

use crate::activity;
use crate::filter::Cascade;
use crate::sqi::{self, SqiResult};
use crate::stats::{self, mean, rms};
use crate::{check_finite, check_rate, Derived, Quality, SignalError};

/// SpO₂ algorithm.
pub const SPO2_ALGORITHM: &str = "spo2-ratio-of-ratios/1";
/// Pulse-rate algorithm.
pub const PULSE_RATE_ALGORITHM: &str = "pr-ppg-acf/1";

/// ⚕ DRAFT: motion above which a window is rejected, g.
pub const MOTION_REJECT_G: f64 = 0.1;
/// ⚕ DRAFT: minimum red/IR pulsatile correlation.
pub const MIN_CHANNEL_CORRELATION: f64 = 0.8;
/// ⚕ DRAFT: maximum coefficient of variation of R across sub-windows.
pub const MAX_R_CV: f64 = 0.1;
/// Shortest window, s.
pub const MIN_WINDOW_S: f64 = 4.0;

/// Device calibration curve `SpO₂ = a + b·R + c·R²` (%), valid for `r_min ≤ R ≤ r_max`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spo2Calibration {
    /// Constant term, %.
    pub a: f64,
    /// Linear term, %.
    pub b: f64,
    /// Quadratic term, %.
    pub c: f64,
    /// Lowest R the calibration covers.
    pub r_min: f64,
    /// Highest R the calibration covers.
    pub r_max: f64,
}

impl Spo2Calibration {
    /// Checks the curve: finite, a non-empty positive R range, and SpO₂ falling as R rises
    /// over the whole range (a curve that does not is not an oximeter calibration).
    pub fn validate(&self) -> Result<(), SignalError> {
        let finite = [self.a, self.b, self.c, self.r_min, self.r_max]
            .iter()
            .all(|v| v.is_finite());
        if !finite || self.r_min <= 0.0 || self.r_max <= self.r_min {
            return Err(SignalError::Invalid("SpO2 calibration range"));
        }
        let slope = |r: f64| self.b + 2.0 * self.c * r;
        if slope(self.r_min) >= 0.0 || slope(self.r_max) >= 0.0 {
            return Err(SignalError::Invalid("SpO2 calibration slope"));
        }
        Ok(())
    }
}

/// Why a window gave no SpO₂ or pulse rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rejection {
    /// Accelerometer motion above the limit.
    Motion,
    /// PPG SQI POOR or UNUSABLE.
    PoorSignal,
    /// Red and infrared pulsatile signals disagree.
    ChannelsDisagree,
    /// R not stable across the window.
    UnstableRatio,
    /// No calibration for this sensor (SpO₂ only).
    NoCalibration,
    /// R outside the calibrated range (SpO₂ only).
    OutsideCalibration,
    /// A DC level is not positive (SpO₂ only).
    InvalidDc,
}

/// Result for one window.
#[derive(Debug, Clone, PartialEq)]
pub struct PpgEstimate {
    /// SpO₂, % (→ `spo2`).
    pub spo2: Option<Derived<f64>>,
    /// Pulse rate, /min (→ `pulse_rate`).
    pub pulse_rate: Option<Derived<f64>>,
    /// Ratio of ratios, when computed.
    pub ratio: Option<f64>,
    /// Infrared PPG SQI (→ `sqi_ppg`).
    pub sqi: SqiResult,
    /// Why values are missing.
    pub rejections: Vec<Rejection>,
}

fn ratio(red_ac: &[f64], red_dc: f64, ir_ac: &[f64], ir_dc: f64) -> Option<f64> {
    let (ra, ia) = (rms(red_ac)?, rms(ir_ac)?);
    if ia <= 0.0 || red_dc <= 0.0 || ir_dc <= 0.0 {
        return None;
    }
    Some((ra / red_dc) / (ia / ir_dc))
}

/// SpO₂ and pulse rate over a window (F-SIG-04). `accel_g` (same window, any rate ≥ 5 Hz,
/// ≥ 10 s) enables accelerometer motion rejection; without it only the signal-based checks run.
pub fn spo2_and_pulse_rate(
    red: &[f64],
    ir: &[f64],
    fs: f64,
    calibration: Option<&Spo2Calibration>,
    motion_g: Option<f64>,
) -> Result<PpgEstimate, SignalError> {
    check_rate(fs, 20.0, 2_000.0)?;
    check_finite(red)?;
    check_finite(ir)?;
    if red.len() != ir.len() {
        return Err(SignalError::LengthMismatch);
    }
    if (ir.len() as f64) < MIN_WINDOW_S * fs {
        return Err(SignalError::Invalid("PPG window shorter than 4 s"));
    }
    if let Some(c) = calibration {
        c.validate()?;
    }
    let sqi = sqi::ppg_sqi(ir, fs, motion_g)?;
    let mut rejections = Vec::new();
    if motion_g.is_some_and(|m| m > MOTION_REJECT_G) {
        rejections.push(Rejection::Motion);
    }
    if !sqi.quality.usable_for_rules() {
        rejections.push(Rejection::PoorSignal);
    }
    let bp = Cascade::bandpass(fs, 0.5, 5.0, 2)?;
    let red_ac = bp.filtfilt(red);
    let ir_ac = bp.filtfilt(ir);
    if stats::correlation(&red_ac, &ir_ac).is_none_or(|r| r < MIN_CHANNEL_CORRELATION) {
        rejections.push(Rejection::ChannelsDisagree);
    }
    let pulse_ok = rejections.is_empty();

    let pulse_rate = if pulse_ok {
        let lags = ((0.2 * fs).round() as usize, (3.0 * fs).round() as usize);
        stats::dominant_period(&ir_ac, lags.0, lags.1).map(|(lag, _)| Derived {
            value: 60.0 * fs / lag,
            quality: sqi.quality,
            algorithm: PULSE_RATE_ALGORITHM,
        })
    } else {
        None
    };

    let (red_dc, ir_dc) = (mean(red).unwrap_or(0.0), mean(ir).unwrap_or(0.0));
    let r = ratio(&red_ac, red_dc, &ir_ac, ir_dc);
    let mut spo2 = None;
    if pulse_ok {
        let seg = (2.0 * fs) as usize;
        let rs: Vec<f64> = (0..)
            .map(|k| k * seg / 2)
            .take_while(|s| s + seg <= ir.len())
            .filter_map(|s| {
                let (a, b) = (s, s + seg);
                ratio(
                    red_ac.get(a..b)?,
                    mean(red.get(a..b)?)?,
                    ir_ac.get(a..b)?,
                    mean(ir.get(a..b)?)?,
                )
            })
            .collect();
        let cv = match (stats::std_dev(&rs), mean(&rs)) {
            (Some(sd), Some(m)) if m > 0.0 => sd / m,
            _ => f64::INFINITY,
        };
        if red_dc <= 0.0 || ir_dc <= 0.0 {
            rejections.push(Rejection::InvalidDc);
        } else if cv > MAX_R_CV {
            rejections.push(Rejection::UnstableRatio);
        } else {
            match (calibration, r) {
                (None, _) => rejections.push(Rejection::NoCalibration),
                (Some(c), Some(r)) if r >= c.r_min && r <= c.r_max => {
                    spo2 = Some(Derived {
                        value: c.a + c.b * r + c.c * r * r,
                        quality: sqi.quality,
                        algorithm: SPO2_ALGORITHM,
                    });
                }
                (Some(_), _) => rejections.push(Rejection::OutsideCalibration),
            }
        }
    }
    Ok(PpgEstimate {
        spo2,
        pulse_rate,
        ratio: r,
        sqi,
        rejections,
    })
}

/// Convenience: motion from a raw accelerometer window for [`spo2_and_pulse_rate`].
pub fn motion_from_accel(accel_g: &[[f64; 3]], fs: f64) -> Result<f64, SignalError> {
    activity::motion_g(accel_g, fs)
}

/// Quality used when a PPG-derived value is combined with others.
pub fn worst(a: Quality, b: Quality) -> Quality {
    a.max(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// A test calibration (only a test fixture: real curves come from the sensor).
    const CAL: Spo2Calibration = Spo2Calibration {
        a: 110.0,
        b: -25.0,
        c: 0.0,
        r_min: 0.3,
        r_max: 2.0,
    };

    fn ppg(fs: f64, seconds: f64, bpm: f64, ac_red: f64, ac_ir: f64) -> (Vec<f64>, Vec<f64>) {
        let n = (fs * seconds) as usize;
        let wave = |i: usize| {
            let t = i as f64 / fs;
            let ph = 2.0 * PI * bpm / 60.0 * t;
            ph.sin() + 0.3 * (2.0 * ph).sin()
        };
        (
            (0..n).map(|i| 50_000.0 + ac_red * wave(i)).collect(),
            (0..n).map(|i| 80_000.0 + ac_ir * wave(i)).collect(),
        )
    }

    /// F-SIG-04: R and SpO₂ follow the calibration; pulse rate within 1 /min (40–180).
    #[test]
    fn f_sig_04_spo2_and_pulse_rate() {
        for bpm in [40.0, 60.0, 100.0, 140.0, 180.0, 240.0] {
            // R = (100/50000)/(200/80000) = 0.8 → SpO₂ = 90 %.
            let (red, ir) = ppg(100.0, 8.0, bpm, 100.0, 200.0);
            let e = spo2_and_pulse_rate(&red, &ir, 100.0, Some(&CAL), Some(0.0)).unwrap();
            assert!(e.rejections.is_empty(), "{bpm}: {:?}", e.rejections);
            let s = e.spo2.unwrap();
            assert!((s.value - 90.0).abs() < 0.2, "{}", s.value);
            assert_eq!(s.algorithm, SPO2_ALGORITHM);
            let pr = e.pulse_rate.unwrap();
            assert!((pr.value - bpm).abs() < 1.0, "{bpm}: {}", pr.value);
        }
    }

    /// F-SIG-04 / rule 2: no calibration, no SpO₂ (pulse rate still computed).
    #[test]
    fn f_sig_04_no_calibration_no_spo2() {
        let (red, ir) = ppg(100.0, 8.0, 70.0, 100.0, 200.0);
        let e = spo2_and_pulse_rate(&red, &ir, 100.0, None, None).unwrap();
        assert_eq!(e.spo2, None);
        assert_eq!(e.rejections, vec![Rejection::NoCalibration]);
        assert!(e.pulse_rate.is_some());
        let bad = Spo2Calibration { b: 25.0, ..CAL };
        assert!(spo2_and_pulse_rate(&red, &ir, 100.0, Some(&bad), None).is_err());
    }

    /// F-SIG-04: motion rejects the window; values are withheld, not estimated.
    #[test]
    fn f_sig_04_motion_rejection() {
        let (red, ir) = ppg(100.0, 8.0, 70.0, 100.0, 200.0);
        let e = spo2_and_pulse_rate(&red, &ir, 100.0, Some(&CAL), Some(0.3)).unwrap();
        assert_eq!(e.spo2, None);
        assert_eq!(e.pulse_rate, None);
        assert!(e.rejections.contains(&Rejection::Motion));
        // Motion artefact that changes R mid-window (red disturbed only) is rejected too.
        let mut red2 = red.clone();
        for (i, v) in red2.iter_mut().enumerate().skip(400) {
            *v += 300.0 * (2.0 * PI * 0.9 * i as f64 / 100.0).sin();
        }
        let e = spo2_and_pulse_rate(&red2, &ir, 100.0, Some(&CAL), None).unwrap();
        assert_eq!(e.spo2, None, "{:?}", e.rejections);
    }

    #[test]
    fn invalid_inputs() {
        let (red, ir) = ppg(100.0, 8.0, 70.0, 100.0, 200.0);
        assert_eq!(
            spo2_and_pulse_rate(&red[..10], &ir, 100.0, None, None),
            Err(SignalError::LengthMismatch)
        );
        assert!(spo2_and_pulse_rate(&red[..100], &ir[..100], 100.0, None, None).is_err());
        assert_eq!(worst(Quality::Good, Quality::Poor), Quality::Poor);
    }
}
