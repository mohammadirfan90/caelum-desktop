//! SR-009: every validation rule rejects the whole ruleset with its reason (table-driven).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code

use caelum_alarms::ruleset::{Ruleset, RulesetError};
use caelum_alarms::signing::TrustPolicy;
use caelum_alarms::BUNDLED_RULESET;

const HEADER: &str = r#"
[ruleset]
version = "t"
status = "DRAFT"
approved_by = "test"
"#;

const BASELINE: &str = r#"
[baseline]
z_fire = 4.0
z_clear = 2.5
for_s = 600
spot_k = 3
spot_n = 4
max_gap_s = 30
at_rest = { parameter = "activity_level", max = 0.3 }
parameters = [{ parameter = "heart_rate", category = "CARDIO", at_rest = true }]
"#;

fn rule(body: &str) -> String {
    format!(
        "{HEADER}\n[[rule]]\nid = \"x\"\ncategory = \"CARDIO\"\npriority = \"warning\"\ntitle = \"t\"\naction = \"a\"\nfire_for_s = 10\nclear_for_s = 10\nmax_gap_s = 30\n{body}\n{BASELINE}"
    )
}

fn errors(text: &str) -> Vec<String> {
    match Ruleset::parse(text) {
        Err(RulesetError::Invalid(e)) => e,
        other => panic!("expected invalid: {other:?}\n{text}"),
    }
}

const SPOT: &str = "spot = { confirm_window_s = 300, recheck_timeout_s = 600 }";

#[test]
fn every_rule_level_check_rejects_with_its_reason() {
    let cases: Vec<(String, &str)> = vec![
        (rule("type = \"threshold\"\nop = \">\"\nfire = 1.0\nclear = 0.5"), "need a parameter"),
        (rule(&format!("type = \"threshold\"\nparameter = \"heart_rate\"\nfire = 130.0\nclear = 120.0\n{SPOT}")), "need op"),
        (rule("type = \"threshold\"\nparameter = \"sleep_stage\"\nop = \">\"\nfire = 1.0\nclear = 0.5"), "not numeric"),
        (rule("type = \"threshold\"\nparameter = \"battery_level\"\nop = \"<\"\nfire = 10.0\nclear = 20.0"), "technical parameters"),
        (rule(&format!("type = \"threshold\"\nparameter = \"heart_rate\"\nop = \">\"\n{SPOT}")), "either fire and clear, or sites"),
        (
            rule(&format!("type = \"threshold\"\nparameter = \"heart_rate\"\nop = \">\"\n{SPOT}\nsites = {{ core_equivalent = {{ fire = 130.0, clear = 120.0 }}, peripheral = {{ fire = 130.0, clear = 120.0 }}, unknown = {{ fire = 130.0, clear = 120.0 }} }}")),
            "only for body_temp_measured",
        ),
        (rule(&format!("type = \"threshold\"\nparameter = \"body_temp_measured\"\nop = \">\"\nfire = 38.0\nclear = 37.7\n{SPOT}")), "need per-site limits"),
        (
            rule("type = \"threshold\"\nparameter = \"heart_rate\"\nop = \">\"\nfire = 130.0\nclear = 120.0\nspot = { confirm_window_s = 0, recheck_timeout_s = 600 }"),
            "spot windows must be positive",
        ),
        (
            rule(&format!("type = \"threshold\"\nparameter = \"heart_rate\"\nop = \">\"\nfire = 130.0\nclear = 120.0\n{SPOT}\nat_rest = {{ parameter = \"heart_rate\", max = 0.3 }}")),
            "at_rest uses activity_level",
        ),
        (
            rule(&format!("type = \"threshold\"\nparameter = \"heart_rate\"\nop = \">\"\nfire = 130.0\nclear = 120.0\n{SPOT}\nevent = \"qrs\"")),
            "only for absence rules",
        ),
        (rule(&format!("type = \"threshold\"\nparameter = \"heart_rate\"\nop = \">\"\nfire = nan\nclear = 120.0\n{SPOT}")), "finite"),
        (rule("type = \"absence\""), "need event and gate"),
        (rule("type = \"absence\"\nevent = \"qrs\"\ngate = { parameter = \"sqi_ecg\", min = 0.7 }\nop = \">\""), "threshold fields are not allowed"),
        (rule("type = \"absence\"\nevent = \"qrs\"\ngate = { parameter = \"nope\", min = 0.7 }"), "unknown parameter"),
        (rule("type = \"sometimes\""), "unknown rule type"),
        (
            rule("type = \"threshold\"\nparameter = \"heart_rate\"\nop = \">\"\nfire = 130.0\nclear = 120.0\ntitle2 = 1").replace("title2 = 1", ""),
            "spot confirmation is required",
        ),
    ];
    for (text, reason) in cases {
        let e = errors(&text);
        assert!(
            e.iter().any(|m| m.contains(reason)),
            "expected {reason:?}, got {e:?}"
        );
    }
}

