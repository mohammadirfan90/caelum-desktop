//! Test equipment for the caelum-signal integration tests: a deterministic synthetic ECG
//! generator, noise sources, beat matching and a reader for the MIT-BIH fixture excerpts.
//! These signals are test fixtures only; they never reach product code.
#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)] // test code

use std::f64::consts::PI;

/// SplitMix64 with Box–Muller normals: deterministic noise for reproducible tests.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn normal(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-300);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
}

/// Beat times (s) at `bpm` with ±`jitter` relative RR variation, over `seconds`.
pub fn beat_times(bpm: f64, seconds: f64, jitter: f64, rng: &mut Rng) -> Vec<f64> {
    let rr = 60.0 / bpm;
    let mut t = 0.4;
    let mut out = Vec::new();
    while t < seconds - 0.4 {
        out.push(t);
        t += rr * (1.0 + jitter * (2.0 * rng.uniform() - 1.0));
    }
    out
}

/// A Gaussian wave component.
fn g(t: f64, centre: f64, width: f64, amp: f64) -> f64 {
    let u = (t - centre) / width;
    amp * (-0.5 * u * u).exp()
}

/// Sum-of-Gaussians PQRST ECG (mV) for the given beats; QT shortens with RR (Bazett-like).
pub fn ecg(fs: f64, seconds: f64, beats: &[f64], r_amp: f64) -> Vec<f64> {
    let n = (fs * seconds) as usize;
    let mut x = vec![0.0; n];
    for (k, &b) in beats.iter().enumerate() {
        let rr = beats.get(k + 1).map_or(0.8, |n| n - b).clamp(0.3, 2.0);
        let qt = 0.4 * rr.sqrt();
        let lo = ((b - 0.3) * fs).max(0.0) as usize;
        let hi = (((b + qt + 0.2) * fs) as usize).min(n);
        for (i, v) in x.iter_mut().enumerate().take(hi).skip(lo) {
            let t = i as f64 / fs;
            *v += g(t, b - 0.16, 0.025, 0.15 * r_amp)
                + g(t, b - 0.025, 0.010, -0.10 * r_amp)
                + g(t, b, 0.010, r_amp)
                + g(t, b + 0.025, 0.010, -0.25 * r_amp)
                + g(t, b + qt - 0.08, 0.045, 0.30 * r_amp);
        }
    }
    x
}

/// Adds baseline wander: `amp` mV at 0.3 Hz plus half of it at 0.05 Hz.
pub fn add_wander(x: &mut [f64], fs: f64, amp: f64) {
    for (i, v) in x.iter_mut().enumerate() {
        let t = i as f64 / fs;
        *v += amp * (2.0 * PI * 0.3 * t).sin() + 0.5 * amp * (2.0 * PI * 0.05 * t + 1.0).sin();
    }
}

/// Adds white Gaussian noise of `rms` mV.
pub fn add_noise(x: &mut [f64], rms: f64, rng: &mut Rng) {
    for v in x.iter_mut() {
        *v += rms * rng.normal();
    }
}

/// Adds mains interference of `amp` mV at `f` Hz.
pub fn add_mains(x: &mut [f64], fs: f64, f: f64, amp: f64) {
    for (i, v) in x.iter_mut().enumerate() {
        *v += amp * (2.0 * PI * f * i as f64 / fs).sin();
    }
}

/// Beat-by-beat comparison (ANSI/AAMI EC57 style, 150 ms match window).
#[derive(Debug, Clone, Copy)]
pub struct Score {
    pub tp: usize,
    pub fn_: usize,
    pub fp: usize,
}

impl Score {
    pub fn sensitivity(&self) -> f64 {
        self.tp as f64 / (self.tp + self.fn_).max(1) as f64
    }

    pub fn ppv(&self) -> f64 {
        self.tp as f64 / (self.tp + self.fp).max(1) as f64
    }
}

/// Matches detections to reference beats (both in seconds) within `tol` s, one to one, counting
/// only reference beats and detections inside `[from, to]`.
pub fn score(reference: &[f64], detected: &[f64], tol: f64, from: f64, to: f64) -> Score {
    let r: Vec<f64> = reference
        .iter()
        .copied()
        .filter(|t| *t >= from && *t <= to)
        .collect();
    let d: Vec<f64> = detected
        .iter()
        .copied()
        .filter(|t| *t >= from && *t <= to)
        .collect();
    let mut used = vec![false; d.len()];
    let mut tp = 0;
    for &rt in &r {
        let best = d
            .iter()
            .enumerate()
            .filter(|(j, dt)| !used[*j] && (*dt - rt).abs() <= tol)
            .min_by(|a, b| (a.1 - rt).abs().total_cmp(&(b.1 - rt).abs()));
        if let Some((j, _)) = best {
            used[j] = true;
            tp += 1;
        }
    }
    Score {
        tp,
        fn_: r.len() - tp,
        fp: d.len() - tp,
    }
}

/// An MIT-BIH format-212 excerpt: channel 0 in mV, plus beat annotation times in s.
pub struct Record {
    pub fs: f64,
    pub mv: Vec<f64>,
    pub beats_s: Vec<f64>,
}

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mitdb");

/// MIT-BIH beat annotation codes (N L R a V F J A S E j / Q e).
fn is_beat(code: u16) -> bool {
    matches!(code, 1..=13 | 25 | 34 | 35 | 38 | 41)
}

/// Reads `<record>.dat` (format 212, 2 channels, excerpt starting at `start_sample` of the
/// original record) and `<record>.atr` (full annotation file).
pub fn read_record(name: &str, start_sample: u64, gain: f64, baseline: f64) -> Record {
    let dat = std::fs::read(format!("{FIXTURES}/{name}.dat")).expect("fixture .dat");
    let atr = std::fs::read(format!("{FIXTURES}/{name}.atr")).expect("fixture .atr");
    let mut mv = Vec::with_capacity(dat.len() / 3);
    for f in dat.as_chunks::<3>().0 {
        let (b0, b1) = (f[0] as i32, f[1] as i32);
        let mut s0 = b0 | ((b1 & 0x0F) << 8);
        if s0 > 2047 {
            s0 -= 4096;
        }
        mv.push((s0 as f64 - baseline) / gain);
    }
    let n = mv.len() as u64;
    let mut beats_s = Vec::new();
    let mut t: u64 = 0;
    let mut i = 0;
    while i + 1 < atr.len() {
        let w = u16::from_le_bytes([atr[i], atr[i + 1]]);
        i += 2;
        let code = w >> 10;
        let interval = u64::from(w & 0x3FF);
        match code {
            0 if interval == 0 => break,
            59 => {
                // SKIP: the next 4 bytes are a 32-bit interval (PDP-11 middle-endian).
                let hi = u16::from_le_bytes([atr[i], atr[i + 1]]) as u64;
                let lo = u16::from_le_bytes([atr[i + 2], atr[i + 3]]) as u64;
                t += (hi << 16) | lo;
                i += 4;
            }
            60..=62 => {}
            63 => {
                // AUX: `interval` bytes of text, padded to even length.
                i += (interval as usize + 1) & !1;
            }
            _ => {
                t += interval;
                if is_beat(code) && t >= start_sample && t < start_sample + n {
                    beats_s.push((t - start_sample) as f64 / 360.0);
                }
            }
        }
    }
    Record {
        fs: 360.0,
        mv,
        beats_s,
    }
}
