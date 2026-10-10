//! SR-002 (M12 gate): model outputs can add observe-level notices but never raise, lower, delay
//! or suppress an absolute-limit alarm. Adversarial model outputs (NaN, ±inf, huge, negative,
//! out of range, timeouts, panics, empty, forged results) run alongside a scenario with critical
//! alarms; every alarm decision except those of the observe-level rule `af.observe` must be
//! identical with and without the models.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use caelum_alarms::engine::{Engine, Event, Input};
use caelum_alarms::ruleset::{Priority, Ruleset};
use caelum_alarms::signing::{built_in_keys, verify, TrustPolicy};
use caelum_alarms::{BUNDLED_RULESET, BUNDLED_SIGNATURES};
use caelum_domain::ids::AlarmId;
use caelum_domain::observation::{MeasurementMode, Quality};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_ml::advisory::{
    admissible, AdvisoryNotice, AdvisoryResult, GateRefusal, ObserveOnlyInput,
};
use caelum_ml::features::Sample;
use caelum_ml::registry::Registry;
use caelum_ml::runtime::RuntimeFactory;
use common::*;
use proptest::prelude::*;

const W: [f32; 2] = [0.05, 0.05];

fn bundled() -> Ruleset {
    verify(
        BUNDLED_RULESET.as_bytes(),
        BUNDLED_SIGNATURES,
        &built_in_keys(),
        TrustPolicy::Development,
    )
    .unwrap()
    .ruleset
}

fn engine() -> Engine {
    let counter = Arc::new(AtomicU64::new(1));
    Engine::with_ids(
        bundled(),
        Box::new(move || {
            AlarmId::from_uuid(uuid::Uuid::from_u128(u128::from(
                counter.fetch_add(1, Ordering::SeqCst),
            )))
        }),
    )
}

fn input(parameter: Parameter, value: f64, t: i64) -> Input {
    Input {
        crew: crew(),
        device: None,
        parameter,
        value,
        at: TaiNs(t * NS),
        quality: Quality::Good,
        mode: MeasurementMode::Continuous,
        site: None,
    }
}

/// One step of the scenario: physiology with absolute-limit breaches (HR > 180 at 200–320 s,
/// SpO2 < 85 at 500–560 s, no QRS at 700–720 s with good ECG quality).
fn step(t: i64) -> (Vec<Input>, bool) {
    let hr = if (200..320).contains(&t) { 190.0 } else { 72.0 };
    let spo2 = if (500..560).contains(&t) { 80.0 } else { 97.0 };
    let mut v = vec![
        input(Parameter::HeartRate, hr, t),
        input(Parameter::Spo2, spo2, t),
        input(Parameter::ActivityLevel, 0.1, t),
        input(Parameter::SqiEcg, 0.9, t),
    ];
    if t % 2 == 0 {
        v.push(input(
            Parameter::RrInterval,
            if t % 4 == 0 { 800.0 } else { 860.0 },
            t,
        ));
    }
    (v, !(700..720).contains(&t))
}

/// A comparable record of an event: alarm IDs replaced by rule IDs.
fn project(ev: &Event, rules: &mut HashMap<AlarmId, String>) -> String {
    let alarm =
        |a: &caelum_alarms::engine::Alarm, kind: &str, rules: &mut HashMap<AlarmId, String>| {
            rules.insert(a.id, a.rule_id.clone());
            let mut v = serde_json::to_value(a).unwrap();
            v.as_object_mut().unwrap().remove("id");
            format!("{kind} {v}")
        };
    match ev {
        Event::Confirming(a) => alarm(a, "confirming", rules),
        Event::Raised(a) => alarm(a, "raised", rules),
        Event::NotConfirmed(a) => alarm(a, "not_confirmed", rules),
        Event::ClearedAwaitingAck(a) => alarm(a, "cleared", rules),
        Event::Reactivated(a) => alarm(a, "reactivated", rules),
        Event::Resolved { alarm, reason } => format!("resolved {} {reason}", rules[alarm]),
        Event::Superseded { alarm, by } => format!("superseded {} by {}", rules[alarm], rules[by]),
        Event::SignalUnusable {
            parameter, quality, ..
        } => {
            format!("unusable {parameter:?} {quality:?}")
        }
        Event::SignalRecovered { parameter, .. } => format!("recovered {parameter:?}"),
    }
}

fn is_model_rule(record: &str) -> bool {
    record.contains("af.observe") || record.contains("AfProbability")
}

