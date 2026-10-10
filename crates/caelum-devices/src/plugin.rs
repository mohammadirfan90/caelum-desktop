//! Adapter plug-in SDK, version 1 (F-ACQ-12; CR-026): vendor decoders outside the core.
//!
//! A plug-in is a **decoder only**. CAELUM owns the device link (connection, notifications,
//! reconnection) and hands the plug-in each frame; the plug-in returns readings. It gets no handle
//! to any device, so it cannot write to one (CLAUDE.md rule 5). Every reading it returns goes
//! through normal ingest (units, plausibility, time, SR-017), and only parameters its signed
//! manifest declares are accepted (the core checks; see `caelum-core::station::plugins`).
//!
//! Two forms:
//! - **In process:** a Rust type implementing [`AdapterPlugin`], compiled into the build.
//! - **Isolated process:** an executable speaking one JSON line per frame on stdin/stdout. Its
//!   host lives in `caelum-core::plugin_host` (local IPC to a child process, not a device link). A crash, a malformed reply or a reply later than
//!   [`FRAME_TIME_LIMIT`] yields an error for that frame and never takes the core down; the
//!   process is restarted on the next frame.
//!
//! Wire protocol (version [`SDK_VERSION`]):
//! - request: `{"sdk":1,"frame":"<hex>"}`
//! - reply: `{"readings":[{"parameter":"heart_rate","value":72,"unit":"/min",
//!   "device_time_tai_ns":null,"quality":"good","demonstration":false,"test_data":false}]}`
//!   or `{"error":"<reason>"}`.

use std::time::Duration;

use caelum_domain::observation::Quality;
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_domain::units::Unit;
use caelum_ingest::{DeviceFlags, RawValue};
use serde::{Deserialize, Serialize};

use crate::sig::Decoded;

/// The SDK version this build speaks.
pub const SDK_VERSION: u32 = 1;
/// Longest a process plug-in may take for one frame.
pub const FRAME_TIME_LIMIT: Duration = Duration::from_millis(250);
/// Largest frame handed to a plug-in.
pub const MAX_FRAME_LEN: usize = 4096;

/// A plug-in failure for one frame (the frame is dropped and counted; nothing is made up).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginError {
    /// The plug-in reported it cannot decode the frame.
    #[error("plug-in refused the frame: {0}")]
    Refused(String),
    /// The reply is not valid.
    #[error("plug-in reply is malformed: {0}")]
    Malformed(String),
    /// The process did not reply in time, exited, or could not start.
    #[error("plug-in process: {0}")]
    Process(String),
    /// The frame is larger than [`MAX_FRAME_LEN`].
    #[error("frame too long")]
    FrameTooLong,
}

/// A vendor decoder (F-ACQ-12). Implementations must be deterministic functions of the frames they
/// are given and must not do I/O.
pub trait AdapterPlugin: Send {
    /// The plug-in's ID (as in its signed manifest).
    fn id(&self) -> &str;
    /// The SDK version it was written for.
    fn sdk_version(&self) -> u32;
    /// Decodes one frame into readings.
    fn decode(&mut self, frame: &[u8]) -> Result<Vec<Decoded>, PluginError>;
}

/// One reading on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireReading {
    /// Catalog key.
    pub parameter: String,
    /// Number, or `null` when the device sent no usable value.
    pub value: Option<f64>,
    /// State (enumerated parameters).
    #[serde(default)]
    pub state: Option<String>,
    /// UCUM unit as the device stated it.
    pub unit: Option<String>,
    /// Device time (TAI ns since 1958), if the device's clock is set.
    pub device_time_tai_ns: Option<i64>,
    /// `good`, `degraded`, `poor` or `unusable`.
    pub quality: String,
    /// The device marks the reading as demonstration data.
    pub demonstration: bool,
    /// The device marks the reading as test data.
    pub test_data: bool,
}

/// One reply on the wire.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireReply {
    /// The readings.
    #[serde(default)]
    pub readings: Option<Vec<WireReading>>,
    /// Why the frame could not be decoded.
    #[serde(default)]
    pub error: Option<String>,
}

/// Lower-case hex of a frame (the request encoding).
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A wire reading as a decoded reading; unknown parameter, unit or quality is refused.
pub fn from_wire(r: &WireReading) -> Result<Decoded, PluginError> {
    let parameter = Parameter::from_key(&r.parameter)
        .ok_or_else(|| PluginError::Malformed(format!("unknown parameter {}", r.parameter)))?;
    let quality = match r.quality.as_str() {
        "good" => Quality::Good,
        "degraded" => Quality::Degraded,
        "poor" => Quality::Poor,
        "unusable" => Quality::Unusable,
        q => return Err(PluginError::Malformed(format!("unknown quality {q}"))),
    };
    let unit = match &r.unit {
        None => None,
        Some(u) => Some(
            Unit::from_ucum(u)
                .ok_or_else(|| PluginError::Malformed(format!("unknown unit {u}")))?,
        ),
    };
    let value = match (&r.value, &r.state) {
        (Some(v), None) if v.is_finite() => Some(RawValue::Number(*v)),
        (None, Some(s)) => Some(RawValue::State(s.clone())),
        (None, None) => None,
        _ => {
            return Err(PluginError::Malformed(
                "a reading has a value or a state, not both".into(),
            ))
        }
    };
    Ok(Decoded {
        parameter,
        value,
        unit,
        device_time: r.device_time_tai_ns.map(TaiNs),
        quality,
        flags: DeviceFlags {
            demonstration: r.demonstration,
            test_data: r.test_data,
        },
        site: None,
    })
}
