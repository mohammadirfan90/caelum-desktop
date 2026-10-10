//! Application state: the station, opened once at start-up from the per-user local data directory
//! with keys from the operating-system keystore (F-SYS-02, R-SEC-01).
//!
//! If the station cannot open (keystore unavailable, key missing, database unreadable) the window
//! still starts and every command reports why; nothing is created in its place.

use std::path::Path;
use std::sync::Mutex;

use caelum_core::security::clock::SystemClock;
use caelum_core::security::keystore::OsKeyStore;
use caelum_core::security::session::SessionConfig;
use caelum_core::station::{Station, StationError};

use crate::commands::{CommandError, ErrorKind};

/// Tauri-managed state.
pub struct AppState {
    station: Mutex<Result<Station, String>>,
}

impl AppState {
    /// Opens the station in `data_dir`.
    pub fn open(data_dir: &Path) -> Self {
        let station = open_station(data_dir).map_err(|e| e.to_string());
        Self {
            station: Mutex::new(station),
        }
    }

    /// State around an already opened station (tests).
    pub fn from_station(station: Station) -> Self {
        Self {
            station: Mutex::new(Ok(station)),
        }
    }

    /// Runs `f` on the station, or reports why it is unavailable.
    pub fn with<T>(
        &self,
        f: impl FnOnce(&mut Station) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        let mut guard = self.station.lock().map_err(|_| CommandError {
            kind: ErrorKind::Unavailable,
            message: "the station state is unavailable after an internal failure".to_owned(),
            retry_after_s: None,
        })?;
        match &mut *guard {
            Ok(station) => f(station),
            Err(reason) => Err(CommandError {
                kind: ErrorKind::Unavailable,
                message: format!("CAELUM could not open its records: {reason}"),
                retry_after_s: None,
            }),
        }
    }
}

fn open_station(data_dir: &Path) -> Result<Station, StationError> {
    let keystore = OsKeyStore::open().map_err(caelum_core::security::SecurityError::from)?;
    let clock = SystemClock::new().map_err(caelum_core::security::SecurityError::from)?;
    Station::open(
        data_dir,
        &keystore,
        Box::new(clock),
        station_name(),
        SessionConfig::default(),
    )
}

/// The computer's name, used as the break-glass audit target; `None` if the OS does not say.
fn station_name() -> Option<String> {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .map(|n| n.trim().to_owned())
        .find(|n| !n.is_empty())
}
