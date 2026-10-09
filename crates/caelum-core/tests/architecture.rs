//! Architecture rules from docs/03_ARCHITECTURE.md §2, checked against `cargo metadata`.
//!
//! Covers the crate boundaries behind CLAUDE.md rules 3 and 5 (R-SEC-04, R-CLN-02, SR-011):
//! the workspace has exactly the documented crates, nothing depends on the app crate, the domain
//! crate does no I/O, and test equipment is reachable only as a dev-dependency or behind a
//! `test-equipment` feature that no default feature enables.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use std::collections::BTreeSet;

use cargo_metadata::{DependencyKind, Metadata, MetadataCommand, Package};

/// Crates listed in docs/03_ARCHITECTURE.md §2.
const DOCUMENTED_CRATES: [&str; 14] = [
    "caelum-domain",
    "caelum-store",
    "caelum-ingest",
    "caelum-signal",
    "caelum-alarms",
    "caelum-ml",
    "caelum-devices",
    "caelum-vehicle",
    "caelum-ground",
    "caelum-mds",
    "caelum-security",
    "caelum-cluster",
    "caelum-core",
    "caelum-test-equipment",
];

/// The Tauri app crate in `src-tauri/`; no other crate may depend on it.
const APP_CRATE: &str = "caelum-desktop";

const TEST_EQUIPMENT: &str = "caelum-test-equipment";
const TEST_EQUIPMENT_FEATURE: &str = "test-equipment";

/// Crates that perform I/O; `caelum-domain` must not depend on any of them.
const IO_CRATES: [&str; 17] = [
    "tokio",
    "rusqlite",
    "refinery",
    "btleplug",
    "bluer",
    "windows",
    "serialport",
    "hidapi",
    "dicom",
    "axum",
    "tokio-tungstenite",
    "rustls",
    "hyper",
    "reqwest",
    "ort",
    "cfdp-rs",
    "tauri",
];

fn metadata() -> Metadata {
    MetadataCommand::new()
        .no_deps()
        .exec()
        .expect("cargo metadata must run in the workspace")
}

fn workspace_packages(meta: &Metadata) -> Vec<&Package> {
    meta.workspace_packages()
}

fn package<'a>(meta: &'a Metadata, name: &str) -> &'a Package {
    workspace_packages(meta)
        .into_iter()
        .find(|p| p.name.as_str() == name)
        .unwrap_or_else(|| panic!("workspace package {name} not found"))
}

/// 03 §2: the workspace contains exactly the documented crates (plus the app crate).
#[test]
fn workspace_has_exactly_the_documented_crates() {
    let meta = metadata();
    let actual: BTreeSet<&str> = workspace_packages(&meta)
        .iter()
        .map(|p| p.name.as_str())
        .filter(|name| *name != APP_CRATE)
        .collect();
    let expected: BTreeSet<&str> = DOCUMENTED_CRATES.into_iter().collect();
    assert_eq!(actual, expected);
}

/// 03 §2: no crate depends on `src-tauri`.
#[test]
fn no_crate_depends_on_the_app_crate() {
    let meta = metadata();
    for p in workspace_packages(&meta) {
        assert!(
            !p.dependencies.iter().any(|d| d.name == APP_CRATE),
            "{} depends on {APP_CRATE}",
            p.name
        );
    }
}

/// 03 §2: `caelum-domain` performs no I/O and depends on no other workspace crate.
#[test]
fn domain_has_no_io_and_no_workspace_dependencies() {
    let meta = metadata();
    let domain = package(&meta, "caelum-domain");
    for d in domain
        .dependencies
        .iter()
        .filter(|d| d.kind == DependencyKind::Normal)
    {
        assert!(
            !DOCUMENTED_CRATES.contains(&d.name.as_str()),
            "caelum-domain depends on workspace crate {}",
            d.name
        );
        assert!(
            !IO_CRATES.contains(&d.name.as_str()),
            "caelum-domain depends on I/O crate {}",
            d.name
        );
    }
}

/// CLAUDE.md rule 3, R-CLN-02: test equipment is only a dev-dependency, or an optional
/// dependency enabled solely through a `test-equipment` feature.
#[test]
fn test_equipment_only_as_dev_dependency_or_behind_its_feature() {
    let meta = metadata();
    for p in workspace_packages(&meta) {
        for d in p.dependencies.iter().filter(|d| d.name == TEST_EQUIPMENT) {
            if d.kind == DependencyKind::Development {
                continue;
            }
            assert!(
                d.optional,
                "{} has a non-optional dependency on {TEST_EQUIPMENT}",
                p.name
            );
            let enabled_by: Vec<&String> = p
                .features
                .iter()
                .filter(|(_, items)| {
                    items
                        .iter()
                        .any(|i| i == &format!("dep:{TEST_EQUIPMENT}") || i == TEST_EQUIPMENT)
                })
                .map(|(feature, _)| feature)
                .collect();
            assert_eq!(
                enabled_by,
                vec![TEST_EQUIPMENT_FEATURE],
                "{}: {TEST_EQUIPMENT} must be enabled only by the `{TEST_EQUIPMENT_FEATURE}` feature",
                p.name
            );
        }
    }
}

/// CLAUDE.md rule 3: no default feature anywhere turns on test equipment.
#[test]
fn no_default_feature_enables_test_equipment() {
    let meta = metadata();
    for p in workspace_packages(&meta) {
        if let Some(defaults) = p.features.get("default") {
            assert!(
                !defaults.iter().any(|f| f.contains(TEST_EQUIPMENT_FEATURE)),
                "{}: default features enable test equipment",
                p.name
            );
        }
    }
}

/// CR-003 §4 and CLAUDE.md: every crate is Apache-2.0 and uses the workspace lints.
#[test]
fn every_crate_is_apache_2_and_inherits_workspace_lints() {
    let meta = metadata();
    for p in workspace_packages(&meta) {
        assert_eq!(
            p.license.as_deref(),
            Some("Apache-2.0"),
            "{} licence",
            p.name
        );
        let manifest = std::fs::read_to_string(&p.manifest_path)
            .expect("manifest must be readable")
            .replace("\r\n", "\n");
        assert!(
            manifest.contains("[lints]\nworkspace = true"),
            "{} does not inherit the workspace lints",
            p.name
        );
    }
}
