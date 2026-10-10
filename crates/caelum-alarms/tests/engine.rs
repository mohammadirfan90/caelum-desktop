//! Rule engine tests (M3 gate; F-ALM-01 deterministic engine, F-ALM-08 baseline notices):
//! table-driven over every rule of the bundled ruleset, named tests for SR-001..006, SR-010,
//! SR-018 and SR-019, and property tests.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use caelum_alarms::engine::{AlarmState, BaselineStats, Engine, Event, Input};
use caelum_alarms::ruleset::{Condition, Op, Rule, Ruleset, ThresholdLimits};
use caelum_alarms::signing::{built_in_keys, verify, TrustPolicy};
use caelum_alarms::{BUNDLED_RULESET, BUNDLED_SIGNATURES};
use caelum_domain::ids::{AlarmId, CrewMemberId, DeviceId};
use caelum_domain::observation::{MeasurementMode, Quality, SiteClass};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use proptest::prelude::*;

const S: i64 = 1_000_000_000;
const T0: i64 = 2_000_000_000 * S;

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

fn engine_with(rules: impl Fn(&Rule) -> bool) -> Engine {
    let mut rs = bundled();
    rs.rules.retain(|r| rules(r));
    let counter = Arc::new(AtomicU64::new(1));
    Engine::with_ids(
        rs,
        Box::new(move || {
            AlarmId::from_uuid(uuid::Uuid::from_u128(u128::from(
                counter.fetch_add(1, Ordering::SeqCst),
            )))
        }),
    )
}

fn only(id: &'static str) -> Engine {
    engine_with(move |r| r.id == id)
}

fn crew() -> CrewMemberId {
    CrewMemberId::from_uuid(uuid::Uuid::from_u128(7))
}

fn input(parameter: Parameter, value: f64, t_s: i64, mode: MeasurementMode) -> Input {
    Input {
        crew: crew(),
        device: Some(DeviceId::from_uuid(uuid::Uuid::from_u128(9))),
        parameter,
        value,
        at: TaiNs(T0 + t_s * S),
        quality: Quality::Good,
        mode,
        site: None,
    }
}

fn cont(p: Parameter, v: f64, t: i64) -> Input {
    input(p, v, t, MeasurementMode::Continuous)
}

fn spot(p: Parameter, v: f64, t: i64) -> Input {
    input(p, v, t, MeasurementMode::Spot)
}

/// Feeds one reading per second from `from` to `to` (inclusive); returns all events.
fn stream(e: &mut Engine, p: Parameter, v: f64, from: i64, to: i64) -> Vec<(i64, Event)> {
    (from..=to)
        .flat_map(|t| {
            e.evaluate(&cont(p, v, t), None)
                .into_iter()
                .map(move |ev| (t, ev))
        })
        .collect()
}

fn raised_at(events: &[(i64, Event)]) -> Option<i64> {
    events
        .iter()
        .find(|(_, e)| matches!(e, Event::Raised(_)))
        .map(|(t, _)| *t)
}

fn fixed(rule: &Rule) -> Option<(Op, f64, f64)> {
    match &rule.condition {
        Condition::Threshold(t) => match t.limits {
            ThresholdLimits::Fixed(l) => Some((t.op, l.fire, l.clear)),
            ThresholdLimits::BySite(_) => None,
        },
        _ => None,
    }
}

/// A value that breaches `fire` by a margin, a value between the thresholds, and the clear value.
fn values(op: Op, fire: f64, clear: f64) -> (f64, Option<f64>, f64) {
    let margin = (fire - clear).abs().max(0.1);
    let breach = match op {
        Op::Below => fire - margin.min(fire.abs() * 0.5).max(0.05),
        Op::Above => fire + margin,
    };
    let between = ((fire - clear).abs() > 1e-9).then(|| (fire + clear) / 2.0);
    (breach, between, clear)
}

