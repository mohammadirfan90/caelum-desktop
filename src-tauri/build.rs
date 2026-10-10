//! Tauri build script. The app manifest lists every IPC command so each one needs an explicit
//! permission in `capabilities/` (least privilege, docs/03_ARCHITECTURE.md §5).

fn main() {
    let attributes =
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "app_info",
            "auth_login",
            "auth_logout",
            "auth_me",
            "auth_reauth",
            "break_glass_open",
            "audit_list",
            "audit_verify",
            "ruleset_status",
            "alarm_acknowledge",
            "alarm_silence",
            "alarm_escalate",
        ]));
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri-build failed: {error:#}");
    }
}
