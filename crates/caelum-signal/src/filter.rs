//! Digital filters (F-SIG-01): second-order sections designed as Butterworth sections from the
//! bilinear transform (RBJ cookbook form), cascades of them, causal streaming state, zero-phase
//! forward-backward filtering and group delay.
//!
//! Implemented here rather than taken from a crate so that the alarm path has no third-party
//! numeric dependency (CR-013 §2).

use std::f64::consts::PI;

use crate::SignalError;

/// One second-order section `H(z) = (b0 + b1 z⁻¹ + b2 z⁻²) / (1 + a1 z⁻¹ + a2 z⁻²)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

/// Butterworth Q values of the sections of a 2nd- and 4th-order filter.
const Q2: [f64; 1] = [std::f64::consts::FRAC_1_SQRT_2];
const Q4: [f64; 2] = [0.541_196_100_146_197, 1.306_562_964_876_376_6];

impl Biquad {
    fn checked(fs: f64, fc: f64) -> Result<f64, SignalError> {
        if !(fs.is_finite() && fc.is_finite() && fs > 0.0 && fc > 0.0 && fc < fs / 2.0) {
            return Err(SignalError::Invalid("filter corner frequency"));
        }
        Ok(2.0 * PI * fc / fs)
    }

    fn normalised(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b0: b[0] / a[0],
            b1: b[1] / a[0],
            b2: b[2] / a[0],
            a1: a[1] / a[0],
            a2: a[2] / a[0],
        }
    }

    /// Low-pass section with corner `fc` and quality `q`.
    pub fn lowpass(fs: f64, fc: f64, q: f64) -> Result<Self, SignalError> {
        let w0 = Self::checked(fs, fc)?;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        Ok(Self::normalised(
            [(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0],
            [1.0 + alpha, -2.0 * c, 1.0 - alpha],
        ))
    }

    /// High-pass section with corner `fc` and quality `q`.
    pub fn highpass(fs: f64, fc: f64, q: f64) -> Result<Self, SignalError> {
        let w0 = Self::checked(fs, fc)?;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * q);
        Ok(Self::normalised(
            [(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0],
            [1.0 + alpha, -2.0 * c, 1.0 - alpha],
        ))
    }

    /// Complex frequency response at `f` Hz as `(re, im)`.
    fn response(&self, f: f64, fs: f64) -> (f64, f64) {
        let w = 2.0 * PI * f / fs;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let (nr, ni) = (
            self.b0 + self.b1 * c1 + self.b2 * c2,
            -(self.b1 * s1 + self.b2 * s2),
        );
        let (dr, di) = (
            1.0 + self.a1 * c1 + self.a2 * c2,
            -(self.a1 * s1 + self.a2 * s2),
        );
        let den = dr * dr + di * di;
        ((nr * dr + ni * di) / den, (ni * dr - nr * di) / den)
    }
}

/// A cascade of second-order sections.
#[derive(Debug, Clone, PartialEq)]
pub struct Cascade {
    sections: Vec<Biquad>,
    fs: f64,
}

/// Streaming (causal) state of a [`Cascade`] (transposed direct form II per section).
#[derive(Debug, Clone, PartialEq)]
pub struct CascadeState {
    z: Vec<[f64; 2]>,
}

impl Cascade {
    /// Butterworth low-pass of order 2 or 4.
    pub fn butter_lowpass(fs: f64, fc: f64, order: u8) -> Result<Self, SignalError> {
        let sections = Self::qs(order)?
            .iter()
            .map(|&q| Biquad::lowpass(fs, fc, q))
            .collect::<Result<_, _>>()?;
        Ok(Self { sections, fs })
    }

    /// Butterworth high-pass of order 2 or 4.
    pub fn butter_highpass(fs: f64, fc: f64, order: u8) -> Result<Self, SignalError> {
        let sections = Self::qs(order)?
            .iter()
            .map(|&q| Biquad::highpass(fs, fc, q))
            .collect::<Result<_, _>>()?;
        Ok(Self { sections, fs })
    }

    /// This cascade followed by `next` (both designed for the same sample rate).
    pub fn then(mut self, next: Cascade) -> Self {
        self.sections.extend(next.sections);
        self
    }

    /// Band-pass as a Butterworth high-pass at `lo` followed by a low-pass at `hi`, each of
    /// `order`.
    pub fn bandpass(fs: f64, lo: f64, hi: f64, order: u8) -> Result<Self, SignalError> {
        if lo >= hi {
            return Err(SignalError::Invalid("band edges"));
        }
        let mut c = Self::butter_highpass(fs, lo, order)?;
        c.sections
            .extend(Self::butter_lowpass(fs, hi, order)?.sections);
        Ok(c)
    }