/// SR-001, SR-003: for every continuous threshold rule, the alarm fires exactly when the
/// persistence window is met (never before), values between the thresholds never clear it, and the
/// clear threshold held for the clear window clears it (latched rules wait for acknowledgement).
#[test]
fn sr_001_sr_003_every_continuous_rule_fires_after_persistence_and_clears_with_hysteresis() {
    let rules: Vec<Rule> = bundled()
        .rules
        .into_iter()
        .filter(|r| fixed(r).is_some())
        .collect();
    assert!(rules.len() >= 12, "{}", rules.len());
    for rule in rules {
        let (op, fire, clear) = fixed(&rule).unwrap();
        let (breach, between, clear_v) = values(op, fire, clear);
        let id = rule.id.clone();
        let mut e = engine_with(|r| r.id == id);
        let p = rule.parameter;
        // Shorter than the persistence window: no alarm.
        let window = i64::from(rule.fire_for_s);
        let early = stream(&mut e, p, breach, 0, window - 1);
        assert_eq!(
            raised_at(&early),
            None,
            "{}: fired before its window",
            rule.id
        );
        let at = stream(&mut e, p, breach, window, window);
        assert_eq!(
            raised_at(&at),
            Some(window),
            "{}: must fire at {window} s",
            rule.id
        );
        let mut t = window + 1;
        if let Some(b) = between {
            let ev = stream(&mut e, p, b, t, t + i64::from(rule.clear_for_s) + 5);
            assert!(
                ev.is_empty(),
                "{}: cleared between thresholds: {ev:?}",
                rule.id
            );
            t += i64::from(rule.clear_for_s) + 6;
        }
        let clear_for = i64::from(rule.clear_for_s);
        let early_clear = stream(&mut e, p, clear_v, t, t + clear_for - 1);
        assert!(early_clear.is_empty(), "{}: cleared too early", rule.id);
        let cleared = stream(&mut e, p, clear_v, t + clear_for, t + clear_for);
        let expected_latched = rule.priority.latching();
        match &cleared[..] {
            [(_, Event::ClearedAwaitingAck(a))] => {
                assert!(expected_latched, "{}", rule.id);
                assert_eq!(a.state, AlarmState::ClearedAwaitingAck);
                let ack = e.acknowledge(a.id);
                assert!(matches!(&ack[..], [Event::Resolved { .. }]), "{}", rule.id);
            }
            [(_, Event::Resolved { .. })] => assert!(!expected_latched, "{}", rule.id),
            other => panic!("{}: unexpected {other:?}", rule.id),
        }
        assert_eq!(e.open_alarms().count(), 0, "{}", rule.id);
    }
}

/// SR-004: a data gap longer than max_gap_s restarts persistence timing.
#[test]
fn sr_004_a_data_gap_restarts_persistence() {
    let mut e = only("spo2.critical");
    assert!(stream(&mut e, Parameter::Spo2, 80.0, 0, 10).is_empty());
    // 31 s gap (> max_gap_s 30): timing restarts at 41.
    assert!(stream(&mut e, Parameter::Spo2, 80.0, 41, 55).is_empty());
    assert_eq!(
        raised_at(&stream(&mut e, Parameter::Spo2, 80.0, 56, 56)),
        Some(56)
    );
    // A 30 s gap does not restart.
    let mut e = only("spo2.critical");
    stream(&mut e, Parameter::Spo2, 80.0, 0, 5);
    assert_eq!(
        raised_at(&stream(&mut e, Parameter::Spo2, 80.0, 35, 35)),
        Some(35)
    );
}

/// SR-005: POOR and UNUSABLE readings pause rules and raise one technical event; a usable reading
/// recovers the signal.
#[test]
fn sr_005_poor_quality_pauses_rules_and_raises_a_technical_event() {
    let mut e = only("spo2.critical");
    let mut events = Vec::new();
    for t in 0..60 {
        let mut i = cont(Parameter::Spo2, 70.0, t);
        i.quality = if t % 2 == 0 {
            Quality::Poor
        } else {
            Quality::Unusable
        };
        events.extend(e.evaluate(&i, None));
    }
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(
        events[0],
        Event::SignalUnusable {
            quality: Quality::Poor,
            ..
        }
    ));
    let ev = e.evaluate(&cont(Parameter::Spo2, 97.0, 61), None);
    assert!(matches!(ev[..], [Event::SignalRecovered { .. }]));
    // Degraded readings are still evaluated.
    let mut e = only("spo2.critical");
    let mut fired = false;
    for t in 0..=15 {
        let mut i = cont(Parameter::Spo2, 80.0, t);
        i.quality = Quality::Degraded;
        fired |= e
            .evaluate(&i, None)
            .iter()
            .any(|ev| matches!(ev, Event::Raised(_)));
    }
    assert!(fired);
}

