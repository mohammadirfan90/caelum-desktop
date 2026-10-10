//! Activity level and sleep/wake (F-SIG-07), microgravity-safe: no posture and no 1 g are
//! assumed.
//!
//! - Motion (`motion-hp-accel/1`): each accelerometer axis is high-pass filtered at 0.25 Hz
//!   (zero phase), which removes gravity on Earth, the absence of it in orbit, and any static
//!   orientation alike; the vector magnitude is averaged over the window (g). Nothing depends on
//!   the direction of "down".
//! - Activity level (`activity-motion/1`, → `activity_level`, 0–1): motion / full scale
//!   [`ACTIVITY_FULL_SCALE_G`], capped at 1. With the `hr.warning.high` at-rest limit of 0.3,
//!   "at rest" means mean motion ≤ 0.15 g.
//! - Sleep/wake (`sleep-wake-actigraphy-hr/1`): per 60 s epoch. An epoch is quiet when its motion
//!   is below [`QUIET_MOTION_G`]. It is sleep when it and at least 9 of the 10 epochs before it
//!   are quiet and, if a resting heart-rate baseline is known, its heart rate is not above
//!   `median + 2 × 1.4826 × MAD`. A missing epoch or too little history gives `None` (unknown,
//!   SR-018), never a guess.
//!
//! All constants ⚕ DRAFT (CR-013 §8); validated on Earth actigraphy only.

use crate::filter::Cascade;
use crate::{check_rate, Derived, Quality, SignalError};

/// Motion algorithm.
pub const MOTION_ALGORITHM: &str = "motion-hp-accel/1";
/// Activity-level algorithm.
pub const ACTIVITY_ALGORITHM: &str = "activity-motion/1";
/// Sleep/wake algorithm.
pub const SLEEP_WAKE_ALGORITHM: &str = "sleep-wake-actigraphy-hr/1";

/// ⚕ DRAFT: motion that maps to activity level 1, g.
pub const ACTIVITY_FULL_SCALE_G: f64 = 0.5;
/// ⚕ DRAFT: an epoch with less motion than this is quiet, g.
pub const QUIET_MOTION_G: f64 = 0.01;
/// Shortest activity window, s.
pub const MIN_WINDOW_S: f64 = 10.0;

/// Mean high-pass acceleration magnitude over a window, g (≥ [`MIN_WINDOW_S`], 5–2000 Hz).
pub fn motion_g(accel_g: &[[f64; 3]], fs: f64) -> Result<f64, SignalError> {
    check_rate(fs, 5.0, 2_000.0)?;
    if accel_g.iter().flatten().any(|v| !v.is_finite()) {
        return Err(SignalError::NonFinite);
    }
    if (accel_g.len() as f64) < MIN_WINDOW_S * fs {
        return Err(SignalError::Invalid("activity window shorter than 10 s"));
    }
    let hp = Cascade::butter_highpass(fs, 0.25, 2)?;
    let axis = |k: usize| hp.filtfilt(&accel_g.iter().map(|a| a[k]).collect::<Vec<_>>());
    let (x, y, z) = (axis(0), axis(1), axis(2));
    let sum: f64 = x
        .iter()
        .zip(&y)
        .zip(&z)
        .map(|((x, y), z)| (x * x + y * y + z * z).sqrt())
        .sum();
    Ok(sum / accel_g.len() as f64)
}

/// Activity level 0–1 over the window (F-SIG-07, → `activity_level`).
pub fn activity_level(accel_g: &[[f64; 3]], fs: f64) -> Result<Derived<f64>, SignalError> {
    let m = motion_g(accel_g, fs)?;
    Ok(Derived {
        value: (m / ACTIVITY_FULL_SCALE_G).min(1.0),
        quality: Quality::Good,
        algorithm: ACTIVITY_ALGORITHM,
    })
}

/// One 60 s epoch of features.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Epoch {
    /// Motion of the epoch, g; `None` when the IMU sent nothing usable.
    pub motion_g: Option<f64>,
    /// Mean heart rate of the epoch, /min, if known.
    pub heart_rate: Option<f64>,
}

/// Sleep or wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SleepWake {
    /// Awake.
    Wake,
    /// Asleep (stage not resolved without EEG).
    Sleep,
}

