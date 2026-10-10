//! Ruleset loading (SR-009): the bundled ruleset matches docs/06 §3, verifies under development
//! keys only, and any tampering or invalid content rejects it (M3 gate: tampered ruleset rejected;
//! release build rejects a development-signed ruleset).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use caelum_alarms::ruleset::{Condition, Op, Priority, Ruleset, RulesetError, ThresholdLimits};
use caelum_alarms::signing::{
    built_in_keys, digest, key_id, sign, verify, Envelope, KeyClass, SignerRole, TrustPolicy,
    TrustedKey, VerifyError,
};
use caelum_alarms::{BUNDLED_RULESET, BUNDLED_SIGNATURES};
use ed25519_dalek::SigningKey;

const SAFETY_DOC: &str = include_str!("../../../docs/06_SAFETY_AND_ALARMS.md");

fn bundled() -> Ruleset {
    verify(
        BUNDLED_RULESET.as_bytes(),
        BUNDLED_SIGNATURES,
        &built_in_keys(),
        TrustPolicy::Development,
    )
    .expect("bundled ruleset verifies in development builds")
    .ruleset
}

/// Rows of the docs/06 §3 rule table: (rule, fires, clears).
fn doc_rows() -> Vec<(String, String, String)> {
    let start = SAFETY_DOC
        .find("| Rule | Category | Fires | Clears |")
        .unwrap();
    SAFETY_DOC[start..]
        .lines()
        .skip(2)
        .take_while(|l| l.starts_with('|'))
        .map(|l| {
            let c: Vec<String> = l
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_owned())
                .collect();
            (c[0].clone(), c[2].clone(), c[3].clone())
        })
        .filter(|(id, _, _)| !id.starts_with("baseline"))
        .collect()
}

fn first_number_after(text: &str, marks: &[char]) -> Option<f64> {
    let i = text.find(marks)?;
    let rest = text[i..].trim_start_matches(marks).trim_start();
    let num: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse().ok()
}