/// SR-006: a higher priority supersedes lower alarms in its group; a lower one is not opened while
/// it is open and opens afterwards if still breaching.
#[test]
fn sr_006_higher_priority_supersedes_within_the_group() {
    let mut e = engine_with(|r| r.id.starts_with("spo2."));
    // 88 %: warning only (fires at 60 s).
    let ev = stream(&mut e, Parameter::Spo2, 88.0, 0, 60);
    let warning = match ev.iter().find(|(_, e)| matches!(e, Event::Raised(_))) {
        Some((_, Event::Raised(a))) => a.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(warning.rule_id, "spo2.warning");
    // 80 %: critical after 15 s supersedes the warning.
    let ev = stream(&mut e, Parameter::Spo2, 80.0, 61, 76);
    assert!(
        ev.iter()
            .any(|(_, e)| matches!(e, Event::Superseded { alarm, .. } if *alarm == warning.id)),
        "{ev:?}"
    );
    let open: Vec<_> = e.open_alarms().map(|a| a.rule_id.clone()).collect();
    assert_eq!(open, ["spo2.critical"]);
    // Critical cleared and acknowledged; still 88 %: the warning opens again.
    let ev = stream(&mut e, Parameter::Spo2, 90.0, 77, 137);
    let crit = e.open_alarms().next().unwrap().id;
    assert!(ev
        .iter()
        .any(|(_, e)| matches!(e, Event::ClearedAwaitingAck(_))));
    e.acknowledge(crit);
    let ev = stream(&mut e, Parameter::Spo2, 88.0, 138, 200);
    assert_eq!(raised_at(&ev).map(|t| t >= 138), Some(true));
    assert_eq!(
        e.open_alarms()
            .map(|a| a.rule_id.as_str())
            .collect::<Vec<_>>(),
        ["spo2.warning"]
    );
}

/// SR-006: core_temp and body_temp_measured rules supersede each other in the THERMAL group.
#[test]
fn sr_006_thermal_group_spans_core_and_measured_temperature() {
    let mut e = engine_with(|r| r.group == "THERMAL");
    let ev = e.evaluate(&spot(Parameter::BodyTempMeasured, 38.3, 0), None);
    let warn = match &ev[..] {
        [Event::Confirming(a)] => a.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(warn.rule_id, "bodytemp.warning");
    let ev = stream(&mut e, Parameter::CoreTemp, 39.8, 1, 301);
    assert!(
        ev.iter()
            .any(|(_, e)| matches!(e, Event::Superseded { alarm, .. } if *alarm == warn.id)),
        "{ev:?}"
    );
    assert!(e.open_alarms().all(|a| a.rule_id == "temp.critical"));
}

/// SR-002, R-ALM-06: baseline notices are observe-level: an absolute alarm supersedes them and they never
/// block or delay it.
#[test]
fn sr_002_baselines_never_suppress_or_delay_absolute_alarms() {
    let mut e = only("spo2.critical");
    let b = BaselineStats {
        median: 98.0,
        mad: 0.5,
        n: 500,
    };
    let mut events = Vec::new();
    let mut act = cont(Parameter::ActivityLevel, 0.1, 0);
    e.evaluate(&act, None);
    for t in 0..=600 {
        if t % 60 == 0 {
            act.at = TaiNs(T0 + t * S);
            e.evaluate(&act, None);
        }
        events.extend(
            e.evaluate(&cont(Parameter::Spo2, 94.0, t), Some(b))
                .into_iter()
                .map(|ev| (t, ev)),
        );
    }
    let notice = match events.iter().find(|(_, e)| matches!(e, Event::Raised(_))) {
        Some((t, Event::Raised(a))) => {
            assert_eq!(*t, 600);
            a.clone()
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(
        (notice.rule_id.as_str(), notice.priority, notice.category),
        ("baseline.spo2", "observe", "RESPIRATORY")
    );
    // Now an absolute breach: it fires on time and supersedes the notice.
    let mut ev = Vec::new();
    for t in 601..=616 {
        ev.extend(
            e.evaluate(&cont(Parameter::Spo2, 80.0, t), Some(b))
                .into_iter()
                .map(|x| (t, x)),
        );
    }
    assert_eq!(raised_at(&ev), Some(616), "absolute rule delayed: {ev:?}");
    assert!(ev
        .iter()
        .any(|(_, e)| matches!(e, Event::Superseded { alarm, .. } if *alarm == notice.id)));
}

/// SR-010: why records rule id, version, values, duration, device and quality.
#[test]
fn sr_010_why_records_rule_version_values_duration_device_and_quality() {
    let mut e = only("hr.critical.high");
    let ev = stream(&mut e, Parameter::HeartRate, 190.0, 0, 30);
    let alarm = match ev.last() {
        Some((_, Event::Raised(a))) => a.clone(),
        other => panic!("{other:?}"),
    };
    let w = &alarm.why;
    assert_eq!(
        (w.rule_id.as_str(), w.ruleset_version.as_str()),
        ("hr.critical.high", "1.0.0-draft.3")
    );
    assert_eq!(
        (w.parameter, w.unit, w.threshold, w.op),
        ("heart_rate", "/min", Some(180.0), Some(">"))
    );
    assert_eq!(w.held_s, Some(30.0));
    assert_eq!(w.readings[0].value, 190.0);
    assert_eq!(w.readings[0].quality, "good");
    assert!(w.readings[0].device.is_some());
    assert_eq!(alarm.trigger_value, Some(190.0));
    let json = serde_json::to_string(w).unwrap();
    assert!(json.contains("\"rule_id\":\"hr.critical.high\""));
}

/// SR-018: unknown or stale activity is not a default: "at rest" rules are evaluated and the why
/// says activity is unknown; known activity above the limit means the rule does not apply.
#[test]
fn sr_018_unknown_context_is_evaluated_and_stated() {
    let mut e = only("hr.warning.high");
    let ev = stream(&mut e, Parameter::HeartRate, 140.0, 0, 120);
    let a = match ev.last() {
        Some((120, Event::Raised(a))) => a.clone(),
        other => panic!("{other:?}"),
    };
    assert!(a.why.notes.iter().any(|n| n.contains("activity unknown")));

    let mut e = only("hr.warning.high");
    for t in (0..=240).step_by(30) {
        e.evaluate(&cont(Parameter::ActivityLevel, 0.8, t), None);
    }
    let mut ev = Vec::new();
    for t in 0..=240 {
        if t % 30 == 0 {
            e.evaluate(&cont(Parameter::ActivityLevel, 0.8, t), None);
        }
        ev.extend(e.evaluate(&cont(Parameter::HeartRate, 140.0, t), None));
    }
    assert!(
        ev.is_empty(),
        "exercising: the at-rest rule does not apply: {ev:?}"
    );

    // Activity reported once, then stale (> 120 s): unknown again, evaluated.
    let mut e = only("hr.warning.high");
    e.evaluate(&cont(Parameter::ActivityLevel, 0.8, 0), None);
    let ev = stream(&mut e, Parameter::HeartRate, 140.0, 121, 241);
    assert_eq!(raised_at(&ev), Some(241));
}

fn spot_rules() -> Vec<Rule> {
    bundled()
        .rules
        .into_iter()
        .filter(|r| matches!(&r.condition, Condition::Threshold(t) if t.spot.is_some()))
        .collect()
}

fn spot_breach(rule: &Rule) -> (f64, f64, Option<SiteClass>) {
    let Condition::Threshold(t) = &rule.condition else {
        unreachable!()
    };
    let l = match t.limits {
        ThresholdLimits::Fixed(l) => l,
        ThresholdLimits::BySite(s) => s.core_equivalent,
    };
    let site = matches!(t.limits, ThresholdLimits::BySite(_)).then_some(SiteClass::CoreEquivalent);
    let (b, _, c) = values(t.op, l.fire, l.clear);
    (b, c, site)
}

fn spot_at(rule: &Rule, v: f64, t: i64, site: Option<SiteClass>) -> Input {
    let mut i = spot(rule.parameter, v, t);
    i.site = site;
    i
}

/// SR-019, every spot rule: breach → CONFIRMING; a breaching repeat confirms.
#[test]
fn sr_019_every_spot_rule_confirms_on_a_breaching_repeat() {
    for rule in spot_rules() {
        let id = rule.id.clone();
        let mut e = engine_with(|r| r.id == id);
        let (b, _, site) = spot_breach(&rule);
        let ev = e.evaluate(&spot_at(&rule, b, 0, site), None);
        assert!(
            matches!(&ev[..], [Event::Confirming(a)] if a.state == AlarmState::Confirming && a.priority == rule.priority.as_str()),
            "{}: {ev:?}",
            rule.id
        );
        let ev = e.evaluate(&spot_at(&rule, b, 60, site), None);
        assert!(
            matches!(&ev[..], [Event::Raised(a)] if a.why.readings.len() == 2),
            "{}: {ev:?}",
            rule.id
        );
    }
}

/// SR-019, every spot rule: no repeat within recheck_timeout_s raises the full alarm.
#[test]
fn sr_019_every_spot_rule_raises_when_no_repeat_arrives() {
    for rule in spot_rules() {
        let id = rule.id.clone();
        let Condition::Threshold(t) = &rule.condition else {
            unreachable!()
        };
        let timeout = i64::from(t.spot.unwrap().recheck_timeout_s);
        let mut e = engine_with(|r| r.id == id);
        let (b, _, site) = spot_breach(&rule);
        e.evaluate(&spot_at(&rule, b, 0, site), None);
        assert!(
            e.tick(TaiNs(T0 + (timeout - 1) * S)).is_empty(),
            "{}",
            rule.id
        );
        let ev = e.tick(TaiNs(T0 + timeout * S));
        assert!(
            matches!(&ev[..], [Event::Raised(a)] if a.why.notes.iter().any(|n| n.contains("no repeat"))),
            "{}: {ev:?}",
            rule.id
        );
    }
}

/// SR-019, every spot rule: a non-breaching repeat closes it as not confirmed, with both values;
/// a new breach within confirm_window_s of the first raises the full alarm at once.
#[test]
fn sr_019_every_spot_rule_not_confirmed_then_rebreach_within_window() {
    for rule in spot_rules() {
        let id = rule.id.clone();
        let Condition::Threshold(t) = &rule.condition else {
            unreachable!()
        };
        let window = i64::from(t.spot.unwrap().confirm_window_s);
        let mut e = engine_with(|r| r.id == id);
        let (b, c, site) = spot_breach(&rule);
        e.evaluate(&spot_at(&rule, b, 0, site), None);
        let ev = e.evaluate(&spot_at(&rule, c, 30, site), None);
        match &ev[..] {
            [Event::NotConfirmed(a)] => {
                let vals: Vec<f64> = a.why.readings.iter().map(|r| r.value).collect();
                assert_eq!(vals, [b, c], "{}", rule.id);
            }
            other => panic!("{}: {other:?}", rule.id),
        }
        assert_eq!(e.open_alarms().count(), 0);
        let ev = e.evaluate(&spot_at(&rule, b, window, site), None);
        assert!(matches!(&ev[..], [Event::Raised(_)]), "{}: {ev:?}", rule.id);
        // After the window, a breach starts confirmation again.
        let mut e = engine_with(|r| r.id == id);
        e.evaluate(&spot_at(&rule, b, 0, site), None);
        e.evaluate(&spot_at(&rule, c, 30, site), None);
        let ev = e.evaluate(&spot_at(&rule, b, window + 1, site), None);
        assert!(
            matches!(&ev[..], [Event::Confirming(_)]),
            "{}: {ev:?}",
            rule.id
        );
    }
}

/// SR-019 (CR-003 proposal): a confirmed spot alarm clears on one reading meeting the clear
/// threshold; latching rules then wait for acknowledgement.
#[test]
fn sr_019_confirmed_spot_alarm_clears_on_one_clear_reading() {
    for rule in spot_rules() {
        let id = rule.id.clone();
        let mut e = engine_with(|r| r.id == id);
        let (b, c, site) = spot_breach(&rule);
        e.evaluate(&spot_at(&rule, b, 0, site), None);
        e.evaluate(&spot_at(&rule, b, 10, site), None);
        let ev = e.evaluate(&spot_at(&rule, c, 20, site), None);
        if rule.priority.latching() {
            let a = match &ev[..] {
                [Event::ClearedAwaitingAck(a)] => a.clone(),
                other => panic!("{}: {other:?}", rule.id),
            };
            assert!(matches!(&e.acknowledge(a.id)[..], [Event::Resolved { .. }]));
        } else {
            assert!(matches!(&ev[..], [Event::Resolved { .. }]), "{}", rule.id);
        }
    }
}

/// SR-019: a spot reading on a rule without spot settings raises the full alarm at once.
#[test]
fn sr_019_spot_reading_without_spot_settings_raises_at_once() {
    let mut e = only("temp.critical");
    let ev = e.evaluate(&spot(Parameter::CoreTemp, 40.0, 0), None);
    assert!(
        matches!(&ev[..], [Event::Raised(a)] if a.state == AlarmState::Active),
        "{ev:?}"
    );
}

/// CR-003 §3: thresholds by thermometer site class; an unknown site uses the lowest threshold.
#[test]
fn measured_temperature_uses_site_thresholds_and_unknown_is_most_conservative() {
    for (site, expect) in [
        (Some(SiteClass::CoreEquivalent), false),
        (Some(SiteClass::Peripheral), true),
        (Some(SiteClass::Unknown), true),
        (None, true),
    ] {
        let mut e = only("bodytemp.warning");
        let mut i = spot(Parameter::BodyTempMeasured, 37.8, 0);
        i.site = site;
        let ev = e.evaluate(&i, None);
        assert_eq!(!ev.is_empty(), expect, "{site:?}: {ev:?}");
        if let [Event::Confirming(a)] = &ev[..] {
            if site != Some(SiteClass::Peripheral) {
                assert!(a.why.notes.iter().any(|n| n.contains("site unknown")));
            }
        }
    }
}

fn qrs(e: &mut Engine, t_ms: i64) -> Vec<Event> {
    e.event(crew(), "qrs", TaiNs(T0 + t_ms * 1_000_000))
}

fn sqi(e: &mut Engine, v: f64, t: i64) -> Vec<Event> {
    e.evaluate(&cont(Parameter::SqiEcg, v, t), None)
}

/// Absence of event: no QRS for 6 s with good ECG SQI fires on the tick; QRS for 10 s clears.
#[test]
fn absence_of_qrs_with_good_sqi_fires_on_the_tick_and_clears_with_qrs() {
    let mut e = only("asystole.critical");
    for t in 0..=5 {
        sqi(&mut e, 0.9, t);
        for k in 0..4 {
            assert!(e.tick(TaiNs(T0 + t * S + k * 250_000_000)).is_empty());
        }
    }
    // Evaluated on the gating input (and on the tick): it fires at 6 s, once.
    let mut ev = sqi(&mut e, 0.9, 6);
    ev.extend(e.tick(TaiNs(T0 + 6 * S)));
    let a = match &ev[..] {
        [Event::Raised(a)] => a.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(a.rule_id, "asystole.critical");
    assert!(a.why.notes[0].contains("no qrs"));
    // QRS every 800 ms for 10 s: the condition clears; the latched alarm waits for ack.
    let mut cleared = Vec::new();
    for k in 0..=13 {
        let t_ms = 7_000 + k * 800;
        if k % 2 == 0 {
            sqi(&mut e, 0.9, t_ms / 1_000);
        }
        cleared.extend(qrs(&mut e, t_ms));
    }
    assert!(
        matches!(&cleared[..], [Event::ClearedAwaitingAck(_)]),
        "{cleared:?}"
    );
    // Absent again before acknowledgement: active again.
    let mut ev = sqi(&mut e, 0.9, 25);
    ev.extend(e.tick(TaiNs(T0 + 25 * S)));
    assert!(
        matches!(&ev[..], [Event::Reactivated(a)] if a.state == AlarmState::Active),
        "{ev:?}"
    );
}

#[test]
fn absence_rules_wait_while_the_gate_is_closed_or_stale() {
    let mut e = only("asystole.critical");
    for t in 0..=20 {
        sqi(&mut e, 0.3, t); // poor ECG: gate closed
        assert!(e.tick(TaiNs(T0 + t * S)).is_empty());
    }
    let mut e = only("asystole.critical");
    sqi(&mut e, 0.9, 0);
    // No new SQI for > 30 s (stale_after_s): the gate is no longer valid.
    assert!(e.tick(TaiNs(T0 + 40 * S)).is_empty());
    // QRS keeps it quiet.
    let mut e = only("asystole.critical");
    for t in 0..=20 {
        sqi(&mut e, 0.9, t);
        qrs(&mut e, t * 1_000);
        assert!(e.tick(TaiNs(T0 + t * S)).is_empty());
    }
}

/// SR-005: an unusable reading of the gate parameter (ECG lead off) closes the gate at once: the
/// absence rule must not fire on the last good SQI while the electrode is disconnected.
#[test]
fn sr_005_an_unusable_gate_reading_closes_the_absence_gate() {
    let mut e = only("asystole.critical");
    for t in 0..=3 {
        sqi(&mut e, 0.9, t);
        qrs(&mut e, t * 1_000);
    }
    let mut lead_off = cont(Parameter::SqiEcg, 0.0, 4);
    lead_off.quality = Quality::Unusable;
    let ev = e.evaluate(&lead_off, None);
    assert!(matches!(ev[..], [Event::SignalUnusable { .. }]), "{ev:?}");
    for t in 4..=20 {
        assert!(
            e.tick(TaiNs(T0 + t * S)).is_empty(),
            "no asystole at {t} s with the lead off"
        );
    }
    // The lead is back with a good signal and no beats: the absence rule runs again.
    let mut fired = false;
    for t in 21..=28 {
        let mut ev = sqi(&mut e, 0.9, t);
        ev.extend(e.tick(TaiNs(T0 + t * S)));
        fired |= ev
            .iter()
            .any(|x| matches!(x, Event::Raised(a) if a.rule_id == "asystole.critical"));
    }
    assert!(fired);
}

/// F-ALM-08: spot baselines fire on k of the last n deviating readings and clear when none deviate.
#[test]
fn spot_baseline_uses_k_of_n_and_clears_when_none_deviate() {
    let mut e = engine_with(|_| false);
    let b = BaselineStats {
        median: 70.0,
        mad: 0.5,
        n: 60,
    };
    let seq = [72.0, 74.0, 70.0, 74.0, 74.0];
    let mut raised = None;
    for (t, v) in seq.iter().enumerate() {
        let ev = e.evaluate(&spot(Parameter::Weight, *v, t as i64 * 86_400), Some(b));
        if let [Event::Raised(a)] = &ev[..] {
            raised = Some((t, a.clone()));
        }
    }
    let (t, a) = raised.expect("raised");
    assert_eq!(t, 4, "3 of the last 4 deviate at the fifth reading");
    assert_eq!((a.priority, a.latched), ("observe", false));
    let mut resolved = false;
    for k in 5..9 {
        resolved |= e
            .evaluate(&spot(Parameter::Weight, 70.0, k * 86_400), Some(b))
            .iter()
            .any(|ev| matches!(ev, Event::Resolved { .. }));
    }
    assert!(resolved);
}

#[test]
fn baselines_ignore_unusable_statistics_and_exercise() {
    let mut e = engine_with(|_| false);
    let zero = BaselineStats {
        median: 70.0,
        mad: 0.0,
        n: 10,
    };
    assert!(stream_with_baseline(&mut e, Parameter::HeartRate, 120.0, 0, 700, zero).is_empty());
    let mut e = engine_with(|_| false);
    let b = BaselineStats {
        median: 60.0,
        mad: 2.0,
        n: 500,
    };
    let mut ev = Vec::new();
    for t in 0..=700 {
        if t % 60 == 0 {
            e.evaluate(&cont(Parameter::ActivityLevel, 0.9, t), None);
        }
        ev.extend(e.evaluate(&cont(Parameter::HeartRate, 120.0, t), Some(b)));
    }
    assert!(ev.is_empty(), "heart-rate baseline is at rest only: {ev:?}");
    // Not a baseline parameter.
    let mut e = engine_with(|_| false);
    assert!(stream_with_baseline(&mut e, Parameter::BpSystolic, 200.0, 0, 700, b).is_empty());
}

fn stream_with_baseline(
    e: &mut Engine,
    p: Parameter,
    v: f64,
    from: i64,
    to: i64,
    b: BaselineStats,
) -> Vec<Event> {
    (from..=to)
        .flat_map(|t| e.evaluate(&cont(p, v, t), Some(b)))
        .collect()
}

/// Continuous baseline: |z| ≥ 4 for 600 s fires an observe notice; |z| ≤ 2.5 for 600 s ends it.
#[test]
fn continuous_baseline_fires_and_resolves_without_acknowledgement() {
    let mut e = engine_with(|_| false);
    let b = BaselineStats {
        median: 40.0,
        mad: 2.0,
        n: 500,
    };
    let ev = stream_with_baseline(&mut e, Parameter::HrvRmssd, 10.0, 0, 600, b);
    assert!(
        matches!(ev.last(), Some(Event::Raised(a)) if a.rule_id == "baseline.hrv_rmssd" && a.category == "CARDIO"),
        "{ev:?}"
    );
    assert!(
        stream_with_baseline(&mut e, Parameter::HrvRmssd, 31.0, 601, 1_500, b).is_empty(),
        "between thresholds"
    );
    let ev = stream_with_baseline(&mut e, Parameter::HrvRmssd, 40.0, 1_501, 2_101, b);
    assert!(matches!(ev.last(), Some(Event::Resolved { .. })), "{ev:?}");
}

/// Latched alarms acknowledged before the condition clears end when it clears.
#[test]
fn acknowledged_before_clearing_resolves_on_clear() {
    let mut e = only("spo2.critical");
    stream(&mut e, Parameter::Spo2, 80.0, 0, 15);
    let id = e.open_alarms().next().unwrap().id;
    assert!(e.acknowledge(id).is_empty(), "still breaching: stays open");
    let ev = stream(&mut e, Parameter::Spo2, 95.0, 16, 76);
    assert!(
        matches!(ev.last(), Some((_, Event::Resolved { .. }))),
        "{ev:?}"
    );
    assert!(
        e.acknowledge(id).is_empty(),
        "unknown alarm: nothing happens"
    );
}

#[test]
fn rebreach_after_clearing_reactivates_and_non_finite_values_are_ignored() {
    let mut e = only("spo2.critical");
    stream(&mut e, Parameter::Spo2, 80.0, 0, 15);
    let ev = stream(&mut e, Parameter::Spo2, 95.0, 16, 76);
    assert!(matches!(ev.last(), Some((_, Event::ClearedAwaitingAck(_)))));
    let ev = stream(&mut e, Parameter::Spo2, 80.0, 77, 77);
    assert!(matches!(&ev[..], [(_, Event::Reactivated(a))] if a.state == AlarmState::Active));
    assert!(e
        .evaluate(&cont(Parameter::Spo2, f64::NAN, 78), None)
        .is_empty());
}

/// A spot-opened CONFIRMING alarm is confirmed by continuous data meeting persistence.
#[test]
fn continuous_data_confirms_a_confirming_alarm() {
    let mut e = only("spo2.critical");
    let ev = e.evaluate(&spot(Parameter::Spo2, 80.0, 0), None);
    assert!(matches!(&ev[..], [Event::Confirming(_)]));
    let ev = stream(&mut e, Parameter::Spo2, 80.0, 1, 16);
    assert_eq!(raised_at(&ev), Some(16), "{ev:?}");
}

/// Alarm state is per crew member (SR-006).
#[test]
fn state_is_per_crew_member() {
    let mut e = only("spo2.critical");
    let other = CrewMemberId::from_uuid(uuid::Uuid::from_u128(8));
    for t in 0..=15 {
        e.evaluate(&cont(Parameter::Spo2, 80.0, t), None);
        let mut i = cont(Parameter::Spo2, 97.0, t);
        i.crew = other;
        e.evaluate(&i, None);
    }
    let crews: Vec<_> = e.open_alarms().map(|a| a.crew).collect();
    assert_eq!(crews, [crew()]);
}

proptest! {
    /// SR-003: for any value stream, a continuous rule never fires before its condition has held
    /// for the persistence window without a longer gap, and the engine is deterministic.
    #[test]
    fn never_fires_before_persistence_and_is_deterministic(
        values in proptest::collection::vec((60.0f64..100.0, 1i64..40), 1..120)
    ) {
        let run = || {
            let mut e = only("spo2.critical");
            let mut t = 0;
            let mut breach_start: Option<i64> = None;
            let mut last = 0;
            let mut out = Vec::new();
            for (v, dt) in &values {
                t += dt;
                let breach = *v < 85.0;
                if breach {
                    if breach_start.is_none() || t - last > 30 {
                        breach_start = Some(t);
                    }
                } else {
                    breach_start = None;
                }
                last = t;
                for ev in e.evaluate(&cont(Parameter::Spo2, *v, t), None) {
                    if let Event::Raised(_) = ev {
                        let start = breach_start.expect("raised while not breaching");
                        assert!(t - start >= 15, "fired after {} s", t - start);
                    }
                    out.push(format!("{t}:{ev:?}"));
                }
            }
            out
        };
        prop_assert_eq!(run(), run());
    }
}

/// SR-019: a cleared, latched spot alarm breaching again before acknowledgement is reactivated;
/// readings between the thresholds change nothing.
#[test]
fn spot_alarm_reactivates_and_ignores_readings_between_thresholds() {
    let mut e = only("spo2.warning");
    e.evaluate(&spot(Parameter::Spo2, 88.0, 0), None);
    e.evaluate(&spot(Parameter::Spo2, 88.0, 10), None);
    assert!(
        e.evaluate(&spot(Parameter::Spo2, 91.0, 20), None)
            .is_empty(),
        "between 90 and 92"
    );
    let ev = e.evaluate(&spot(Parameter::Spo2, 93.0, 30), None);
    assert!(matches!(&ev[..], [Event::ClearedAwaitingAck(_)]));
    assert!(e
        .evaluate(&spot(Parameter::Spo2, 93.0, 35), None)
        .is_empty());
    let ev = e.evaluate(&spot(Parameter::Spo2, 88.0, 40), None);
    assert!(
        matches!(&ev[..], [Event::Reactivated(a)] if a.why.readings.len() == 3),
        "{ev:?}"
    );
    assert!(
        e.evaluate(&spot(Parameter::Spo2, 88.0, 50), None)
            .is_empty(),
        "already active"
    );
}

/// SR-006: a lower spot alarm is not opened while a higher one in its group is open; a repeat
/// breach after the higher one ends raises it at once.
#[test]
fn a_blocked_spot_breach_is_remembered_for_confirmation() {
    let mut e = engine_with(|r| r.id.starts_with("spo2."));
    stream(&mut e, Parameter::Spo2, 80.0, 0, 15);
    assert!(
        e.evaluate(&spot(Parameter::Spo2, 88.0, 20), None)
            .is_empty(),
        "blocked by critical"
    );
    let crit = e.open_alarms().next().unwrap().id;
    stream(&mut e, Parameter::Spo2, 95.0, 21, 81);
    e.acknowledge(crit);
    let ev = e.evaluate(&spot(Parameter::Spo2, 88.0, 100), None);
    assert!(
        matches!(&ev[..], [Event::Raised(a)] if a.rule_id == "spo2.warning"),
        "{ev:?}"
    );
}

/// SR-018: an at-rest baseline with unknown activity is evaluated, and the why says so.
#[test]
fn at_rest_baseline_with_unknown_activity_is_evaluated() {
    let mut e = engine_with(|_| false);
    let b = BaselineStats {
        median: 60.0,
        mad: 2.0,
        n: 500,
    };
    let ev = stream_with_baseline(&mut e, Parameter::HeartRate, 75.0, 0, 600, b);
    match ev.last() {
        Some(Event::Raised(a)) => {
            assert!(a.why.notes.iter().any(|n| n.contains("activity unknown")))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn engines_use_random_ids_and_expose_their_ruleset() {
    let mut a = Engine::new(bundled());
    assert_eq!(a.ruleset().version, "1.0.0-draft.3");
    stream(&mut a, Parameter::Spo2, 80.0, 0, 15);
    let mut b = Engine::new(bundled());
    stream(&mut b, Parameter::Spo2, 80.0, 0, 15);
    let ida = a.open_alarms().next().unwrap().id;
    let idb = b.open_alarms().next().unwrap().id;
    assert_ne!(ida, idb);
}
