//! Application state: the station, opened once at start-up from the per-user local data directory
//! with keys from the operating-system keystore (F-SYS-02, R-SEC-01).
//!
//! If the station cannot open (keystore unavailable, key missing, database unreadable) the window
//! still starts and every command reports why; nothing is created in its place.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_specta::Event as _;

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

/// docs/03 §3: the alarm tick runs at 4 Hz.
pub const TICK: Duration = Duration::from_millis(250);

/// Starts the alarm tick on its own thread (database work stays off the async runtime): sounds
/// from before a restart first, then every tick's events to every window. Alarms do not depend
/// on sign-in, so the tick runs whatever the session state.
pub fn start_alarm_tick(app: AppHandle) {
    std::thread::spawn(move || {
        let emit = |out: &caelum_core::alarms::Output| {
            for e in crate::events::bus_events(out) {
                let _ = e.emit(&app);
            }
        };
        if let Ok(out) = app.state::<AppState>().with(|s| Ok(s.take_restored())) {
            emit(&out);
        }
        loop {
            std::thread::sleep(TICK);
            match app
                .state::<AppState>()
                .with(|s| s.tick().map_err(CommandError::from))
            {
                Ok(out) => emit(&out),
                // Station unavailable: the window already says why (every command reports it).
                Err(_) => std::thread::sleep(Duration::from_secs(1)),
            }
        }
    });
}