fn seconds(text: &str) -> Option<u32> {
    let i = text.find("for ")?;
    let num: String = text[i + 4..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    num.parse().ok()
}

/// docs/06 §3 is the source the ruleset was written from: every documented rule exists with the
/// same direction, thresholds and windows, and the ruleset has no undocumented rule.
#[test]
fn bundled_ruleset_matches_docs_06_section_3() {
    let rs = bundled();
    let rows = doc_rows();
    let mut doc_ids: Vec<&str> = rows.iter().map(|(id, _, _)| id.as_str()).collect();
    let mut rule_ids: Vec<&str> = rs.rules.iter().map(|r| r.id.as_str()).collect();
    doc_ids.sort_unstable();
    rule_ids.sort_unstable();
    assert_eq!(rule_ids, doc_ids);
    for (id, fires, clears) in &rows {
        let rule = rs.rules.iter().find(|r| &r.id == id).unwrap();
        match &rule.condition {
            Condition::Threshold(t) => match t.limits {
                ThresholdLimits::Fixed(l) => {
                    let op = if fires.contains(" < ") {
                        Op::Below
                    } else {
                        Op::Above
                    };
                    assert_eq!(t.op, op, "{id}");
                    assert_eq!(
                        first_number_after(fires, &['<', '>']),
                        Some(l.fire),
                        "{id} fire"
                    );
                    assert_eq!(
                        first_number_after(clears, &['≥', '≤']),
                        Some(l.clear),
                        "{id} clear"
                    );
                    assert_eq!(seconds(fires), Some(rule.fire_for_s), "{id} fire window");
                    assert_eq!(seconds(clears), Some(rule.clear_for_s), "{id} clear window");
                }
                ThresholdLimits::BySite(s) => {
                    assert!(
                        fires.contains(&format!("core-equivalent {:.1}", s.core_equivalent.fire)),
                        "{id}"
                    );
                    assert!(
                        fires.contains(&format!("peripheral and unknown {:.1}", s.peripheral.fire)),
                        "{id}"
                    );
                    assert_eq!(s.peripheral, s.unknown, "{id}");
                    assert!(
                        clears.contains(&format!(
                            "≤ {:.1} / ≤ {:.1}",
                            s.core_equivalent.clear, s.peripheral.clear
                        )),
                        "{id}"
                    );
                }
            },
            Condition::Absence(a) => {
                assert!(
                    fires.contains(&format!("no QRS for {} s", rule.fire_for_s)),
                    "{id}"
                );
                assert!(
                    clears.contains(&format!("for {} s", rule.clear_for_s)),
                    "{id}"
                );
                assert_eq!(a.event, "qrs");
            }
        }
    }
}

#[test]
fn bundled_ruleset_is_draft_and_development_signed() {
    let v = verify(
        BUNDLED_RULESET.as_bytes(),
        BUNDLED_SIGNATURES,
        &built_in_keys(),
        TrustPolicy::Development,
    )
    .unwrap();
    assert!(v.development, "DEVELOPMENT RULESET banner");
    assert_eq!(v.ruleset.status, "DRAFT");
    assert!(
        v.ruleset.rules.iter().all(|r| r.max_gap_s == 30),
        "docs/06 §3: 30 s until review"
    );
    let af = v
        .ruleset
        .rules
        .iter()
        .find(|r| r.id == "af.observe")
        .unwrap();
    assert_eq!(af.priority, Priority::Observe);
}

/// M3 gate, SR-009: a release build rejects a development-signed ruleset.
#[test]
fn sr_009_release_builds_reject_development_signed_rulesets() {
    let r = verify(
        BUNDLED_RULESET.as_bytes(),
        BUNDLED_SIGNATURES,
        &built_in_keys(),
        TrustPolicy::Flight,
    );
    assert!(matches!(r, Err(VerifyError::UntrustedKey(_))), "{r:?}");
    // No flight key is compiled in until a licensed clinician advisor countersigns (CR-003).
    assert!(built_in_keys()
        .iter()
        .all(|k| k.class == KeyClass::Development));
}

fn test_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn envelope(bytes: &[u8], version: &str, signers: &[(SignerRole, &SigningKey)]) -> String {
    toml::to_string(&Envelope {
        ruleset_version: version.to_owned(),
        sha256: digest(bytes),
        signatures: signers
            .iter()
            .map(|(role, k)| sign(bytes, version, *role, k))
            .collect(),
    })
    .unwrap()
}

/// SR-009: flight keys need a medical approver signature; with it, no banner.
#[test]
fn sr_009_flight_policy_needs_a_medical_approver_flight_signature() {
    let author = test_key(1);
    let medic = test_key(2);
    let keys = [
        TrustedKey {
            class: KeyClass::Flight,
            key: author.verifying_key(),
        },
        TrustedKey {
            class: KeyClass::Flight,
            key: medic.verifying_key(),
        },
    ];
    let bytes = BUNDLED_RULESET.as_bytes();
    let version = "1.0.0-draft.1";
    let author_only = envelope(bytes, version, &[(SignerRole::Author, &author)]);
    assert_eq!(
        verify(bytes, &author_only, &keys, TrustPolicy::Flight),
        Err(VerifyError::NoMedicalApproval)
    );
    let both = envelope(
        bytes,
        version,
        &[
            (SignerRole::Author, &author),
            (SignerRole::MedicalApprover, &medic),
        ],
    );
    let v = verify(bytes, &both, &keys, TrustPolicy::Flight).unwrap();
    assert!(!v.development);
}

/// M3 gate, SR-009: any tampering rejects the ruleset.
#[test]
fn sr_009_tampered_rulesets_are_rejected() {
    let keys = built_in_keys();
    let dev = TrustPolicy::Development;
    // One changed threshold, signatures untouched.
    let tampered = BUNDLED_RULESET.replacen("fire = 85.0", "fire = 75.0", 1);
    assert_eq!(
        verify(tampered.as_bytes(), BUNDLED_SIGNATURES, &keys, dev),
        Err(VerifyError::DigestMismatch)
    );
    // The digest updated to match, but the signature cannot be.
    let mut env: Envelope = toml::from_str(BUNDLED_SIGNATURES).unwrap();
    env.sha256 = digest(tampered.as_bytes());
    let forged = toml::to_string(&env).unwrap();
    assert!(matches!(
        verify(tampered.as_bytes(), &forged, &keys, dev),
        Err(VerifyError::BadSignature(_))
    ));
    // Signed by a key the build does not know.
    let stranger = test_key(9);
    let foreign = envelope(
        BUNDLED_RULESET.as_bytes(),
        "1.0.0-draft.1",
        &[(SignerRole::Author, &stranger)],
    );
    assert_eq!(
        verify(BUNDLED_RULESET.as_bytes(), &foreign, &keys, dev),
        Err(VerifyError::UntrustedKey(key_id(&stranger.verifying_key())))
    );
    // Unsigned, unparsable, wrong version.
    let unsigned = envelope(BUNDLED_RULESET.as_bytes(), "1.0.0-draft.1", &[]);
    assert_eq!(
        verify(BUNDLED_RULESET.as_bytes(), &unsigned, &keys, dev),
        Err(VerifyError::Unsigned)
    );
    assert!(matches!(
        verify(BUNDLED_RULESET.as_bytes(), "not = [toml", &keys, dev),
        Err(VerifyError::Envelope(_))
    ));
    let trusted = TrustedKey {
        class: KeyClass::Development,
        key: stranger.verifying_key(),
    };
    let wrong_version = envelope(
        BUNDLED_RULESET.as_bytes(),
        "9.9.9",
        &[(SignerRole::Author, &stranger)],
    );
    assert!(matches!(
        verify(BUNDLED_RULESET.as_bytes(), &wrong_version, &[trusted], dev),
        Err(VerifyError::VersionMismatch { .. })
    ));
}

fn invalid(edit: impl Fn(&str) -> String) -> Vec<String> {
    match Ruleset::parse(&edit(BUNDLED_RULESET)) {
        Err(RulesetError::Invalid(errors)) => errors,
        other => panic!("expected invalid, got {other:?}"),
    }
}

/// SR-009: invalid content rejects the whole ruleset, with every reason.
#[test]
fn invalid_rules_reject_the_whole_ruleset() {
    let wrong_side = invalid(|t| t.replacen("clear = 90.0", "clear = 80.0", 1));
    assert!(
        wrong_side
            .iter()
            .any(|e| e.contains("spo2.critical") && e.contains("wrong side")),
        "{wrong_side:?}"
    );
    let dup = invalid(|t| t.replacen("id = \"spo2.warning\"", "id = \"spo2.critical\"", 1));
    assert!(dup.iter().any(|e| e.contains("duplicate")));
    let unknown = invalid(|t| t.replacen("parameter = \"spo2\"", "parameter = \"spo3\"", 1));
    assert!(unknown.iter().any(|e| e.contains("unknown parameter")));
    let af = invalid(|t| t.replacen("priority = \"observe\"", "priority = \"warning\"", 1));
    assert!(af.iter().any(|e| e.contains("af_probability")));
    let no_spot = invalid(|t| {
        t.replacen(
            "spot = { confirm_window_s = 300, recheck_timeout_s = 120 }\n",
            "",
            1,
        )
    });
    assert!(no_spot.iter().any(|e| e.contains("SR-019")), "{no_spot:?}");
    let site = invalid(|t| {
        t.replacen(
            "unknown = { fire = 37.5, clear = 37.2 }",
            "unknown = { fire = 38.5, clear = 37.2 }",
            1,
        )
    });
    assert!(
        site.iter().any(|e| e.contains("most conservative")),
        "{site:?}"
    );
    let gap = invalid(|t| t.replacen("max_gap_s = 30", "max_gap_s = 0", 1));
    assert!(gap.iter().any(|e| e.contains("max_gap_s")));
    let implausible = invalid(|t| t.replacen("fire = 180.0", "fire = 400.0", 1));
    assert!(
        implausible.iter().any(|e| e.contains("plausible range")),
        "{implausible:?}"
    );
}

#[test]
fn unknown_fields_and_bad_toml_are_refused() {
    let extra = BUNDLED_RULESET.replacen(
        "fire_for_s = 15",
        "fire_for_s = 15\nsilence_forever = true",
        1,
    );
    assert!(matches!(
        Ruleset::parse(&extra),
        Err(RulesetError::Parse(_))
    ));
    assert!(matches!(
        Ruleset::parse("[[rule"),
        Err(RulesetError::Parse(_))
    ));
}
