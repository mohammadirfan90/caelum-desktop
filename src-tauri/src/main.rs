//! CAELUM Desktop executable. All logic lives in `caelum_desktop_lib` and the core crates.

// No console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(error) = caelum_desktop_lib::run() {
        eprintln!("CAELUM failed to start: {error}");
        std::process::exit(1);
    }
}
