//! IPC commands. Commands are thin: logic lives in the core crates (docs/03_ARCHITECTURE.md §1).
//!
//! `app_info` is public (shown before login); `docs/ROLE_MATRIX.md` (M1) lists it as such.
//! Every later command is authenticated and authorised (CLAUDE.md rule 6).

use serde::Serialize;
use specta::Type;

/// Product name shown in the window; must match `productName` in `tauri.conf.json`.
pub const PRODUCT_NAME: &str = "CAELUM";

/// Application identity shown in the main window (M0, F-SYS-06).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// Product name.
    pub product_name: String,
    /// Application version, taken from the Cargo workspace (single source of truth).
    pub version: String,
}

/// Returns the product name and version of this build.
#[tauri::command]
#[specta::specta]
pub fn app_info() -> AppInfo {
    AppInfo {
        product_name: PRODUCT_NAME.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F-SYS-06: the version shown comes from the Cargo workspace version.
    #[test]
    fn app_info_reports_workspace_version_and_product_name() {
        let info = app_info();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.version, "0.1.0");
        assert_eq!(info.product_name, "CAELUM");
    }
}
