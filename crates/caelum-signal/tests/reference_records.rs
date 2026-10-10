//! M7 gate on PhysioNet reference records (fixtures in tests/fixtures, ODC-By 1.0, see README):
//! - F-SIG-01 QRS accuracy on MIT-BIH Arrhythmia Database excerpts;
//! - F-SIG-01/F-SIG-02 + asystole.critical: the QRS events and ECG SQI drive the real alarm
//!   engine with the bundled ruleset; the alarm fires on true asystole alarms of the
//!   PhysioNet/CinC Challenge 2015 and does not fire on its false asystole alarms (noise and
//!   artefact with poor SQI).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::str::FromStr;

use caelum_alarms::engine::{Engine, Event, Input};
use caelum_alarms::ruleset::Ruleset;
use caelum_domain::ids::CrewMemberId;
use caelum_domain::observation::{MeasurementMode, Quality};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_signal::ecg::{detect_qrs, EcgProcessor};
use common::*;

fn detections(r: &Record) -> Vec<f64> {
    detect_qrs(&r.mv, r.fs)
        .unwrap()
        .iter()
        .map(|e| e.sample as f64 / r.fs)
        .collect()
}

fn mitdb(name: &str, start: u64) -> (Record, Score) {
    let r = read_record(name, start, 200.0, 1024.0);
    let end = r.mv.len() as f64 / r.fs;
    let s = score(&r.beats_s, &detections(&r), 0.15, 1.0, end - 1.0);
    (r, s)
}

/// F-SIG-01: MIT-BIH 100 (normal sinus rhythm, a few APCs), first 180 s: Se and PPV ≥ 99 %.
#[test]
fn f_sig_01_mitdb_100_clean() {
    let (_, s) = mitdb("100", 0);
    eprintln!("100: Se {:.4} PPV {:.4} {s:?}", s.sensitivity(), s.ppv());
    assert!(s.sensitivity() >= 0.99 && s.ppv() >= 0.99, "{s:?}");
}

/// F-SIG-01: MIT-BIH 119 (ventricular bigeminy), first 180 s: ectopic beats are detected,
/// Se and PPV ≥ 99 %.
#[test]
fn f_sig_01_mitdb_119_bigeminy() {
    let (_, s) = mitdb("119", 0);
    eprintln!("119: Se {:.4} PPV {:.4} {s:?}", s.sensitivity(), s.ppv());
    assert!(s.sensitivity() >= 0.99 && s.ppv() >= 0.99, "{s:?}");
}

/// F-SIG-01: MIT-BIH 105 (heavy noise and artefact), first 180 s: Se and PPV ≥ 95 %.
#[test]
fn f_sig_01_mitdb_105_noisy() {
    let (_, s) = mitdb("105", 0);
    eprintln!("105: Se {:.4} PPV {:.4} {s:?}", s.sensitivity(), s.ppv());
    assert!(s.sensitivity() >= 0.95 && s.ppv() >= 0.95, "{s:?}");
}

/// F-SIG-01 / H-01: MIT-BIH 232 (sick sinus, pauses up to 5.9 s), 1020–1080 s: every pause is
/// reproduced without false beats inside it, and no beat is missed.
#[test]
fn f_sig_01_mitdb_232_pauses() {
    let (r, s) = mitdb("232", 367_200);
    eprintln!("232: Se {:.4} PPV {:.4} {s:?}", s.sensitivity(), s.ppv());
    assert!(s.sensitivity() >= 0.99 && s.ppv() >= 0.99, "{s:?}");
    let longest_ref = r
        .beats_s
        .windows(2)
        .map(|w| w[1] - w[0])
        .fold(0.0, f64::max);
    let det = detections(&r);
    let longest_det = det.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max);
    assert!(longest_ref > 5.5, "{longest_ref}");
    assert!(
        (longest_det - longest_ref).abs() < 0.15,
        "{longest_det} vs {longest_ref}"
    );
}