#[test]
fn absence_rules_need_a_positive_window_and_rules_need_text() {
    let text =
        rule("type = \"absence\"\nevent = \"qrs\"\ngate = { parameter = \"sqi_ecg\", min = 0.7 }")
            .replacen("fire_for_s = 10", "fire_for_s = 0", 1);
    assert!(errors(&text).iter().any(|e| e.contains("absence window")));
    let text =
        rule("type = \"absence\"\nevent = \"qrs\"\ngate = { parameter = \"sqi_ecg\", min = 0.7 }")
            .replacen("title = \"t\"", "title = \" \"", 1);
    assert!(errors(&text).iter().any(|e| e.contains("title and action")));
}

/// Per-site limits on a falling threshold: the unknown site must be the most conservative too.
#[test]
fn below_thresholds_check_the_unknown_site_as_well() {
    let text = rule(&format!(
        "type = \"threshold\"\nparameter = \"body_temp_measured\"\nop = \"<\"\n{SPOT}\nsites = {{ core_equivalent = {{ fire = 35.0, clear = 35.5 }}, peripheral = {{ fire = 34.5, clear = 35.0 }}, unknown = {{ fire = 34.0, clear = 35.0 }} }}"
    ));
    assert!(errors(&text)
        .iter()
        .any(|e| e.contains("most conservative")));
}

#[test]
fn header_and_baseline_checks_reject_the_ruleset() {
    let ok_rule =
        "type = \"absence\"\nevent = \"qrs\"\ngate = { parameter = \"sqi_ecg\", min = 0.7 }";
    let no_version = rule(ok_rule).replacen("version = \"t\"", "version = \" \"", 1);
    assert!(errors(&no_version)
        .iter()
        .any(|e| e.contains("version is required")));
    let z = rule(ok_rule).replacen("z_clear = 2.5", "z_clear = 5.0", 1);
    assert!(errors(&z).iter().any(|e| e.contains("z_clear < z_fire")));
    let k = rule(ok_rule).replacen("spot_k = 3", "spot_k = 5", 1);
    assert!(errors(&k).iter().any(|e| e.contains("spot_k")));
    let rest = rule(ok_rule).replacen(
        "at_rest = { parameter = \"activity_level\", max = 0.3 }\nparameters",
        "at_rest = { parameter = \"heart_rate\", max = 0.3 }\nparameters",
        1,
    );
    assert!(
        errors(&rest)
            .iter()
            .any(|e| e.contains("baseline: at_rest")),
        "{:?}",
        errors(&rest)
    );
    let unknown = rule(ok_rule).replacen(
        "{ parameter = \"heart_rate\", category",
        "{ parameter = \"pulse\", category",
        1,
    );
    assert!(errors(&unknown)
        .iter()
        .any(|e| e.contains("unknown parameter")));
    assert!(Ruleset::parse(&rule(ok_rule)).is_ok());
}

#[test]
fn the_bundled_ruleset_parses_and_tests_run_with_the_development_policy() {
    assert!(Ruleset::parse(BUNDLED_RULESET).is_ok());
    assert_eq!(TrustPolicy::for_this_build(), TrustPolicy::Development);
}
