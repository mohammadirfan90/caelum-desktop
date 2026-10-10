//! What decoders hand to ingest, and what ingest produces (docs/03 §3 steps 1–2).

use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::observation::{Classification, MeasurementMode, Quality, TemperatureSite};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_domain::units::Unit;
use serde::Serialize;

/// A decoded value as the device sent it.
#[derive(Debug, Clone, PartialEq)]
pub enum RawValue {
    /// A number in the device's unit.
    Number(f64),
    /// An enumerated state.
    State(String),
}

/// Flags a device sets on a reading (SR-017).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeviceFlags {
    /// The device marks the reading as demonstration data (e.g. PLXS status bit 10).
    pub demonstration: bool,
    /// The device marks the reading as test data (e.g. PLXS status bit 11).
    pub test_data: bool,
}

/// One reading from a decoder. Fields a device did not send stay `None`; ingest never fills them.
#[derive(Debug, Clone, PartialEq)]
pub struct RawReading {
    /// The device.
    pub device_id: DeviceId,
    /// The catalog parameter the decoder mapped the value to.
    pub parameter: Parameter,
    /// The value, if the device sent one.
    pub value: Option<RawValue>,
    /// The unit, if the device stated one (required for numbers; never assumed).
    pub unit: Option<Unit>,
    /// The device's own timestamp, on its clock, if it sent one.
    pub device_time: Option<TaiNs>,
    /// Station time the reading arrived.
    pub received_at: TaiNs,
    /// Signal quality as determined by the decoder from the device's indicators.
    pub quality: Quality,
    /// Continuous or spot, from the device profile.
    pub mode: MeasurementMode,
    /// Demonstration and test flags (SR-017).
    pub flags: DeviceFlags,
    /// Thermometer site, if the device reported one (only for `body_temp_measured`).
    pub site: Option<TemperatureSite>,
}

/// Which clock the stored time comes from (provenance of the time correction, docs/05 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TimeSource {
    /// Device time corrected by its measured offset.
    DeviceCorrected {
        /// The offset applied, ns.
        offset_ns: i64,
        /// Remaining error bound, ns.
        residual_ns: u64,
    },
    /// The device sent no time: station receipt time.
    Receipt,
    /// The device sent a time but its clock has no measured offset: station receipt time.
    ReceiptUnsynchronised,
}

/// Context stored with an observation (JSON column `context`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Provenance {
    /// How `measured_at` was obtained.
    pub time: TimeSource,
    /// The value and unit as the device sent them, before conversion.
    pub original: Option<Original>,
    /// Thermometer site (`unknown` when not reported), for `body_temp_measured` only.
    pub site: Option<TemperatureSite>,
}

/// The value as received.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Original {
    /// Value in the device's unit.
    pub value: f64,
    /// The device's UCUM unit.
    pub unit: &'static str,
}

/// A validated value in the catalog's canonical form.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A number in the parameter's canonical unit.
    Number(f64),
    /// One of the parameter's states.
    State(&'static str),
}

/// A validated, time-corrected observation ready to store and evaluate.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    /// Crew member the device is assigned to.
    pub crew_member_id: CrewMemberId,
    /// Source device.
    pub device_id: DeviceId,
    /// Parameter.
    pub parameter: Parameter,
    /// Value in canonical form.
    pub value: Value,
    /// Canonical unit of the parameter.
    pub unit: Unit,
    /// When it was measured (station TAI).
    pub measured_at: TaiNs,
    /// When it arrived (station TAI).
    pub received_at: TaiNs,
    /// Quality.
    pub quality: Quality,
    /// Provenance class.
    pub classification: Classification,
    /// Continuous or spot.
    pub mode: MeasurementMode,
    /// Context and provenance.
    pub provenance: Provenance,
}
