//! Multiple windows (F-SYS-13): the waveform viewer opens in its own window, for example on a
//! second monitor. It uses the same station and session, so every value in it still comes from
//! an authorised command.

use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::commands::{CommandError, ErrorKind};
use crate::state::AppState;

/// Label of the waveform window (listed in `capabilities/main-window.json`).
pub const WAVEFORM_WINDOW: &str = "waveform";

/// Opens (or focuses) the waveform viewer window.
#[tauri::command]
#[specta::specta]
pub async fn window_open_waveform(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), CommandError> {
    state.with(|s| Ok(s.window_open_allowed()?))?;
    let failed = |e: tauri::Error| CommandError {
        kind: ErrorKind::Failed,
        message: e.to_string(),
        retry_after_s: None,
    };
    if let Some(w) = app.get_webview_window(WAVEFORM_WINDOW) {
        return w.set_focus().map_err(failed);
    }
    WebviewWindowBuilder::new(
        &app,
        WAVEFORM_WINDOW,
        WebviewUrl::App("index.html?view=waveform".into()),
    )
    .title("CAELUM ECG")
    .inner_size(1280.0, 480.0)
    .build()
    .map(|_| ())
    .map_err(failed)
}
