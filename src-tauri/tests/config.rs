//! Locks the security- and offline-relevant Tauri settings (docs/03_ARCHITECTURE.md §5 and §7,
//! R-OPS-03, F-SYS-06, CR-002 D4, CR-003 §4) so a later change cannot loosen them silently.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use std::path::{Path, PathBuf};

use serde_json::Value;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("invalid JSON in {}: {e}", path.display()))
}

fn conf() -> Value {
    read_json(&crate_dir().join("tauri.conf.json"))
}

fn csp_sources(csp: &str) -> Vec<&str> {
    csp.split(';')
        .flat_map(|directive| directive.split_whitespace().skip(1))
        .collect()
}

/// CR-003 §4: bundle identifier and product name are fixed.
#[test]
fn identity_matches_cr_003() {
    let conf = conf();
    assert_eq!(conf["identifier"], "org.chronostrider.caelum");
    assert_eq!(
        conf["productName"],
        caelum_desktop_lib::commands::PRODUCT_NAME
    );
}

/// One source of truth: the version comes from the Cargo workspace, never from tauri.conf.json.
#[test]
fn version_is_not_overridden_in_tauri_conf() {
    assert!(
        conf().get("version").is_none(),
        "tauri.conf.json must not set a version"
    );
}

/// 03 §5: CSP locked, no remote content, no eval, no inline script or style in release builds.
#[test]
fn release_csp_allows_no_remote_content_and_no_inline_or_eval() {
    let conf = conf();
    let csp = conf["app"]["security"]["csp"]
        .as_str()
        .expect("csp must be a string");
    assert!(csp.contains("default-src 'self'"), "{csp}");
    assert!(csp.contains("object-src 'none'"), "{csp}");
    for source in csp_sources(csp) {
        assert!(
            !source.contains("unsafe-eval") && !source.contains("unsafe-inline"),
            "release CSP must not contain {source}"
        );
        if source.starts_with("http") || source.starts_with("ws") {
            assert_eq!(
                source, "http://ipc.localhost",
                "remote source {source} in release CSP"
            );
        }
    }
}

/// 03 §5: no global Tauri object and no Tauri plugins (no shell, file system, HTTP or updater).
#[test]
fn no_global_api_and_no_plugins() {
    let conf = conf();
    assert_eq!(conf["app"]["withGlobalTauri"], false);
    let plugins = conf.get("plugins").and_then(Value::as_object);
    assert!(
        plugins.is_none_or(|p| p.is_empty()),
        "plugins configured: {plugins:?}"
    );
    let manifest = std::fs::read_to_string(crate_dir().join("Cargo.toml")).expect("Cargo.toml");
    assert!(
        !manifest.contains("tauri-plugin-"),
        "src-tauri depends on a Tauri plugin"
    );
}

/// The IPC command names of `docs/ROLE_MATRIX.md` §3, as the security core defines them.
fn matrix_commands() -> Vec<&'static str> {
    caelum_core::security::role::Command::ALL
        .iter()
        .map(|c| c.name())
        .collect()
}

/// 03 §5: every capability grants only CAELUM's own commands: exactly those of the role matrix.
#[test]
fn capabilities_grant_only_app_commands() {
    let expected: Vec<String> = matrix_commands()
        .iter()
        .map(|n| format!("allow-{}", n.replace('_', "-")))
        .collect();
    let dir = crate_dir().join("capabilities");
    let mut files = 0;
    for entry in std::fs::read_dir(&dir).expect("capabilities directory") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        files += 1;
        let capability = read_json(&path);
        let permissions = capability["permissions"]
            .as_array()
            .expect("permissions array");
        let names: Vec<&str> = permissions.iter().filter_map(Value::as_str).collect();
        assert_eq!(names, expected, "{}", path.display());
    }
    assert_eq!(files, 1, "expected exactly one capability file");
}

/// Every command in the role matrix is in the build manifest (so it needs a permission) and in the
/// generated bindings, and there are no others (R-SEC-02, CLAUDE.md rule 6).
#[test]
fn registered_commands_are_exactly_the_role_matrix_commands() {
    let mut expected = matrix_commands();
    expected.sort_unstable();

    let build = std::fs::read_to_string(crate_dir().join("build.rs")).expect("build.rs");
    let start = build.find(".commands(&[").expect("manifest command list");
    let end = build[start..].find("])").expect("end of list") + start;
    let mut manifest: Vec<&str> = build[start..end].split('"').skip(1).step_by(2).collect();
    manifest.sort_unstable();
    assert_eq!(manifest, expected, "build.rs app manifest");

    let bindings =
        std::fs::read_to_string(crate_dir().join("../ui/src/bindings.ts")).expect("bindings");
    let mut invoked: Vec<&str> = bindings
        .split("__TAURI_INVOKE")
        .skip(2) // the import line, then one split per command
        .filter_map(|part| part.split('"').nth(1))
        .collect();
    invoked.sort_unstable();
    assert_eq!(invoked, expected, "ui/src/bindings.ts");
}

/// R-OPS-03, CR-004: MSI and per-machine NSIS installers. Development builds use WebView2 mode
/// `skip` (no runtime files needed for tests and clippy); release builds overlay
/// tauri.release.conf.json, which ships the pinned WebView2 Fixed Version runtime.
#[test]
fn windows_installers_ship_pinned_webview2_fixed_runtime_in_release() {
    let conf = conf();
    let targets: Vec<&str> = conf["bundle"]["targets"]
        .as_array()
        .expect("bundle targets")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(targets, ["msi", "nsis"]);
    let windows = &conf["bundle"]["windows"];
    assert_eq!(windows["webviewInstallMode"]["type"], "skip");
    assert_eq!(windows["nsis"]["installMode"], "perMachine");
    assert_eq!(conf["bundle"]["license"], "Apache-2.0");

    let release = read_json(&crate_dir().join("tauri.release.conf.json"));
    let mode = &release["bundle"]["windows"]["webviewInstallMode"];
    assert_eq!(mode["type"], "fixedRuntime");
    assert_eq!(mode["path"], "./webview2-fixed-runtime/");
}

/// CR-004: the ~670 MB runtime is expanded from the tool cache, never committed to git.
#[test]
fn webview2_runtime_directory_is_git_ignored() {
    let gitignore = std::fs::read_to_string(crate_dir().join("../.gitignore")).expect(".gitignore");
    assert!(
        gitignore
            .lines()
            .any(|l| l.trim() == "/src-tauri/webview2-fixed-runtime/"),
        ".gitignore must exclude src-tauri/webview2-fixed-runtime/"
    );
}
