//! F-ML-01: package verification (hash pinning, signatures, manifest validation).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // test code

mod common;

use caelum_alarms::signing::{ConfigVerifyError, KeyClass, SignerRole, TrustPolicy};
use caelum_ml::manifest::{Manifest, ManifestError, ValidationStatus};
use caelum_ml::package::{self, PackageError};
use common::*;

const DEV: TrustPolicy = TrustPolicy::Development;
const FLIGHT: TrustPolicy = TrustPolicy::Flight;

#[test]
fn f_ml_01_valid_package_verifies_and_pins_the_model() {
    let b = build_package(&[0.01, 0.02], -1.0, "1.0.0", "NOT_VALIDATED");
    let p = package::verify(
        &b.manifest,
        &b.envelope,
        &b.model,
        &keys(KeyClass::Development),
        DEV,
    )
    .expect("verifies");
    assert_eq!(p.manifest().model.id, "T2");
    assert_eq!(p.model_bytes(), b.model.as_slice());
    assert!(p.development());
    assert_eq!(
        p.manifest().validation.status,
        ValidationStatus::NotValidated
    );
    assert_eq!(p.manifest().input_len(), 2);
}

#[test]
fn f_ml_01_tampered_model_is_rejected() {
    let b = build_package(&[0.01, 0.02], -1.0, "1.0.0", "NOT_VALIDATED");
    let mut model = b.model.clone();
    model[0] ^= 0x01;
    let err = package::verify(
        &b.manifest,
        &b.envelope,
        &model,
        &keys(KeyClass::Development),
        DEV,
    )
    .unwrap_err();
    assert!(
        matches!(err, PackageError::ModelDigestMismatch { .. }),
        "{err}"
    );
}

#[test]
fn f_ml_01_tampered_manifest_is_rejected() {
    let b = build_package(&[0.01, 0.02], -1.0, "1.0.0", "NOT_VALIDATED");
    let text = String::from_utf8(b.manifest.clone())
        .unwrap()
        .replace("time_limit_ms = 200", "time_limit_ms = 900");
    let err = package::verify(
        text.as_bytes(),
        &b.envelope,
        &b.model,
        &keys(KeyClass::Development),
        DEV,
    )
    .unwrap_err();
    assert_eq!(
        err,
        PackageError::Signature(ConfigVerifyError::DigestMismatch)
    );
}

#[test]
fn f_ml_01_bad_signature_is_rejected() {
    let b = build_package(&[0.01, 0.02], -1.0, "1.0.0", "NOT_VALIDATED");
    // A valid signature by the trusted key over different bytes, presented for these bytes.
    let other = envelope(
        "model",
        b"other bytes",
        "1.0.0",
        &author_key(),
        SignerRole::Author,
    );
    let sig_line = other
        .lines()
        .find(|l| l.starts_with("signature ="))
        .unwrap()
        .to_owned();
    let forged: String = b
        .envelope
        .lines()
        .map(|l| {
            if l.starts_with("signature =") {
                sig_line.clone()
            } else {
                l.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let err = package::verify(
        &b.manifest,
        &forged,
        &b.model,
        &keys(KeyClass::Development),
        DEV,
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            PackageError::Signature(ConfigVerifyError::BadSignature(_))
        ),
        "{err}"
    );
}

#[test]
fn f_ml_01_untrusted_key_is_rejected() {
    let b = build_package(&[0.01, 0.02], -1.0, "1.0.0", "NOT_VALIDATED");
    let env = envelope(
        "model",
        &b.manifest,
        "1.0.0",
        &other_key(),
        SignerRole::Author,
    );
    let err = package::verify(
        &b.manifest,
        &env,
        &b.model,
        &keys(KeyClass::Development),
        DEV,
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            PackageError::Signature(ConfigVerifyError::UntrustedKey(_))
        ),
        "{err}"
    );
}

#[test]
fn f_ml_01_wrong_signature_kind_is_rejected() {
    let b = build_package(&[0.01, 0.02], -1.0, "1.0.0", "NOT_VALIDATED");
    // A crew-roster signature over the very same bytes never passes as a model signature.
    let env = envelope(
        "crew",
        &b.manifest,
        "1.0.0",
        &author_key(),
        SignerRole::Author,
    );
    let err = package::verify(
        &b.manifest,
        &env,
        &b.model,
        &keys(KeyClass::Development),
        DEV,
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            PackageError::Signature(ConfigVerifyError::WrongKind { .. })
        ),
        "{err}"
    );
}