/// Runs the scenario; `models` is called at every 10 s with the engine and the samples so far.
fn run(
    mut models: impl FnMut(i64, &[Sample], &mut Engine) -> Vec<Event>,
) -> (Vec<String>, Vec<String>) {
    let mut e = engine();
    let mut rules = HashMap::new();
    let mut records = Vec::new();
    let mut model_records = Vec::new();
    let mut history: Vec<Sample> = Vec::new();
    for t in 0..=900 {
        let (inputs, qrs) = step(t);
        let mut events = Vec::new();
        for i in &inputs {
            history.push(Sample {
                parameter: i.parameter,
                value: i.value,
                at: i.at,
                quality: i.quality,
            });
            events.extend(e.evaluate(i, None));
        }
        if qrs {
            events.extend(e.event(crew(), "qrs", TaiNs(t * NS)));
        }
        events.extend(e.tick(TaiNs(t * NS)));
        if t % 10 == 0 {
            let start = history.partition_point(|s| s.at.0 < (t - 120) * NS);
            history.drain(..start);
            for ev in models(t, &history, &mut e) {
                model_records.push(project(&ev, &mut rules));
            }
        }
        for ev in &events {
            records.push(project(ev, &mut rules));
        }
    }
    (records, model_records)
}

fn registries() -> Vec<Registry> {
    let factories: Vec<Box<dyn RuntimeFactory>> = vec![
        Box::new(LinearFactory),
        Box::new(AdversarialFactory(Adversary::Constant(f32::NAN))),
        Box::new(AdversarialFactory(Adversary::Constant(f32::INFINITY))),
        Box::new(AdversarialFactory(Adversary::Constant(f32::NEG_INFINITY))),
        Box::new(AdversarialFactory(Adversary::Constant(1e30))),
        Box::new(AdversarialFactory(Adversary::Constant(-1.0))),
        Box::new(AdversarialFactory(Adversary::Constant(0.999))),
        Box::new(AdversarialFactory(Adversary::Constant(0.0))),
        Box::new(AdversarialFactory(Adversary::FirstThen(0.95, f32::NAN))),
        Box::new(AdversarialFactory(Adversary::Hang(Duration::from_millis(
            1000,
        )))),
        Box::new(AdversarialFactory(Adversary::Panic)),
        Box::new(AdversarialFactory(Adversary::Empty)),
        Box::new(AdversarialFactory(Adversary::Many(0.97))),
    ];
    factories
        .iter()
        .map(|f| {
            let mut r = Registry::new();
            r.install(verified(&W, -6.0, "1.0.0"), f.as_ref()).unwrap();
            r
        })
        .collect()
}

/// Forged results: a caller that builds results by hand still cannot reach other rules.
fn forged(t: i64, value: f64, parameter: Parameter) -> AdvisoryResult {
    AdvisoryResult {
        model_id: "forged".to_owned(),
        model_version: "0".to_owned(),
        validation: caelum_ml::manifest::ValidationStatus::Validated,
        status: caelum_ml::advisory::DisplayStatus::Validated,
        development_signed: false,
        crew: crew(),
        output: "x".to_owned(),
        label: "x".to_owned(),
        value,
        at: TaiNs(t * NS),
        quality: Quality::Good,
        contributions: vec![],
        parameter: Some(parameter),
        notice: None,
    }
}

#[test]
fn sr_002_adversarial_model_outputs_never_change_absolute_alarm_decisions() {
    let (baseline, none) = run(|_, _, _| Vec::new());
    assert!(none.is_empty());
    // The scenario does exercise critical alarms.
    for rule in ["hr.critical.high", "spo2.critical", "asystole.critical"] {
        assert!(
            baseline
                .iter()
                .any(|r| r.starts_with("raised") && r.contains(rule)),
            "{rule} not raised: {baseline:#?}"
        );
    }

    // One model reports af_probability at a time (as in operation); each adversary gets its own
    // run, compared with the run without models.
    let strip =
        |v: &[String]| -> Vec<String> { v.iter().filter(|r| !is_model_rule(r)).cloned().collect() };
    let mut notices = 0;
    let mut admitted = 0;
    let mut af_raised = Vec::new();
    for (k, r) in registries().iter().enumerate() {
        let (with_models, model_events) = run(|t, history, e| {
            let mut events = Vec::new();
            if let Ok(results) = r.evaluate("T2", crew(), history, TaiNs(t * NS)) {
                for res in results {
                    assert!(res.value.is_finite() && (0.0..=1.0).contains(&res.value));
                    assert!(!res.contributions.is_empty(), "F-ML-04: explained");
                    if let Some(n) = AdvisoryNotice::from_result(&res) {
                        assert_eq!(n.level().priority(), Priority::Observe);
                        notices += 1;
                    }
                    let gated = ObserveOnlyInput::new(&res, None).expect("af_probability admitted");
                    assert_eq!(gated.parameter(), Parameter::AfProbability);
                    events.extend(
                        gated
                            .apply(e)
                            .expect("bundled ruleset admits af_probability"),
                    );
                    admitted += 1;
                }
            }
            // Forged results aimed at absolute-limit rules and their context are refused.
            for (p, v) in [
                (Parameter::HeartRate, 30.0),
                (Parameter::Spo2, 70.0),
                (Parameter::SqiEcg, 0.0),
                (Parameter::ActivityLevel, 1.0),
                (Parameter::CoreTemp, 41.0),
            ] {
                let refused = ObserveOnlyInput::new(&forged(t, v, p), None);
                assert!(
                    matches!(refused, Err(GateRefusal::NotModelOutput(_))),
                    "{p:?}"
                );
            }
            for v in [f64::NAN, f64::INFINITY, -1.0, 1e30] {
                assert_eq!(
                    ObserveOnlyInput::new(&forged(t, v, Parameter::AfProbability), None),
                    Err(GateRefusal::Value)
                );
            }
            events
        });
        assert_eq!(
            strip(&with_models),
            strip(&baseline),
            "SR-002: absolute-limit alarm decisions changed by model outputs (adversary {k})"
        );
        assert_eq!(
            strip(&model_events),
            Vec::<String>::new(),
            "models touched other rules (adversary {k})"
        );
        af_raised.extend(
            model_events
                .into_iter()
                .chain(with_models)
                .filter(|r| r.contains("af.observe") && r.starts_with("raised")),
        );
    }
    // The models did produce admitted observe-level output (the comparison is not vacuous).
    assert!(
        admitted > 0 && notices > 0,
        "admitted {admitted}, notices {notices}"
    );
    assert!(!af_raised.is_empty(), "af.observe raised from model output");
    assert!(
        af_raised
            .iter()
            .all(|r| r.contains("\"priority\":\"observe\"")),
        "{af_raised:?}"
    );
}

