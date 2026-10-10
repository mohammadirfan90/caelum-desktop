//! ECG QRS detection and the streaming ECG processor (F-SIG-01, F-SIG-02).
//!
//! # QRS detection: Pan–Tompkins, causal and streaming (`qrs-pan-tompkins/1`)
//! Pan J, Tompkins WJ. A real-time QRS detection algorithm. IEEE Trans Biomed Eng 1985;32:230–6.
//! 1. Band-pass 5–15 Hz (2nd-order Butterworth high- and low-pass, causal).
//! 2. Five-point derivative, squaring, 150 ms moving-window integration (MWI).
//! 3. MWI peaks with a 200 ms refractory period (the larger of two close peaks wins).
//! 4. Adaptive thresholds: `SPKI`/`NPKI` running estimates (1/8 update), threshold
//!    `NPKI + ¼(SPKI − NPKI)`; T-wave discrimination (a peak within 360 ms of the last QRS with
//!    less than half its maximal slope is a T wave); search-back at half the threshold when no
//!    QRS is found for 1.66 × the mean of the last 8 RR intervals.
//! 5. Learning phase: the thresholds are initialised from the first 2 s that contain a peak above
//!    the amplitude floor (SPKI = ⅓ of the largest MWI peak, NPKI = ½ of the mean MWI). Peaks
//!    seen while learning are replayed once the thresholds exist, so no beat is lost.
//! 6. The R peak is located on the 0.5–40 Hz ECG within ±125 ms of the QRS position implied by
//!    the filter delays.
//!
//! CAELUM addition (CR-013 §4, safety): a peak whose 5–15 Hz amplitude is below
//! [`AMPLITUDE_FLOOR_MV`] is never a QRS, whatever the thresholds. Thresholds never decay during
//! a pause. Together these keep the isoelectric line of an asystole from being counted as beats,
//! which would hide `asystole.critical` (H-01). Input is in millivolts.
//!
//! A detector instance covers one contiguous stream. After a gap, dropped packets or a non-finite
//! sample the caller starts a new instance (`push` resets itself and returns an error on a
//! non-finite sample).

use std::collections::VecDeque;

use crate::filter::Cascade;
use crate::hrv;
use crate::sqi::{self, SqiResult};
use crate::{check_rate, Derived, Quality, SignalError};

/// Algorithm identifier of the QRS detector.
pub const QRS_ALGORITHM: &str = "qrs-pan-tompkins/1";

/// ⚕ DRAFT (CR-013 §4): minimum 5–15 Hz band amplitude of a QRS complex, mV.
pub const AMPLITUDE_FLOOR_MV: f64 = 0.04;

/// Lowest and highest supported ECG sample rates, Hz.
pub const ECG_RATE_RANGE: (f64, f64) = (100.0, 2_000.0);

/// One detected QRS complex.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QrsEvent {
    /// Sample index of the R peak, counted from the first sample given to the detector.
    pub sample: u64,
    /// Signed R amplitude on the 0.5–40 Hz ECG relative to the local median, mV (used for
    /// ECG-derived respiration).
    pub amplitude_mv: f64,
    /// Found by search-back at the lower threshold.
    pub searchback: bool,
}

/// A fixed-capacity buffer addressed by absolute sample index.
#[derive(Debug, Clone)]
struct Ring {
    start: u64,
    data: VecDeque<f64>,
    cap: usize,
}

impl Ring {
    fn new(cap: usize) -> Self {
        Self {
            start: 0,
            data: VecDeque::with_capacity(cap + 1),
            cap,
        }
    }

    fn push(&mut self, v: f64) {
        self.data.push_back(v);
        if self.data.len() > self.cap {
            self.data.pop_front();
            self.start += 1;
        }
    }

    fn end(&self) -> u64 {
        self.start + self.data.len() as u64
    }