#[test]
fn f_ml_01_signed_version_must_match_manifest() {
    let b = build_package(&[0.01, 0.02], -1.0, "1.0.0", "NOT_VALIDATED");
    let env = envelope(
        "model",
        &b.manifest,
        "2.0.0",
        &author_key(),
        SignerRole::Author,
    );
    let err = package::verify(
        &b.manifest,
        &env,
        &b.model,
        &keys(KeyClass::Development),
        DEV,
    )
    .unwrap_err();
    assert!(matches!(err, PackageError::VersionMismatch { .. }), "{err}");
}

#[test]
fn f_ml_01_release_builds_need_flight_keys() {
    let b = build_package(&[0.01, 0.02], -1.0, "1.0.0", "NOT_VALIDATED");
    let err = package::verify(
        &b.manifest,
        &b.envelope,
        &b.model,
        &keys(KeyClass::Development),
        FLIGHT,
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            PackageError::Signature(ConfigVerifyError::UntrustedKey(_))
        ),
        "{err}"
    );
    let ok = package::verify(
        &b.manifest,
        &b.envelope,
        &b.model,
        &keys(KeyClass::Flight),
        FLIGHT,
    )
    .expect("flight-signed NOT_VALIDATED package loads");
    assert!(!ok.development());
}

#[test]
fn f_ml_01_release_builds_refuse_research_packages() {
    let b = build_package(&[0.01, 0.02], -1.0, "1.0.0", "RESEARCH");
    let err = package::verify(
        &b.manifest,
        &b.envelope,
        &b.model,
        &keys(KeyClass::Flight),
        FLIGHT,
    )
    .unwrap_err();
    assert_eq!(err, PackageError::ResearchInRelease);
    package::verify(
        &b.manifest,
        &b.envelope,
        &b.model,
        &keys(KeyClass::Development),
        DEV,
    )
    .expect("development builds may load research packages");
}

#[test]
fn f_ml_01_validated_in_release_needs_medical_approver() {
    let b = build_package(&[0.01, 0.02], -1.0, "1.0.0", "VALIDATED");
    let err = package::verify(
        &b.manifest,
        &b.envelope,
        &b.model,
        &keys(KeyClass::Flight),
        FLIGHT,
    )
    .unwrap_err();
    assert_eq!(err, PackageError::NoMedicalApproval);
    // Author plus medical approver (same test key in both roles is enough for the format test).
    let author = envelope(
        "model",
        &b.manifest,
        "1.0.0",
        &author_key(),
        SignerRole::Author,
    );
    let approver = envelope(
        "model",
        &b.manifest,
        "1.0.0",
        &author_key(),
        SignerRole::MedicalApprover,
    );
    let approver_sig: String = approver
        .lines()
        .skip_while(|l| !l.starts_with("[[signature]]"))
        .collect::<Vec<_>>()
        .join("\n");
    let both = format!("{author}\n{approver_sig}\n");
    package::verify(
        &b.manifest,
        &both,
        &b.model,
        &keys(KeyClass::Flight),
        FLIGHT,
    )
    .expect("validated package with medical approval loads");
}

fn manifest_with(edit: impl Fn(String) -> String) -> Result<Manifest, ManifestError> {
    let text = manifest_text(&"a".repeat(64), "1.0.0", "NOT_VALIDATED");
    Manifest::parse(&edit(text))
}

#[test]
fn f_ml_01_missing_or_unknown_fields_are_rejected() {
    for field in [
        "sha256 = ",
        "gate = ",
        "intended_use = ",
        "training_data = ",
        "evaluation = ",
        "limitations = ",
        "approver = ",
        "time_limit_ms = ",
        "input_name = ",
        "top_k = ",
        "reference = ",
    ] {
        let r = manifest_with(|t| {
            t.lines()
                .filter(|l| !l.starts_with(field))
                .collect::<Vec<_>>()
                .join("\n")
        });
        assert!(matches!(r, Err(ManifestError::Parse(_))), "{field}: {r:?}");
    }
    let r = manifest_with(|t| t.replace("[runtime]", "[runtime]\nthreads = 4"));
    assert!(matches!(r, Err(ManifestError::Parse(_))), "{r:?}");
}