/// A Challenge 2015 excerpt: channel 0 (ECG II) in mV from 250 s; `None` for invalid samples.
fn challenge(name: &str, nsig: usize, gain: f64) -> Vec<Option<f64>> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/challenge2015/");
    let dat = std::fs::read(format!("{path}{name}.dat")).unwrap();
    dat.chunks_exact(2 * nsig)
        .map(|f| {
            let v = i16::from_le_bytes([f[0], f[1]]);
            (v != i16::MIN).then(|| f64::from(v) / gain)
        })
        .collect()
}

/// Runs the ECG through the processor and the alarm engine as the lead wires them
/// (docs/integration/M7.md): QRS → `event("qrs")`, SQI → `sqi_ecg` observation, 4 Hz tick.
/// Returns the record times (s) at which `asystole.critical` was raised.
fn asystole_times(ecg: &[Option<f64>], fs: f64, start_s: f64) -> Vec<f64> {
    let ruleset = Ruleset::parse(caelum_alarms::BUNDLED_RULESET).unwrap();
    let mut engine = Engine::new(ruleset);
    let crew = CrewMemberId::from_str("00000000-0000-4000-8000-000000000001").unwrap();
    let tai = |s: f64| TaiNs((s * 1e9) as i64);
    let mut p = EcgProcessor::new(fs).unwrap();
    let mut offset = 0usize; // sample index of the processor's sample 0
    let mut raised = Vec::new();
    let chunk = (fs / 4.0) as usize;
    let mut events = Vec::new();
    for (k, c) in ecg.chunks(chunk).enumerate() {
        let first = k * chunk;
        let samples: Option<Vec<f64>> = c.iter().copied().collect();
        match samples {
            Some(s) => {
                let out = p.push(&s).unwrap();
                for q in out.qrs {
                    let t = start_s + (offset as f64 + q.sample as f64) / fs;
                    events.extend(engine.event(crew, "qrs", tai(t)));
                }
                for r in out.reports {
                    let t = start_s + (offset as f64 + r.end_sample as f64) / fs;
                    let input = Input {
                        crew,
                        device: None,
                        parameter: Parameter::SqiEcg,
                        value: r.sqi.score,
                        at: tai(t),
                        quality: Quality::Good,
                        mode: MeasurementMode::Continuous,
                        site: None,
                    };
                    events.extend(engine.evaluate(&input, None));
                }
            }
            None => {
                // Invalid samples: a gap. The processor restarts after it.
                p = EcgProcessor::new(fs).unwrap();
                offset = first + c.len();
            }
        }
        let now = start_s + (first + c.len()) as f64 / fs;
        events.extend(engine.tick(tai(now)));
        for e in events.drain(..) {
            if let Event::Raised(a) = e {
                if a.rule_id == "asystole.critical" {
                    raised.push(now);
                }
            }
        }
    }
    raised
}

/// M7 gate: `asystole.critical` fires on true asystole alarms (event between 290 and 300 s).
#[test]
fn asystole_fires_on_true_asystole_records() {
    for (name, nsig, gain, record_end) in
        [("a161l", 4, 5976.0, 330.0), ("a172s", 4, 28740.0, 300.0)]
    {
        let ecg = challenge(name, nsig, gain);
        let fired = asystole_times(&ecg, 250.0, 250.0);
        eprintln!("{name}: asystole raised at {fired:?}");
        assert!(
            fired.iter().any(|t| *t >= 290.0 && *t <= record_end),
            "{name}: {fired:?}"
        );
    }
}

/// M7 gate: `asystole.critical` does not fire on false asystole alarms (noise, artefact,
/// lead problems): the SQI keeps the gate closed or the beats are still detected.
#[test]
fn asystole_does_not_fire_on_false_alarm_records() {
    for (name, nsig, gain) in [
        ("a103l", 3, 7247.0),
        ("a104s", 3, 9902.0),
        ("a105l", 3, 5321.0),
    ] {
        let ecg = challenge(name, nsig, gain);
        let fired = asystole_times(&ecg, 250.0, 250.0);
        eprintln!("{name}: asystole raised at {fired:?}");
        assert!(fired.is_empty(), "{name}: {fired:?}");
    }
}
