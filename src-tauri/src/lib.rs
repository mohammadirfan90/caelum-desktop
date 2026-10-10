//! CAELUM Desktop application shell (Tauri 2): window, IPC commands and application state.
//!
//! The Rust core owns all data and logic; this crate only wires it to the WebView UI
//! (docs/03_ARCHITECTURE.md §1). Only [`ipc`] names `tauri-specta` (docs/third-party-risk.md).
//!
//! M0: F-SYS-06, R-OPS-03. M1: sign-in and audit commands (F-SYS-01/02/03, R-SEC-01..03).

// CLAUDE.md rule 3, R-CLN-02: test equipment can never be compiled into a release build.
#[cfg(all(feature = "test-equipment", not(debug_assertions)))]
compile_error!(
    "the `test-equipment` feature must never be enabled in a release build (CLAUDE.md rule 3)"
);

use tauri::Manager;

pub mod commands;
pub mod events;
pub mod ipc;
pub mod state;

/// Builds and runs the application. Errors are returned, never unwrapped.
///
/// Records live in the per-user local (non-roaming) data directory, encrypted with the key from
/// the operating-system keystore.
pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .setup(|app| {
            ipc::mount_events(app);
            let data_dir = app.path().app_local_data_dir()?;
            app.manage(state::AppState::open(&data_dir));
            state::start_alarm_tick(app.handle().clone());
            Ok(())
        })
        .invoke_handler(ipc::invoke_handler())
        .run(tauri::generate_context!())
}
