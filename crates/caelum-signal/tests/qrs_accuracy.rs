//! F-SIG-01 / F-SIG-02 accuracy on synthetic ECG (test fixtures generated in test code):
//! QRS detection at 40–180 bpm, clean and with noise, baseline wander and mains interference;
//! sensitivity and PPV ≥ 99 % clean and ≥ 95 % with noise (CR-013 §12); asystole logic: a pause
//! yields no QRS while the SQI stays good, and noise yields an unusable SQI.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use caelum_signal::ecg::{detect_qrs, EcgProcessor};
use caelum_signal::sqi::{ecg_sqi, SqiFinding};
use caelum_signal::Quality;
use common::*;

const RATES: [f64; 8] = [40.0, 50.0, 60.0, 80.0, 100.0, 130.0, 160.0, 180.0];
const SECONDS: f64 = 120.0;

fn run(fs: f64, bpm: f64, seed: u64, corrupt: impl Fn(&mut Vec<f64>, &mut Rng)) -> Score {
    let mut rng = Rng::new(seed);
    let beats = beat_times(bpm, SECONDS, 0.03, &mut rng);
    let mut x = ecg(fs, SECONDS, &beats, 1.0);
    corrupt(&mut x, &mut rng);
    let det: Vec<f64> = detect_qrs(&x, fs)
        .unwrap()
        .iter()
        .map(|e| e.sample as f64 / fs)
        .collect();
    score(&beats, &det, 0.075, 0.0, SECONDS)
}

/// F-SIG-01: clean ECG, 40–180 bpm, 250 and 500 Hz: Se and PPV ≥ 99 %.
#[test]
fn f_sig_01_qrs_clean_40_to_180_bpm() {
    for fs in [250.0, 500.0] {
        for (k, bpm) in RATES.iter().enumerate() {
            let s = run(fs, *bpm, k as u64, |_, _| {});
            assert!(
                s.sensitivity() >= 0.99 && s.ppv() >= 0.99,
                "{fs} Hz {bpm} bpm: {s:?}"
            );
        }
    }
}

/// F-SIG-01: baseline wander (1.5 mV), white noise (0.1 mV RMS, SNR ≈ 20 dB) and 50 Hz mains
/// (0.1 mV): Se and PPV ≥ 95 %.
#[test]
fn f_sig_01_qrs_with_noise_and_baseline_wander() {
    for (k, bpm) in RATES.iter().enumerate() {
        let s = run(250.0, *bpm, 100 + k as u64, |x, rng| {
            add_wander(x, 250.0, 1.5);
            add_noise(x, 0.1, rng);
            add_mains(x, 250.0, 50.0, 0.1);
        });
        assert!(
            s.sensitivity() >= 0.95 && s.ppv() >= 0.95,
            "{bpm} bpm: {s:?}"
        );
    }
}

/// F-SIG-01: low-amplitude wearable ECG (0.3 mV R wave) with 0.03 mV noise: ≥ 95 %.
#[test]
fn f_sig_01_qrs_low_amplitude_wearable() {
    for (k, bpm) in [45.0, 75.0, 150.0].iter().enumerate() {
        let mut rng = Rng::new(200 + k as u64);
        let beats = beat_times(*bpm, SECONDS, 0.03, &mut rng);
        let mut x = ecg(250.0, SECONDS, &beats, 0.3);
        add_wander(&mut x, 250.0, 0.5);
        add_noise(&mut x, 0.03, &mut rng);
        let det: Vec<f64> = detect_qrs(&x, 250.0)
            .unwrap()
            .iter()
            .map(|e| e.sample as f64 / 250.0)
            .collect();
        let s = score(&beats, &det, 0.075, 0.0, SECONDS);
        assert!(s.sensitivity() >= 0.95 && s.ppv() >= 0.95, "{bpm}: {s:?}");
    }
}