    fn qs(order: u8) -> Result<&'static [f64], SignalError> {
        match order {
            2 => Ok(&Q2),
            4 => Ok(&Q4),
            _ => Err(SignalError::Invalid("filter order (2 or 4)")),
        }
    }

    /// A zeroed streaming state.
    pub fn state(&self) -> CascadeState {
        CascadeState {
            z: vec![[0.0; 2]; self.sections.len()],
        }
    }

    /// Filters one sample causally.
    pub fn step(&self, st: &mut CascadeState, x: f64) -> f64 {
        let mut v = x;
        for (s, z) in self.sections.iter().zip(st.z.iter_mut()) {
            let y = s.b0 * v + z[0];
            z[0] = s.b1 * v - s.a1 * y + z[1];
            z[1] = s.b2 * v - s.a2 * y;
            v = y;
        }
        v
    }

    /// Filters a whole slice causally from a zero state.
    pub fn filter(&self, x: &[f64]) -> Vec<f64> {
        let mut st = self.state();
        x.iter().map(|&v| self.step(&mut st, v)).collect()
    }

    /// The state the cascade would have after a long run of the constant input `x0`
    /// (the steady state used to start forward-backward filtering without a step transient).
    pub fn settled_state(&self, x0: f64) -> CascadeState {
        let mut v = x0;
        let z = self
            .sections
            .iter()
            .map(|s| {
                let dc = (s.b0 + s.b1 + s.b2) / (1.0 + s.a1 + s.a2);
                let y = dc * v;
                let z1 = s.b2 * v - s.a2 * y;
                let z0 = s.b1 * v - s.a1 * y + z1;
                v = y;
                [z0, z1]
            })
            .collect();
        CascadeState { z }
    }

    fn filter_settled(&self, x: &[f64]) -> Vec<f64> {
        let mut st = self.settled_state(x.first().copied().unwrap_or(0.0));
        x.iter().map(|&v| self.step(&mut st, v)).collect()
    }

    /// Zero-phase forward-backward filtering. The ends are extended by odd reflection (as in
    /// common practice) to limit start-up transients; the output has the input's length.
    pub fn filtfilt(&self, x: &[f64]) -> Vec<f64> {
        let n = x.len();
        if n < 2 {
            return x.to_vec();
        }
        let pad = (3 * self.sections.len() * 2 + 1)
            .max((self.fs * 1.0) as usize)
            .min(n - 1);
        let (first, last) = (x[0], x[n - 1]);
        let mut ext = Vec::with_capacity(n + 2 * pad);
        ext.extend((1..=pad).rev().map(|i| 2.0 * first - x[i]));
        ext.extend_from_slice(x);
        ext.extend((1..=pad).map(|i| 2.0 * last - x[n - 1 - i]));
        let fwd = self.filter_settled(&ext);
        let mut rev: Vec<f64> = fwd.into_iter().rev().collect();
        rev = self.filter_settled(&rev);
        rev.reverse();
        rev[pad..pad + n].to_vec()
    }

    /// Gain `|H(f)|`.
    pub fn gain(&self, f: f64) -> f64 {
        self.sections
            .iter()
            .map(|s| {
                let (re, im) = s.response(f, self.fs);
                (re * re + im * im).sqrt()
            })
            .product()
    }

    /// Group delay in samples at `f` Hz (numerical derivative of the unwrapped phase).
    pub fn group_delay(&self, f: f64) -> f64 {
        let df = (self.fs * 1e-5).max(1e-6);
        let phase = |f: f64| -> f64 {
            self.sections
                .iter()
                .map(|s| {
                    let (re, im) = s.response(f, self.fs);
                    im.atan2(re)
                })
                .sum()
        };
        let mut dphi = phase(f + df) - phase(f - df);
        while dphi > PI {
            dphi -= 2.0 * PI;
        }
        while dphi < -PI {
            dphi += 2.0 * PI;
        }
        let dw = 2.0 * PI * (2.0 * df) / self.fs;
        -dphi / dw
    }
}