#[test]
fn sr_002_gate_refuses_parameters_with_non_observe_rules_or_context_roles() {
    let rs = bundled();
    assert_eq!(admissible(&rs, Parameter::AfProbability), Ok(()));
    assert!(admissible(&rs, Parameter::HeartRate).is_err());
    assert!(admissible(&rs, Parameter::SqiEcg).is_err());
    assert!(admissible(&rs, Parameter::ActivityLevel).is_err());
    // A ruleset that promoted af.observe to warning (needs a CR) is refused at apply time.
    let mut promoted = rs.clone();
    for r in promoted.rules.iter_mut().filter(|r| r.id == "af.observe") {
        r.priority = Priority::Warning;
    }
    assert!(matches!(
        admissible(&promoted, Parameter::AfProbability),
        Err(GateRefusal::NotObserveOnly { .. })
    ));
    let mut e = Engine::new(promoted);
    let input = ObserveOnlyInput::new(&forged(0, 0.99, Parameter::AfProbability), None).unwrap();
    assert!(input.apply(&mut e).is_err());
    assert_eq!(e.open_alarms().count(), 0);
    // af_probability sharing a group with an absolute rule is refused.
    let mut shared = rs.clone();
    for r in shared.rules.iter_mut().filter(|r| r.id == "af.observe") {
        r.group = "heart_rate".to_owned();
    }
    assert!(matches!(
        admissible(&shared, Parameter::AfProbability),
        Err(GateRefusal::SharedGroup { .. })
    ));
    // af_probability used as an absence gate is refused.
    let mut gated = rs;
    for r in gated.rules.iter_mut() {
        if let caelum_alarms::ruleset::Condition::Absence(a) = &mut r.condition {
            a.gate.0 = Parameter::AfProbability;
        }
    }
    assert!(admissible(&gated, Parameter::AfProbability).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// SR-002 property: any sequence of model values (any f64 bits) and any forged target
    /// parameter leaves every non-model alarm decision unchanged.
    #[test]
    fn sr_002_property_any_model_values_leave_alarms_unchanged(
        bits in proptest::collection::vec(any::<u64>(), 91),
        targets in proptest::collection::vec(0usize..6, 91),
    ) {
        let params = [
            Parameter::AfProbability,
            Parameter::HeartRate,
            Parameter::SqiEcg,
            Parameter::RhythmClass,
            Parameter::BpTrendSystolic,
            Parameter::SleepStage,
        ];
        let (baseline, _) = run(|_, _, _| Vec::new());
        let (with_models, model_events) = run(|t, _, e| {
            let i = (t / 10) as usize;
            let value = f64::from_bits(bits[i]);
            let res = forged(t, value, params[targets[i]]);
            match ObserveOnlyInput::new(&res, None) {
                Ok(g) => g.apply(e).unwrap_or_default(),
                Err(_) => Vec::new(),
            }
        });
        let strip = |v: &[String]| -> Vec<String> {
            v.iter().filter(|r| !is_model_rule(r)).cloned().collect()
        };
        prop_assert_eq!(strip(&with_models), strip(&baseline));
        prop_assert!(strip(&model_events).is_empty());
    }
}