/// F-SIG-01 / F-SIG-02 / H-01: sinus rhythm, then a 10 s pause (isoelectric line with amplifier
/// noise and wander), then sinus again. No QRS is reported in the pause, and the SQI over the
/// pause stays at or above the `asystole.critical` gate (0.7), so the absence rule can fire.
#[test]
fn f_sig_01_pause_gives_no_qrs_while_sqi_stays_good() {
    let fs = 250.0;
    let mut rng = Rng::new(7);
    let mut beats: Vec<f64> = beat_times(70.0, 30.0, 0.02, &mut rng);
    let resume = beats.last().unwrap() + 10.0;
    beats.extend(
        beat_times(70.0, 30.0, 0.02, &mut rng)
            .iter()
            .map(|t| t + resume - 0.4),
    );
    let seconds = 72.0;
    let mut x = ecg(fs, seconds, &beats, 1.0);
    add_wander(&mut x, fs, 0.3);
    add_noise(&mut x, 0.01, &mut rng);

    let mut p = EcgProcessor::new(fs).unwrap();
    let mut qrs = Vec::new();
    let mut reports = Vec::new();
    for chunk in x.chunks(25) {
        let out = p.push(chunk).unwrap();
        qrs.extend(out.qrs.iter().map(|e| e.sample as f64 / fs));
        reports.extend(out.reports);
    }
    let pause_start = beats
        .iter()
        .copied()
        .filter(|t| *t < resume - 1.0)
        .fold(0.0, f64::max);
    let in_pause: Vec<f64> = qrs
        .iter()
        .copied()
        .filter(|t| *t > pause_start + 0.2 && *t < resume - 0.2)
        .collect();
    assert!(in_pause.is_empty(), "beats in the pause: {in_pause:?}");
    let longest = qrs.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max);
    assert!(longest > 9.5, "longest RR {longest}");
    // Every SQI report over the whole record, pause included, opens the gate.
    for r in &reports {
        assert!(
            r.sqi.score >= 0.7,
            "end {} s: {:?}",
            r.end_sample as f64 / fs,
            r.sqi
        );
    }
    // Heart rate resumes after the pause.
    assert!(reports.last().unwrap().heart_rate.is_some());
}

/// F-SIG-02 / SR-005: noise without ECG (electrode artefact, EMG) scores UNUSABLE, so the
/// absence rule stays closed and a technical alarm is raised instead of an asystole alarm.
#[test]
fn f_sig_02_noise_gives_unusable_sqi() {
    let fs = 250.0;
    let mut rng = Rng::new(9);
    for rms in [0.15, 0.3, 1.0] {
        let mut x = vec![0.0; (10.0 * fs) as usize];
        add_noise(&mut x, rms, &mut rng);
        let mut p = EcgProcessor::new(fs).unwrap();
        let out = p.push(&x).unwrap();
        assert!(!out.reports.is_empty());
        for r in &out.reports {
            assert_eq!(r.sqi.quality, Quality::Unusable, "{rms}: {:?}", r.sqi);
            assert!(r.sqi.score < 0.7);
            assert_eq!(r.heart_rate, None);
        }
    }
    // Lead off (flat line) is unusable too.
    let flat = ecg_sqi(&vec![0.0; 1_250], fs, &[]).unwrap();
    assert_eq!(flat.findings, vec![SqiFinding::FlatLine]);
}

/// F-SIG-02: clean and moderately noisy ECG score GOOD.
#[test]
fn f_sig_02_clean_ecg_scores_good() {
    let fs = 250.0;
    let mut rng = Rng::new(11);
    let beats = beat_times(75.0, 20.0, 0.03, &mut rng);
    let mut x = ecg(fs, 20.0, &beats, 1.0);
    add_wander(&mut x, fs, 1.0);
    add_noise(&mut x, 0.02, &mut rng);
    let mut p = EcgProcessor::new(fs).unwrap();
    let out = p.push(&x).unwrap();
    for r in &out.reports {
        assert_eq!(r.sqi.quality, Quality::Good, "{:?}", r.sqi);
        let hr = r.heart_rate.unwrap();
        assert!((hr.value - 75.0).abs() < 4.0, "{}", hr.value);
    }
}
