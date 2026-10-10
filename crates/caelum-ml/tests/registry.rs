//! F-ML-01 registry, status and time limits; F-ML-04 explanations.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code

mod common;

use std::time::{Duration, Instant};

use caelum_domain::time::TaiNs;
use caelum_ml::advisory::{AdvisoryLevel, AdvisoryNotice, DisplayStatus};
use caelum_ml::manifest::ValidationStatus;
use caelum_ml::registry::{EvaluationError, InstallError, Registry};
use caelum_ml::runtime::{NoRuntime, RuntimeError};
use common::*;

/// AF-like test weights: high HR and high RMSSD raise the output.
const W: [f32; 2] = [0.05, 0.05];
const B: f32 = -6.0;

fn registry_with(factory: &dyn caelum_ml::runtime::RuntimeFactory) -> Registry {
    let mut r = Registry::new();
    r.install(verified(&W, B, "1.0.0"), factory)
        .expect("installs");
    r
}

#[test]
fn f_ml_01_no_runtime_means_unavailable_and_no_output() {
    let r = registry_with(&NoRuntime);
    let s = r.statuses();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].status, DisplayStatus::Unavailable);
    assert!(s[0].reason.as_deref().unwrap().contains("onnx"));
    let err = r
        .evaluate("T2", crew(), &samples(0, 60, 70.0), TaiNs(60 * NS))
        .unwrap_err();
    assert!(matches!(err, EvaluationError::Unavailable { .. }));
}

#[test]
fn f_ml_01_status_view_shows_validation_and_card() {
    let r = registry_with(&LinearFactory);
    let s = &r.statuses()[0];
    assert_eq!(s.status, DisplayStatus::Investigational);
    assert_eq!(s.validation, ValidationStatus::NotValidated);
    assert!(s.development_signed);
    assert_eq!(s.intended_use, "Test equipment only");
    assert_eq!(s.model_sha256.len(), 64);
    let json = serde_json::to_value(s).unwrap();
    assert_eq!(json["status"], "INVESTIGATIONAL");
    assert_eq!(json["validation"], "NOT_VALIDATED");
}

#[test]
fn f_ml_01_versions_only_move_forward_and_rollback_needs_removal() {
    let mut r = registry_with(&LinearFactory);
    let err = r
        .install(verified(&W, B, "1.0.0"), &LinearFactory)
        .unwrap_err();
    assert!(matches!(err, InstallError::NotNewer { .. }));
    r.install(verified(&W, B, "1.10.0"), &LinearFactory)
        .expect("newer");
    let err = r
        .install(verified(&W, B, "1.9.0"), &LinearFactory)
        .unwrap_err();
    assert!(matches!(err, InstallError::NotNewer { .. }));
    assert!(r.remove("T2"));
    r.install(verified(&W, B, "1.9.0"), &LinearFactory)
        .expect("after removal");
    assert_eq!(r.statuses()[0].version, "1.9.0");
}

#[test]
fn f_ml_01_disabled_models_produce_nothing_until_enabled() {
    let mut r = registry_with(&LinearFactory);
    assert!(r.disable("T2", "turned off by the CMO"));
    assert_eq!(r.statuses()[0].status, DisplayStatus::Disabled);
    let err = r
        .evaluate("T2", crew(), &samples(0, 60, 70.0), TaiNs(60 * NS))
        .unwrap_err();
    assert_eq!(err, EvaluationError::Disabled("T2".to_owned()));
    assert_eq!(
        r.enable("T2", &LinearFactory),
        Some(DisplayStatus::Investigational)
    );
    r.evaluate("T2", crew(), &samples(0, 60, 70.0), TaiNs(60 * NS))
        .expect("evaluates again");
    assert!(!r.disable("nope", "x"));
    assert!(matches!(
        r.evaluate("nope", crew(), &[], TaiNs(0)),
        Err(EvaluationError::UnknownModel(_))
    ));
}

#[test]
fn f_ml_01_f_ml_04_every_output_is_explained() {
    let r = registry_with(&LinearFactory);
    let out = r
        .evaluate("T2", crew(), &samples(0, 60, 150.0), TaiNs(60 * NS))
        .expect("output");
    assert_eq!(out.len(), 1);
    let res = &out[0];
    // sigmoid(0.05*150 + 0.05*60 - 6) = sigmoid(4.5)
    let expected = 1.0 / (1.0 + (-4.5f32).exp());
    assert!(
        (res.value - f64::from(expected)).abs() < 1e-6,
        "{}",
        res.value
    );
    assert_eq!(res.status, DisplayStatus::Investigational);
    assert_eq!(res.contributions.len(), 2, "top_k = 2");
    // HR (150 vs reference 70) moves the output more than RMSSD (60 vs 30).
    assert_eq!(res.contributions[0].feature, "hr_mean");
    assert_eq!(res.contributions[0].value, Some(150.0));
    assert_eq!(res.contributions[0].reference, 70.0);
    assert!(res.contributions[0].delta > res.contributions[1].delta);
    assert!(res.contributions.iter().all(|c| c.delta > 0.0));
    // Notice: above 0.8, observe level only, names the model and its status.
    let notice = AdvisoryNotice::from_result(res).expect("notice");
    assert_eq!(notice.level(), AdvisoryLevel::Observe);
    assert_eq!(
        notice.level().priority(),
        caelum_alarms::ruleset::Priority::Observe
    );
    assert!(notice.title().contains("T2") && notice.title().contains("investigational"));
    assert_eq!(notice.result().contributions, res.contributions);
}

