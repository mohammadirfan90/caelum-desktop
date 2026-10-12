//! Verification of the trained NASA / clinical models against M12 requirements.
//!
//! Confirms that all generated model packages in `models/` (M1, M2, M3, M4, M5, M6, M9)
//! verify with built-in development keys, pin their SHA-256 hashes, validate their manifests,
//! define occlusion explanation configurations (F-ML-04), and conform to SR-002.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;

use caelum_alarms::signing::{built_in_keys, TrustPolicy};
use caelum_ml::manifest::ValidationStatus;
use caelum_ml::package;

const DEV: TrustPolicy = TrustPolicy::Development;

fn repo_models_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap().parent().unwrap().join("models")
}

#[test]
fn test_all_trained_model_packages_verify_with_built_in_keys() {
    let models_dir = repo_models_dir();
    let keys = built_in_keys();
    let model_ids = ["M1", "M2", "M3", "M4", "M5", "M6", "M9"];

    for id in model_ids {
        let pkg_dir = models_dir.join(id);
        assert!(
            pkg_dir.exists(),
            "Model directory for {id} must exist at {:?}",
            pkg_dir
        );

        let pkg = package::load_dir(&pkg_dir, &keys, DEV)
            .unwrap_or_else(|e| panic!("Model package {id} failed to load and verify: {e}"));

        assert_eq!(pkg.manifest().model.id, id, "Manifest model id must match");
        assert!(
            !pkg.model_bytes().is_empty(),
            "Model binary bytes must not be empty"
        );
        assert!(
            pkg.development(),
            "Package must be accepted under development policy"
        );
        assert_eq!(
            pkg.manifest().validation.status,
            ValidationStatus::NotValidated,
            "Package {id} must have NOT_VALIDATED status"
        );
        assert!(
            pkg.manifest().explanation.top_k >= 2,
            "Package {id} must configure top_k >= 2 explanations (F-ML-04)"
        );
        assert!(
            !pkg.manifest().features.is_empty(),
            "Package {id} must specify input features"
        );
        assert!(
            !pkg.manifest().outputs.is_empty(),
            "Package {id} must specify output slots"
        );
    }
}

#[test]
fn test_m2_conforms_to_observe_only_parameter_gate() {
    let models_dir = repo_models_dir();
    let keys = built_in_keys();
    let pkg = package::load_dir(&models_dir.join("M2"), &keys, DEV).expect("M2 loads");

    let out = &pkg.manifest().outputs[0];
    assert_eq!(
        out.parameter.as_deref(),
        Some("af_probability"),
        "M2 must feed parameter af_probability into observe rule (docs/08 §3)"
    );
}
