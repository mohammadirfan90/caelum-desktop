//! F-ML-01 / CLAUDE.md rule 2: feature builders never impute; missing data means no output.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code

mod common;

use caelum_domain::observation::Quality;
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_ml::features::{self, build_feature, MissingData};
use caelum_ml::manifest::{Aggregate, FeatureSpec};
use common::*;

fn spec(aggregate: Aggregate, min_samples: u32, length: Option<u32>) -> FeatureSpec {
    FeatureSpec {
        name: "f".to_owned(),
        parameter: "heart_rate".to_owned(),
        aggregate,
        window_s: 20,
        min_samples,
        max_gap_s: 5,
        length,
        reference: 60.0,
    }
}

fn hr(values: &[(i64, f64)]) -> Vec<features::Sample> {
    values
        .iter()
        .map(|(t, v)| sample(Parameter::HeartRate, *v, *t, Quality::Good))
        .collect()
}

fn even(from: i64, to: i64, step: i64, v: impl Fn(i64) -> f64) -> Vec<(i64, f64)> {
    (0..)
        .map(|i| from + i * step)
        .take_while(|t| *t <= to)
        .map(|t| (t, v(t)))
        .collect()
}

#[test]
fn f_ml_01_aggregates_over_a_complete_window() {
    let s = hr(&even(100, 120, 4, |t| (t - 100) as f64)); // 0,4,8,12,16,20 at t=100..120
    let now = TaiNs(120 * NS);
    let get = |a| build_feature(&spec(a, 2, None), &s, now).unwrap().0;
    assert_eq!(get(Aggregate::Mean), vec![10.0]);
    assert_eq!(get(Aggregate::Median), vec![10.0]);
    assert_eq!(get(Aggregate::Min), vec![0.0]);
    assert_eq!(get(Aggregate::Max), vec![20.0]);
    assert_eq!(get(Aggregate::Last), vec![20.0]);
    assert_eq!(get(Aggregate::Rmssd), vec![4.0]);
    let std = get(Aggregate::Std)[0];
    assert!((std - 7.483_314_773_547_883).abs() < 1e-9, "{std}");
    let series = build_feature(&spec(Aggregate::Series, 3, Some(3)), &s, now)
        .unwrap()
        .0;
    assert_eq!(series, vec![12.0, 16.0, 20.0]);
}

#[test]
fn f_ml_01_gap_inside_window_gives_no_feature() {
    let mut v = even(100, 108, 2, |_| 60.0);
    v.extend(even(115, 120, 1, |_| 60.0)); // 7 s gap
    let err = build_feature(&spec(Aggregate::Mean, 2, None), &hr(&v), TaiNs(120 * NS)).unwrap_err();
    assert!(
        matches!(err, MissingData::Gap { gap_ms: 7000, .. }),
        "{err}"
    );
}

#[test]
fn f_ml_01_stale_or_late_starting_data_gives_no_feature() {
    // Ends 6 s before now.
    let v = even(100, 114, 2, |_| 60.0);
    let err = build_feature(&spec(Aggregate::Mean, 2, None), &hr(&v), TaiNs(120 * NS)).unwrap_err();
    assert!(matches!(err, MissingData::Gap { .. }));
    // Starts 8 s into the window.
    let v = even(108, 120, 2, |_| 60.0);
    let err = build_feature(&spec(Aggregate::Mean, 2, None), &hr(&v), TaiNs(120 * NS)).unwrap_err();
    assert!(matches!(err, MissingData::Gap { .. }));
}

#[test]
fn f_ml_01_too_few_samples_and_no_samples_give_no_feature() {
    let v = even(100, 120, 5, |_| 60.0);
    let err =
        build_feature(&spec(Aggregate::Mean, 10, None), &hr(&v), TaiNs(120 * NS)).unwrap_err();
    assert!(matches!(
        err,
        MissingData::TooFewSamples {
            found: 5,
            needed: 10,
            ..
        }
    ));
    let err = build_feature(&spec(Aggregate::Mean, 1, None), &[], TaiNs(120 * NS)).unwrap_err();
    assert!(matches!(err, MissingData::TooFewSamples { found: 0, .. }));
}

#[test]
fn f_ml_01_sr_005_poor_quality_and_non_finite_values_are_gaps_not_values() {
    let mut s = hr(&even(100, 120, 2, |_| 60.0));
    for x in s
        .iter_mut()
        .filter(|x| (106..=114).contains(&(x.at.0 / NS)))
    {
        x.quality = Quality::Poor;
    }
    let err = build_feature(&spec(Aggregate::Mean, 2, None), &s, TaiNs(120 * NS)).unwrap_err();
    assert!(matches!(err, MissingData::Gap { .. }), "{err}");
    let mut s = hr(&even(100, 120, 2, |_| 60.0));
    for x in s
        .iter_mut()
        .filter(|x| (106..=114).contains(&(x.at.0 / NS)))
    {
        x.value = f64::NAN;
    }
    let err = build_feature(&spec(Aggregate::Mean, 2, None), &s, TaiNs(120 * NS)).unwrap_err();
    assert!(matches!(err, MissingData::Gap { .. }), "{err}");
    // Degraded readings are used, and the vector reports the worst quality.
    let mut s = hr(&even(100, 120, 2, |_| 60.0));
    s[3].quality = Quality::Degraded;
    let (_, q) = build_feature(&spec(Aggregate::Mean, 2, None), &s, TaiNs(120 * NS)).unwrap();
    assert_eq!(q, Quality::Degraded);
}

#[test]
fn f_ml_01_values_beyond_f32_are_not_representable() {
    let s = hr(&even(100, 120, 2, |_| 1e300));
    let err = build_feature(&spec(Aggregate::Mean, 2, None), &s, TaiNs(120 * NS)).unwrap_err();
    assert!(matches!(err, MissingData::NotRepresentable { .. }));
}

#[test]
fn f_ml_01_future_and_other_parameter_samples_are_ignored() {
    let mut s = hr(&even(100, 120, 2, |_| 60.0));
    s.push(sample(Parameter::HeartRate, 500.0, 130, Quality::Good));
    s.push(sample(Parameter::Spo2, 90.0, 110, Quality::Good));
    let (v, _) = build_feature(&spec(Aggregate::Max, 2, None), &s, TaiNs(120 * NS)).unwrap();
    assert_eq!(v, vec![60.0]);
}

#[test]
fn f_ml_01_one_missing_feature_means_no_vector() {
    let m = caelum_ml::manifest::Manifest::parse(&manifest_text(
        &"a".repeat(64),
        "1.0.0",
        "NOT_VALIDATED",
    ))
    .unwrap();
    // Heart rate present, RR intervals absent: no vector at all.
    let only_hr: Vec<_> = samples(0, 60, 70.0)
        .into_iter()
        .filter(|s| s.parameter == Parameter::HeartRate)
        .collect();
    let err = features::build(&m.features, &only_hr, TaiNs(60 * NS)).unwrap_err();
    assert!(matches!(err, MissingData::TooFewSamples { ref feature, .. } if feature == "rr_rmssd"));
    let v = features::build(&m.features, &samples(0, 60, 70.0), TaiNs(60 * NS)).unwrap();
    assert_eq!(v.values, vec![70.0, 60.0]);
    assert_eq!(v.slots.len(), 2);
    assert_eq!(v.slots[1].value, Some(60.0));
}