#[test]
fn f_ml_01_inconsistent_manifests_are_rejected() {
    type Edit = Box<dyn Fn(String) -> String>;
    let cases: Vec<(&str, Edit)> = vec![
        (
            "empty card",
            Box::new(|t| {
                t.replace(
                    "limitations = \"Not a clinical model\"",
                    "limitations = \" \"",
                )
            }),
        ),
        (
            "bad hash",
            Box::new(|t| t.replace(&"a".repeat(64), &"A".repeat(64))),
        ),
        (
            "path file",
            Box::new(|t| t.replace("file = \"model.onnx\"", "file = \"../model.onnx\"")),
        ),
        (
            "time limit",
            Box::new(|t| t.replace("time_limit_ms = 200", "time_limit_ms = 5000")),
        ),
        (
            "zero limit",
            Box::new(|t| t.replace("time_limit_ms = 200", "time_limit_ms = 0")),
        ),
        (
            "validated without evidence",
            Box::new(|t| t.replace("NOT_VALIDATED", "VALIDATED")),
        ),
        (
            "unknown parameter",
            Box::new(|t| t.replace("parameter = \"heart_rate\"", "parameter = \"pulse_speed\"")),
        ),
        (
            "non-numeric parameter",
            Box::new(|t| t.replace("parameter = \"heart_rate\"", "parameter = \"sleep_stage\"")),
        ),
        (
            "duplicate feature",
            Box::new(|t| t.replace("name = \"rr_rmssd\"", "name = \"hr_mean\"")),
        ),
        (
            "gap above window",
            Box::new(|t| {
                t.replace(
                    "max_gap_s = 10\nreference = 70.0",
                    "max_gap_s = 61\nreference = 70.0",
                )
            }),
        ),
        (
            "rmssd one sample",
            Box::new(|t| {
                t.replace(
                    "min_samples = 5\nmax_gap_s = 10\nreference = 30.0",
                    "min_samples = 1\nmax_gap_s = 10\nreference = 30.0",
                )
            }),
        ),
        (
            "series without length",
            Box::new(|t| t.replace("aggregate = \"mean\"", "aggregate = \"series\"")),
        ),
        (
            "length on scalar",
            Box::new(|t| t.replace("aggregate = \"mean\"", "aggregate = \"mean\"\nlength = 4")),
        ),
        (
            "output range",
            Box::new(|t| t.replace("min = 0.0\nmax = 1.0", "min = 1.0\nmax = 1.0")),
        ),
        (
            "output beyond parameter",
            Box::new(|t| t.replace("min = 0.0\nmax = 1.0", "min = 0.0\nmax = 2.0")),
        ),
        (
            "not a derived parameter",
            Box::new(|t| t.replace("parameter = \"af_probability\"", "parameter = \"spo2\"")),
        ),
        (
            "notice outside range",
            Box::new(|t| t.replace("threshold = 0.8", "threshold = 1.5")),
        ),
        (
            "top_k zero",
            Box::new(|t| t.replace("top_k = 2", "top_k = 0")),
        ),
    ];
    for (name, edit) in cases {
        let r = manifest_with(&edit);
        assert!(r.is_err(), "{name} must be rejected");
    }
    manifest_with(|t| t).expect("the unedited manifest is valid");
}

#[test]
fn f_ml_01_load_dir_reads_and_verifies_a_package() {
    let dir = std::env::temp_dir().join(format!("caelum-ml-pkg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let b = build_package(&[0.01, 0.02], -1.0, "1.0.0", "NOT_VALIDATED");
    std::fs::write(dir.join(package::MANIFEST_FILE), &b.manifest).unwrap();
    std::fs::write(dir.join(package::SIGNATURE_FILE), &b.envelope).unwrap();
    std::fs::write(dir.join("model.onnx"), &b.model).unwrap();
    let p = package::load_dir(&dir, &keys(KeyClass::Development), DEV).expect("loads");
    assert_eq!(p.manifest().model.version, "1.0.0");
    std::fs::write(dir.join("model.onnx"), b"replaced").unwrap();
    let err = package::load_dir(&dir, &keys(KeyClass::Development), DEV).unwrap_err();
    assert!(matches!(err, PackageError::ModelDigestMismatch { .. }));
    std::fs::remove_file(dir.join("model.onnx")).unwrap();
    let err = package::load_dir(&dir, &keys(KeyClass::Development), DEV).unwrap_err();
    assert!(matches!(err, PackageError::Io { .. }));
    let _ = std::fs::remove_dir_all(&dir);
}
