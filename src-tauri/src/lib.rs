//! CAELUM Desktop application shell (Tauri 2): window, IPC commands and application state.
//!
//! The Rust core owns all data and logic; this crate only wires it to the WebView UI
//! (docs/03_ARCHITECTURE.md §1). Only [`ipc`] names `tauri-specta` (docs/third-party-risk.md).
//!
//! M0: F-SYS-06, R-OPS-03.

// CLAUDE.md rule 3, R-CLN-02: test equipment can never be compiled into a release build.
#[cfg(all(feature = "test-equipment", not(debug_assertions)))]
compile_error!(
    "the `test-equipment` feature must never be enabled in a release build (CLAUDE.md rule 3)"
);

pub mod commands;
pub mod ipc;

/// Builds and runs the application. Errors are returned, never unwrapped.
pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .invoke_handler(ipc::invoke_handler())
        .run(tauri::generate_context!())
}