#[test]
fn f_ml_01_below_notice_threshold_gives_result_without_notice() {
    let r = registry_with(&LinearFactory);
    let out = r
        .evaluate("T2", crew(), &samples(0, 60, 60.0), TaiNs(60 * NS))
        .expect("output");
    assert!(out[0].value < 0.8);
    assert!(AdvisoryNotice::from_result(&out[0]).is_none());
}

#[test]
fn f_ml_01_missing_data_gives_no_output() {
    let r = registry_with(&LinearFactory);
    let err = r
        .evaluate("T2", crew(), &samples(0, 30, 70.0), TaiNs(60 * NS))
        .unwrap_err();
    assert!(matches!(err, EvaluationError::NoData(_)), "{err}");
}

#[test]
fn f_ml_01_hard_time_limit_then_busy_then_recovers() {
    let r = registry_with(&AdversarialFactory(Adversary::Hang(Duration::from_millis(
        1500,
    ))));
    let started = Instant::now();
    let err = r
        .evaluate("T2", crew(), &samples(0, 60, 70.0), TaiNs(60 * NS))
        .unwrap_err();
    assert_eq!(
        err,
        EvaluationError::Runtime(RuntimeError::TimedOut { limit_ms: 200 })
    );
    assert!(
        started.elapsed() < Duration::from_millis(1200),
        "returned at the limit"
    );
    // The timed-out worker is still running: no second inference piles up.
    let err = r
        .evaluate("T2", crew(), &samples(0, 60, 70.0), TaiNs(60 * NS))
        .unwrap_err();
    assert_eq!(err, EvaluationError::Runtime(RuntimeError::Busy));
    // Once the worker returns, the model runs again (and times out again: no output, ever).
    std::thread::sleep(Duration::from_millis(1700));
    let err = r
        .evaluate("T2", crew(), &samples(0, 60, 70.0), TaiNs(60 * NS))
        .unwrap_err();
    assert_eq!(
        err,
        EvaluationError::Runtime(RuntimeError::TimedOut { limit_ms: 200 })
    );
}

#[test]
fn f_ml_01_panicking_or_failing_runtime_gives_no_output() {
    let r = registry_with(&AdversarialFactory(Adversary::Panic));
    let err = r
        .evaluate("T2", crew(), &samples(0, 60, 70.0), TaiNs(60 * NS))
        .unwrap_err();
    assert_eq!(err, EvaluationError::Runtime(RuntimeError::WorkerLost));
    // The in-flight flag was cleared during unwinding.
    let err = r
        .evaluate("T2", crew(), &samples(0, 60, 70.0), TaiNs(60 * NS))
        .unwrap_err();
    assert_eq!(err, EvaluationError::Runtime(RuntimeError::WorkerLost));
    let r = registry_with(&AdversarialFactory(Adversary::Fail));
    assert!(matches!(
        r.evaluate("T2", crew(), &samples(0, 60, 70.0), TaiNs(60 * NS)),
        Err(EvaluationError::Runtime(RuntimeError::Run(_)))
    ));
}

#[test]
fn f_ml_01_implausible_outputs_are_rejected() {
    for v in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        1e30,
        -0.5,
        1.000_001,
        f32::MAX,
    ] {
        let r = registry_with(&AdversarialFactory(Adversary::Constant(v)));
        let err = r
            .evaluate("T2", crew(), &samples(0, 60, 70.0), TaiNs(60 * NS))
            .unwrap_err();
        assert!(
            matches!(err, EvaluationError::Implausible { .. }),
            "{v}: {err}"
        );
    }
    let r = registry_with(&AdversarialFactory(Adversary::Empty));
    assert!(matches!(
        r.evaluate("T2", crew(), &samples(0, 60, 70.0), TaiNs(60 * NS)),
        Err(EvaluationError::OutputShape { found: 0, index: 0 })
    ));
}

#[test]
fn f_ml_04_no_explanation_means_no_output() {
    // First inference plausible, occlusion runs return NaN: the output is withheld.
    let r = registry_with(&AdversarialFactory(Adversary::FirstThen(0.9, f32::NAN)));
    let err = r
        .evaluate("T2", crew(), &samples(0, 60, 70.0), TaiNs(60 * NS))
        .unwrap_err();
    assert!(matches!(err, EvaluationError::Explanation(_)), "{err}");
}
