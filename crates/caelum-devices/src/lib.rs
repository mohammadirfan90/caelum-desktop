//! Device adapters: adapter trait, registry, BLE central, pairing modules, USB/serial, GHS, Bluetooth
//! SIG profiles, sensor-kit protocol, DICOM, ECG files and FHIR import.
//!
//! Adapters are read-only except for calls listed in docs/write-exceptions.md (R-SEC-07, SR-011).
//!
//! Implemented in M2, M7 and M8: F-ACQ-01..11.

pub mod ble;
pub mod hid;
pub mod link;
pub mod pairing;
pub mod registry;
pub mod serial;
pub mod sig;

use caelum_domain::ids::DeviceId;
use caelum_domain::observation::MeasurementMode;
use caelum_domain::time::TaiNs;
use caelum_ingest::RawReading;

/// Turns a decoded value into the reading ingest validates (docs/03 §3 step 1).
pub fn raw_reading(
    decoded: sig::Decoded,
    device_id: DeviceId,
    received_at: TaiNs,
    mode: MeasurementMode,
) -> RawReading {
    RawReading {
        device_id,
        parameter: decoded.parameter,
        value: decoded.value,
        unit: decoded.unit,
        device_time: decoded.device_time,
        received_at,
        quality: decoded.quality,
        mode,
        flags: decoded.flags,
        site: decoded.site,
    }
}