/// Resting heart-rate baseline (median and unscaled MAD, from [`crate::baseline`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HrBaseline {
    /// Median, /min.
    pub median: f64,
    /// Median absolute deviation, /min.
    pub mad: f64,
}

/// Sleep/wake per epoch (F-SIG-07); see the module docs. Output has one entry per epoch.
pub fn sleep_wake(
    epochs: &[Epoch],
    hr_rest: Option<HrBaseline>,
) -> Vec<Option<Derived<SleepWake>>> {
    let d = |value| {
        Some(Derived {
            value,
            quality: Quality::Good,
            algorithm: SLEEP_WAKE_ALGORITHM,
        })
    };
    (0..epochs.len())
        .map(|i| {
            let m = epochs[i].motion_g?;
            if m >= QUIET_MOTION_G {
                return d(SleepWake::Wake);
            }
            if i < 10 {
                return None;
            }
            let mut quiet = 0;
            for e in &epochs[i - 10..i] {
                match e.motion_g {
                    Some(v) if v < QUIET_MOTION_G => quiet += 1,
                    Some(_) => {}
                    None => return None,
                }
            }
            if quiet < 9 {
                return d(SleepWake::Wake);
            }
            if let (Some(b), Some(hr)) = (hr_rest, epochs[i].heart_rate) {
                if hr > b.median + 2.0 * 1.4826 * b.mad {
                    return d(SleepWake::Wake);
                }
            }
            d(SleepWake::Sleep)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn accel(fs: f64, seconds: f64, static_g: [f64; 3], amp: f64) -> Vec<[f64; 3]> {
        (0..(fs * seconds) as usize)
            .map(|i| {
                let s = amp * (2.0 * PI * 2.0 * i as f64 / fs).sin();
                [static_g[0] + s, static_g[1], static_g[2] + 0.5 * s]
            })
            .collect()
    }

    /// F-SIG-07: the same movement gives the same activity on Earth (any posture) and in
    /// microgravity: static acceleration is ignored.
    #[test]
    fn f_sig_07_activity_is_independent_of_gravity_and_posture() {
        let fs = 50.0;
        let a = activity_level(&accel(fs, 30.0, [0.0, 0.0, 1.0], 0.2), fs).unwrap();
        let b = activity_level(&accel(fs, 30.0, [0.7, 0.7, 0.0], 0.2), fs).unwrap();
        let c = activity_level(&accel(fs, 30.0, [0.0, 0.0, 0.0], 0.2), fs).unwrap();
        assert!((a.value - b.value).abs() < 1e-6 && (a.value - c.value).abs() < 1e-6);
        assert!(a.value > 0.25, "{}", a.value);
        let rest = activity_level(&accel(fs, 30.0, [0.0, 0.0, 1.0], 0.0), fs).unwrap();
        assert!(rest.value < 1e-9);
        assert_eq!(rest.algorithm, ACTIVITY_ALGORITHM);
        assert!(activity_level(&accel(fs, 5.0, [0.0; 3], 0.0), fs).is_err());
    }

    /// F-SIG-07 / SR-018: sleep needs a quiet history; a missing epoch is unknown, not a guess.
    #[test]
    fn f_sig_07_sleep_wake_with_unknown_context() {
        let quiet = Epoch {
            motion_g: Some(0.002),
            heart_rate: Some(52.0),
        };
        let mut e = vec![quiet; 15];
        e[3] = Epoch {
            motion_g: Some(0.2),
            heart_rate: Some(80.0),
        };
        e[13].motion_g = None;
        let out = sleep_wake(&e, None);
        assert_eq!(out[0], None, "no history yet");
        assert_eq!(out[3].unwrap().value, SleepWake::Wake);
        assert_eq!(out[12].unwrap().value, SleepWake::Sleep);
        assert_eq!(out[13], None, "no motion data");
        assert_eq!(out[14], None, "history has a gap");
        let base = HrBaseline {
            median: 50.0,
            mad: 2.0,
        };
        let mut high = e.clone();
        high[12].heart_rate = Some(70.0);
        assert_eq!(
            sleep_wake(&high, Some(base))[12].unwrap().value,
            SleepWake::Wake
        );
    }
}