/// Centred moving average of odd width `w` (shrinks at the ends).
pub fn moving_average(x: &[f64], w: usize) -> Vec<f64> {
    let n = x.len();
    let half = w / 2;
    let mut prefix = Vec::with_capacity(n + 1);
    prefix.push(0.0);
    for v in x {
        let last = prefix.last().copied().unwrap_or(0.0);
        prefix.push(last + v);
    }
    (0..n)
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(n);
            (prefix[hi] - prefix[lo]) / (hi - lo) as f64
        })
        .collect()
}

/// Removes baseline wander from an ECG: 4th-order Butterworth high-pass at 0.5 Hz (the
/// monitoring-grade corner of IEC 60601-2-27) and 2nd-order low-pass at 40 Hz, applied forward
/// and backward (zero phase) (F-SIG-01).
/// Returns the filtered ECG in the input's unit.
pub fn ecg_clean(x: &[f64], fs: f64) -> Result<Vec<f64>, SignalError> {
    crate::check_rate(fs, 100.0, 2_000.0)?;
    crate::check_finite(x)?;
    Ok(Cascade::butter_highpass(fs, 0.5, 4)?
        .then(Cascade::butter_lowpass(fs, 40.0, 2)?)
        .filtfilt(x))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f64, fs: f64, n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| (2.0 * PI * f * i as f64 / fs).sin())
            .collect()
    }

    fn rms(x: &[f64]) -> f64 {
        (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn butterworth_gain_is_minus_3_db_at_the_corner() {
        let lp = Cascade::butter_lowpass(250.0, 40.0, 2).unwrap();
        assert!((lp.gain(40.0) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
        assert!((lp.gain(0.0) - 1.0).abs() < 1e-9);
        let hp = Cascade::butter_highpass(250.0, 0.5, 4).unwrap();
        assert!((hp.gain(0.5) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!(hp.gain(0.05) < 1e-3);
    }

    #[test]
    fn invalid_designs_are_refused() {
        assert!(Cascade::butter_lowpass(100.0, 60.0, 2).is_err());
        assert!(Cascade::butter_lowpass(100.0, 10.0, 3).is_err());
        assert!(Cascade::bandpass(100.0, 10.0, 5.0, 2).is_err());
        assert!(Cascade::butter_highpass(f64::NAN, 1.0, 2).is_err());
    }

    /// F-SIG-01: baseline wander (0.2 Hz, 2 mV) is removed while a 10 Hz component passes.
    #[test]
    fn f_sig_01_ecg_clean_removes_baseline_wander() {
        let fs = 360.0;
        let n = 7_200;
        let wander: Vec<f64> = sine(0.2, fs, n).iter().map(|v| 2.0 * v).collect();
        let signal = sine(10.0, fs, n);
        let x: Vec<f64> = wander.iter().zip(&signal).map(|(a, b)| a + b).collect();
        let y = ecg_clean(&x, fs).unwrap();
        let err: Vec<f64> = y[720..n - 720]
            .iter()
            .zip(&signal[720..n - 720])
            .map(|(a, b)| a - b)
            .collect();
        assert!(rms(&err) < 0.03, "residual {}", rms(&err));
        assert!(ecg_clean(&[f64::NAN; 10], fs).is_err());
        assert!(ecg_clean(&x, 50.0).is_err());
    }

    #[test]
    fn filtfilt_is_zero_phase() {
        let fs = 100.0;
        let x = sine(2.0, fs, 2_000);
        let y = Cascade::butter_lowpass(fs, 10.0, 4).unwrap().filtfilt(&x);
        let err: Vec<f64> = y[200..1800]
            .iter()
            .zip(&x[200..1800])
            .map(|(a, b)| a - b)
            .collect();
        assert!(rms(&err) < 1e-3);
        assert_eq!(
            Cascade::butter_lowpass(fs, 10.0, 2)
                .unwrap()
                .filtfilt(&[1.0]),
            vec![1.0]
        );
    }

    #[test]
    fn group_delay_of_a_lowpass_matches_the_analytic_value() {
        // 2nd-order Butterworth low-pass: τ(0) = √2 / ω_c (analog), close for fc ≪ fs.
        let fs = 1_000.0;
        let lp = Cascade::butter_lowpass(fs, 10.0, 2).unwrap();
        let expected = std::f64::consts::SQRT_2 / (2.0 * PI * 10.0) * fs;
        assert!((lp.group_delay(0.1) - expected).abs() / expected < 0.02);
    }

    #[test]
    fn moving_average_is_centred() {
        assert_eq!(
            moving_average(&[0.0, 3.0, 0.0, 3.0], 3),
            vec![1.5, 1.0, 2.0, 1.5]
        );
    }
}