    /// Values in `[lo, hi]` (inclusive, clipped to what is held), with the index of the first.
    fn range(&self, lo: i64, hi: i64) -> (u64, Vec<f64>) {
        let lo = lo.max(self.start as i64).max(0) as u64;
        let hi = hi.min(self.end() as i64 - 1);
        if hi < lo as i64 {
            return (lo, Vec::new());
        }
        let a = (lo - self.start) as usize;
        let b = (hi as u64 - self.start) as usize;
        (lo, self.data.range(a..=b).copied().collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Peak {
    /// MWI sample index.
    idx: u64,
    /// MWI value.
    val: f64,
    /// Largest |derivative| inside the integration window.
    slope: f64,
    /// Above the amplitude floor.
    amp_ok: bool,
    /// R peak sample and amplitude.
    r: u64,
    amplitude: f64,
}

#[derive(Debug, Clone)]
struct Learning {
    start: u64,
    max_peak: f64,
    mwi_sum: f64,
    mwi_n: u64,
    peaks: Vec<Peak>,
}

/// Streaming Pan–Tompkins QRS detector (F-SIG-01).
#[derive(Debug, Clone)]
pub struct QrsDetector {
    fs: f64,
    bp: Cascade,
    bp_state: crate::filter::CascadeState,
    clean: Cascade,
    clean_state: crate::filter::CascadeState,
    /// Delay of the band-pass at 10 Hz and of the 0.5–40 Hz ECG, samples.
    d_bp: f64,
    d_clean: f64,
    win: usize,
    refractory: u64,
    t_wave: u64,
    learn_len: u64,
    last5: [f64; 5],
    sq: VecDeque<f64>,
    sq_sum: f64,
    mwi_prev: [f64; 2],
    bp_buf: Ring,
    clean_buf: Ring,
    deriv_buf: Ring,
    n: u64,
    pending: Option<(u64, f64)>,
    learning: Option<Learning>,
    spki: f64,
    npki: f64,
    last_qrs: Option<Peak>,
    rr: VecDeque<u64>,
    since_last: Vec<Peak>,
    searched: bool,
}

impl QrsDetector {
    /// A detector for an ECG sampled at `fs` Hz (100–2000 Hz), in millivolts.
    pub fn new(fs: f64) -> Result<Self, SignalError> {
        check_rate(fs, ECG_RATE_RANGE.0, ECG_RATE_RANGE.1)?;
        let bp = Cascade::bandpass(fs, 5.0, 15.0, 2)?;
        let clean = Cascade::bandpass(fs, 0.5, 40.0, 2)?;
        let d_bp = bp.group_delay(10.0);
        let d_clean = clean.group_delay(10.0);
        let win = ((0.15 * fs).round() as usize).max(1);
        let keep = (2.5 * fs) as usize;
        Ok(Self {
            fs,
            bp_state: bp.state(),
            clean_state: clean.state(),
            bp,
            clean,
            d_bp,
            d_clean,
            win,
            refractory: (0.2 * fs).round() as u64,
            t_wave: (0.36 * fs).round() as u64,
            learn_len: (2.0 * fs).round() as u64,
            last5: [0.0; 5],
            sq: VecDeque::with_capacity(win + 1),
            sq_sum: 0.0,
            mwi_prev: [0.0; 2],
            bp_buf: Ring::new(keep),
            clean_buf: Ring::new(keep),
            deriv_buf: Ring::new(keep),
            n: 0,
            pending: None,
            learning: Some(Learning {
                start: 0,
                max_peak: 0.0,
                mwi_sum: 0.0,
                mwi_n: 0,
                peaks: Vec::new(),
            }),
            spki: 0.0,
            npki: 0.0,
            last_qrs: None,
            rr: VecDeque::with_capacity(9),
            since_last: Vec::new(),
            searched: false,
        })
    }

    /// Sample rate, Hz.
    pub fn sample_rate(&self) -> f64 {
        self.fs
    }

    /// Samples processed so far.
    pub fn samples_seen(&self) -> u64 {
        self.n
    }

    /// Processes a chunk; returns the QRS complexes confirmed by it, in time order. A non-finite
    /// sample resets the detector (the caller treats it as a gap) and returns an error; nothing
    /// from that chunk is used.
    pub fn push(&mut self, x: &[f64]) -> Result<Vec<QrsEvent>, SignalError> {
        if x.iter().any(|v| !v.is_finite()) {
            *self = Self::new(self.fs)?;
            return Err(SignalError::NonFinite);
        }
        let mut out = Vec::new();
        for &v in x {
            self.step(v, &mut out);
        }
        Ok(out)
    }

    fn step(&mut self, x: f64, out: &mut Vec<QrsEvent>) {
        let n = self.n;
        let c = self.clean.step(&mut self.clean_state, x);
        let b = self.bp.step(&mut self.bp_state, x);
        self.clean_buf.push(c);
        self.bp_buf.push(b);
        self.last5 = [
            self.last5[1],
            self.last5[2],
            self.last5[3],
            self.last5[4],
            b,
        ];
        let [b4, b3, _, b1, b0] = self.last5;
        let d = (2.0 * b0 + b1 - b3 - 2.0 * b4) * self.fs / 8.0;
        self.deriv_buf.push(d);
        let s = d * d;
        self.sq.push_back(s);
        self.sq_sum += s;
        if self.sq.len() > self.win {
            self.sq_sum -= self.sq.pop_front().unwrap_or(0.0);
        }
        let m = (self.sq_sum / self.win as f64).max(0.0);

        // Raw local maximum of the MWI at n − 1.
        let [p2, p1] = self.mwi_prev;
        if n >= 2 && p1 > p2 && p1 >= m && p1 > 0.0 {
            let idx = n - 1;
            match self.pending {
                Some((pi, pv)) if idx - pi < self.refractory => {
                    if p1 > pv {
                        self.pending = Some((idx, p1));
                    }
                }
                Some(_) => {
                    self.release(out);
                    self.pending = Some((idx, p1));
                }
                None => self.pending = Some((idx, p1)),
            }
        }
        self.mwi_prev = [p1, m];
        if let Some((pi, _)) = self.pending {
            if n - pi >= self.refractory {
                self.release(out);
            }
        }

        if let Some(l) = self.learning.as_mut() {
            l.mwi_sum += m;
            l.mwi_n += 1;
        }
        self.finish_learning(out);
        self.searchback(out);
        self.n += 1;
    }

    /// Releases the pending peak to the threshold logic (or to the learning store).
    fn release(&mut self, out: &mut Vec<QrsEvent>) {
        let Some((idx, val)) = self.pending.take() else {
            return;
        };
        let peak = self.describe(idx, val);
        if let Some(l) = self.learning.as_mut() {
            if peak.amp_ok {
                l.max_peak = l.max_peak.max(val);
            }
            l.peaks.push(peak);
            return;
        }
        self.classify(peak, out);
    }

    fn describe(&self, idx: u64, val: f64) -> Peak {
        let p = idx as i64;
        let half = (self.win as i64 - 1) / 2;
        let h = (0.125 * self.fs).round() as i64;
        // Band-pass window around the implied QRS position (in band-pass time).
        let centre_bp = p - 2 - half;
        let (_, bpw) = self.bp_buf.range(centre_bp - h, centre_bp + h);
        let amp = bpw.iter().fold(0.0_f64, |a, v| a.max(v.abs()));
        let (_, dw) = self.deriv_buf.range(p - self.win as i64 + 1, p);
        let slope = dw.iter().fold(0.0_f64, |a, v| a.max(v.abs()));
        // R peak on the 0.5–40 Hz ECG.
        let centre_ecg = centre_bp as f64 - self.d_bp;
        let centre_clean = (centre_ecg + self.d_clean).round() as i64;
        let (first, cw) = self.clean_buf.range(centre_clean - h, centre_clean + h);
        let med = crate::stats::median(&cw).unwrap_or(0.0);
        let mut best = (first, 0.0_f64);
        for (k, v) in cw.iter().enumerate() {
            if (v - med).abs() > best.1.abs() {
                best = (first + k as u64, v - med);
            }
        }
        let r = (best.0 as f64 - self.d_clean).round().max(0.0) as u64;
        Peak {
            idx,
            val,
            slope,
            amp_ok: amp >= AMPLITUDE_FLOOR_MV,
            r,
            amplitude: best.1,
        }
    }

    fn finish_learning(&mut self, out: &mut Vec<QrsEvent>) {
        let Some(l) = self.learning.as_mut() else {
            return;
        };
        if self.n + 1 < l.start + self.learn_len {
            return;
        }
        if l.max_peak <= 0.0 {
            // Nothing above the amplitude floor yet (e.g. an asystole at start): keep learning on
            // the next 2 s; those peaks were noise.
            *l = Learning {
                start: self.n + 1,
                max_peak: 0.0,
                mwi_sum: 0.0,
                mwi_n: 0,
                peaks: Vec::new(),
            };
            return;
        }
        let mean = if l.mwi_n > 0 {
            l.mwi_sum / l.mwi_n as f64
        } else {
            0.0
        };
        self.spki = l.max_peak / 3.0;
        self.npki = mean / 2.0;
        let peaks = std::mem::take(&mut l.peaks);
        self.learning = None;
        for p in peaks {
            self.classify(p, out);
        }
    }

    fn threshold(&self) -> f64 {
        self.npki + 0.25 * (self.spki - self.npki)
    }

    fn classify(&mut self, p: Peak, out: &mut Vec<QrsEvent>) {
        if !p.amp_ok {
            self.noise(p);
            return;
        }
        if p.val > self.threshold() {
            if let Some(last) = self.last_qrs {
                if p.idx.saturating_sub(last.idx) < self.t_wave && p.slope < 0.5 * last.slope {
                    self.noise(p);
                    return;
                }
            }
            self.spki = 0.125 * p.val + 0.875 * self.spki;
            self.accept(p, false, out);
        } else {
            self.noise(p);
        }
    }

    fn noise(&mut self, p: Peak) {
        self.npki = 0.125 * p.val + 0.875 * self.npki;
        self.since_last.push(p);
    }

    fn accept(&mut self, p: Peak, searchback: bool, out: &mut Vec<QrsEvent>) {
        if let Some(last) = self.last_qrs {
            if p.r <= last.r {
                return; // never emit out of order
            }
            self.rr.push_back(p.r - last.r);
            if self.rr.len() > 8 {
                self.rr.pop_front();
            }
        }
        self.last_qrs = Some(p);
        self.since_last.clear();
        self.searched = false;
        out.push(QrsEvent {
            sample: p.r,
            amplitude_mv: p.amplitude,
            searchback,
        });
    }

    fn searchback(&mut self, out: &mut Vec<QrsEvent>) {
        if self.learning.is_some() || self.searched || self.rr.is_empty() {
            return;
        }
        let Some(last) = self.last_qrs else {
            return;
        };
        let mean_rr = self.rr.iter().sum::<u64>() as f64 / self.rr.len() as f64;
        if ((self.n - last.idx) as f64) < 1.66 * mean_rr {
            return;
        }
        self.searched = true;
        let th2 = 0.5 * self.threshold();
        let best = self
            .since_last
            .iter()
            .filter(|p| p.amp_ok && p.val > th2 && p.idx - last.idx >= self.refractory)
            .filter(|p| !(p.idx - last.idx < self.t_wave && p.slope < 0.5 * last.slope))
            .copied()
            .fold(None::<Peak>, |a, p| match a {
                Some(b) if b.val >= p.val => Some(b),
                _ => Some(p),
            });
        if let Some(p) = best {
            self.spki = 0.25 * p.val + 0.75 * self.spki;
            // Peaks after the recovered beat stay candidates for the next search.
            let rest: Vec<Peak> = self
                .since_last
                .iter()
                .filter(|q| q.idx > p.idx)
                .copied()
                .collect();
            self.accept(p, true, out);
            self.since_last = rest;
        }
    }
}

/// Detects QRS complexes in a whole record (F-SIG-01). Returns R-peak sample indices.
pub fn detect_qrs(ecg_mv: &[f64], fs: f64) -> Result<Vec<QrsEvent>, SignalError> {
    let mut d = QrsDetector::new(fs)?;
    let mut out = d.push(ecg_mv)?;
    // Flush: run the detector over 1 s of the last value so peaks near the end are released.
    let tail = ecg_mv.last().copied().unwrap_or(0.0);
    let flush = vec![tail; fs as usize];
    out.extend(d.push(&flush)?);
    let n = ecg_mv.len() as u64;
    out.retain(|e| e.sample < n);
    Ok(out)
}

/// Length of the SQI window, s.
pub const SQI_WINDOW_S: f64 = 5.0;
/// Heart-rate window, s.
pub const HR_WINDOW_S: f64 = 10.0;
/// The SQI and heart-rate windows end this long before the newest sample, so every QRS in them
/// has been confirmed by the detector, s.
pub const REPORT_LAG_S: f64 = 0.6;

/// Once-per-second ECG summary from [`EcgProcessor`].
#[derive(Debug, Clone, PartialEq)]
pub struct EcgReport {
    /// Last sample index covered by the SQI and heart-rate windows.
    pub end_sample: u64,
    /// ECG signal quality over the last [`SQI_WINDOW_S`] (→ `sqi_ecg`, gate of
    /// `asystole.critical`).
    pub sqi: SqiResult,
    /// Heart rate over the last [`HR_WINDOW_S`] (→ `heart_rate`), `None` when fewer than three RR
    /// intervals are available or the signal is unusable.
    pub heart_rate: Option<Derived<f64>>,
}

/// Output of one [`EcgProcessor::push`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EcgOutput {
    /// QRS complexes confirmed by this chunk (→ the `qrs` event of `asystole.critical`).
    pub qrs: Vec<QrsEvent>,
    /// Reports due in this chunk, one per second of signal.
    pub reports: Vec<EcgReport>,
}

/// Streaming ECG pipeline for one lead of one device (F-SIG-01, F-SIG-02): QRS events as they
/// are confirmed, and once per second the ECG SQI and heart rate.
#[derive(Debug, Clone)]
pub struct EcgProcessor {
    detector: QrsDetector,
    raw: Ring,
    beats: VecDeque<QrsEvent>,
    next_report: u64,
}

impl EcgProcessor {
    /// A processor for an ECG at `fs` Hz in millivolts.
    pub fn new(fs: f64) -> Result<Self, SignalError> {
        let detector = QrsDetector::new(fs)?;
        let keep = ((HR_WINDOW_S + REPORT_LAG_S + 1.0) * fs) as usize;
        Ok(Self {
            detector,
            raw: Ring::new(keep),
            beats: VecDeque::new(),
            next_report: ((SQI_WINDOW_S + REPORT_LAG_S) * fs).round() as u64,
        })
    }

    /// Sample rate, Hz.
    pub fn sample_rate(&self) -> f64 {
        self.detector.sample_rate()
    }

    /// Processes a chunk. A non-finite sample resets the processor and returns an error.
    pub fn push(&mut self, x: &[f64]) -> Result<EcgOutput, SignalError> {
        let fs = self.detector.sample_rate();
        if x.iter().any(|v| !v.is_finite()) {
            *self = Self::new(fs)?;
            return Err(SignalError::NonFinite);
        }
        let mut out = EcgOutput::default();
        let lag = (REPORT_LAG_S * fs).round() as u64;
        let sqi_len = (SQI_WINDOW_S * fs).round() as u64;
        let hr_len = (HR_WINDOW_S * fs).round() as u64;
        let step = fs.round() as u64;
        for &v in x {
            let qrs = self.detector.push(&[v])?;
            self.raw.push(v);
            for e in qrs {
                self.beats.push_back(e);
                out.qrs.push(e);
            }
            let seen = self.detector.samples_seen();
            if seen >= self.next_report {
                let end = seen - lag; // exclusive
                out.reports.push(self.report(end, sqi_len, hr_len)?);
                self.next_report += step.max(1);
                while self
                    .beats
                    .front()
                    .is_some_and(|b| b.sample + hr_len + lag < seen)
                {
                    self.beats.pop_front();
                }
            }
        }
        Ok(out)
    }

    fn report(&self, end: u64, sqi_len: u64, hr_len: u64) -> Result<EcgReport, SignalError> {
        let fs = self.detector.sample_rate();
        let start = end.saturating_sub(sqi_len);
        let (first, window) = self.raw.range(start as i64, end as i64 - 1);
        let beats_in: Vec<usize> = self
            .beats
            .iter()
            .filter(|b| b.sample >= first && b.sample < end)
            .map(|b| (b.sample - first) as usize)
            .collect();
        let sqi = sqi::ecg_sqi(&window, fs, &beats_in)?;
        let hr_start = end.saturating_sub(hr_len);
        let times: Vec<f64> = self
            .beats
            .iter()
            .filter(|b| b.sample >= hr_start && b.sample < end)
            .map(|b| b.sample as f64 / fs)
            .collect();
        let heart_rate = if sqi.quality == Quality::Unusable {
            None
        } else {
            hrv::heart_rate(&times, sqi.quality)
        };
        Ok(EcgReport {
            end_sample: end.saturating_sub(1),
            sqi,
            heart_rate,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_ranges_are_clipped() {
        let mut r = Ring::new(4);
        for v in 0..6 {
            r.push(v as f64);
        }
        assert_eq!(r.start, 2);
        assert_eq!(r.range(0, 3), (2, vec![2.0, 3.0]));
        assert_eq!(r.range(4, 10), (4, vec![4.0, 5.0]));
        assert_eq!(r.range(7, 9).1, Vec::<f64>::new());
    }

    #[test]
    fn invalid_rate_and_non_finite_samples_are_refused() {
        assert!(QrsDetector::new(50.0).is_err());
        let mut d = QrsDetector::new(250.0).unwrap();
        assert_eq!(d.push(&[0.0, f64::NAN]), Err(SignalError::NonFinite));
        assert_eq!(d.samples_seen(), 0, "the detector restarts after a gap");
        let mut p = EcgProcessor::new(250.0).unwrap();
        assert!(p.push(&[f64::INFINITY]).is_err());
    }

    /// F-SIG-01 / H-01: a flat line never yields beats, and learning waits for a real QRS.
    #[test]
    fn f_sig_01_flat_line_gives_no_beats() {
        assert!(detect_qrs(&vec![0.0; 5_000], 250.0).unwrap().is_empty());
    }
}
