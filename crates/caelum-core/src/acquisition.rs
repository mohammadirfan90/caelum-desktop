//! Acquisition path (docs/03 §3 steps 1–3): a device notification is decoded, filed under the
//! crew member its device is assigned to, validated by ingest and appended to the database.
//! Rejected readings are counted per device and reason (dropped readings, F-ACQ-10); nothing is
//! stored for them (R-CLN-02).

use std::collections::HashMap;

use caelum_devices::raw_reading;
use caelum_devices::registry::{DeviceStatus, Registry, RegistryError};
use caelum_devices::sig::{self, DecodeError};
use caelum_domain::ids::DeviceId;
use caelum_domain::observation::{Classification, MeasurementMode, Quality};
use caelum_domain::time::{LeapSeconds, TaiNs, TimeError};
use caelum_ingest::{ingest, ClockOffset, Observation, Rejection, Value};
use caelum_store::{Database, ObservationRecord, StoreError};

/// Adapter name of emulated devices (`caelum-test-equipment`). Their readings are always marked
/// test data: rejected in release builds, stored as `simulated` in test-equipment builds.
pub const TEST_EQUIPMENT_ADAPTER: &str = "test-equipment";

/// Acquisition errors (the notification could not be processed at all).
#[derive(Debug, thiserror::Error)]
pub enum AcquisitionError {
    /// Registry failure (unknown device, storage).
    #[error(transparent)]
    Registry(#[from] RegistryError),
    /// Storage failure.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Context could not be serialised.
    #[error("context: {0}")]
    Context(String),
}

/// What happened to one notification.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// IDs of the stored observations.
    pub stored: Vec<String>,
    /// Readings that were rejected, with the reason.
    pub rejected: Vec<Rejection>,
    /// Set if the payload could not be decoded (then nothing was stored).
    pub malformed: Option<DecodeError>,
}

/// Acquisition state: the leap-second table for device timestamps, each device's measured clock
/// offset, and dropped-reading counters.
pub struct Acquisition {
    leap: LeapSeconds,
    offsets: HashMap<DeviceId, ClockOffset>,
    dropped: HashMap<(DeviceId, &'static str), u64>,
}

impl Acquisition {
    /// New acquisition state with the bundled leap-second table.
    pub fn new() -> Result<Self, TimeError> {
        Ok(Self {
            leap: LeapSeconds::bundled()?,
            offsets: HashMap::new(),
            dropped: HashMap::new(),
        })
    }

    /// Records a device's measured clock offset (F-ACQ-11).
    pub fn set_clock_offset(&mut self, device: DeviceId, offset: ClockOffset) {
        self.offsets.insert(device, offset);
    }

    /// Dropped readings for a device, by reason code (F-ACQ-10).
    pub fn dropped(&self, device: DeviceId) -> Vec<(&'static str, u64)> {
        let mut out: Vec<_> = self
            .dropped
            .iter()
            .filter(|((d, _), _)| *d == device)
            .map(|((_, code), n)| (*code, *n))
            .collect();
        out.sort_unstable();
        out
    }

    fn count(&mut self, device: DeviceId, code: &'static str) {
        *self.dropped.entry((device, code)).or_default() += 1;
    }

    /// Processes one notification from a registered device.
    pub fn notification(
        &mut self,
        db: &Database,
        device: DeviceId,
        characteristic: u16,
        payload: &[u8],
        received_at: TaiNs,
    ) -> Result<Outcome, AcquisitionError> {
        let registry = Registry::new(db);
        let record = db
            .device(&device.to_string())?
            .ok_or(RegistryError::UnknownDevice(device))?;
        registry.record_status(device, DeviceStatus::Connected, Some(received_at), None)?;
        let measurement = match sig::decode(characteristic, payload, &self.leap) {
            Ok(m) => m,
            Err(e) => {
                self.count(device, "malformed");
                return Ok(Outcome {
                    malformed: Some(e),
                    ..Outcome::default()
                });
            }
        };
        let owner = registry.owner(device, measurement.device_user)?;
        let emulated = record.adapter == TEST_EQUIPMENT_ADAPTER;
        let offset = self.offsets.get(&device).copied();
        let mut outcome = Outcome::default();
        for decoded in measurement.readings {
            let mut raw = raw_reading(decoded, device, received_at, measurement.mode);
            if emulated {
                raw.flags.test_data = true;
            }
            match ingest(raw, owner, offset.as_ref()) {
                Ok(obs) => {
                    let row = observation_record(&obs)?;
                    db.insert_observation(&row)?;
                    outcome.stored.push(row.id);
                }
                Err(rejection) => {
                    self.count(device, rejection.code());
                    outcome.rejected.push(rejection);
                }
            }
        }
        Ok(outcome)
    }
}

fn quality_name(q: Quality) -> &'static str {
    match q {
        Quality::Good => "good",
        Quality::Degraded => "degraded",
        Quality::Poor => "poor",
        Quality::Unusable => "unusable",
    }
}

fn classification_name(c: Classification) -> &'static str {
    match c {
        Classification::Measured => "measured",
        Classification::Derived => "derived",
        Classification::Entered => "entered",
        Classification::Imported => "imported",
        #[cfg(feature = "test-equipment")]
        Classification::Simulated => "simulated",
    }
}

fn mode_name(m: MeasurementMode) -> &'static str {
    match m {
        MeasurementMode::Continuous => "continuous",
        MeasurementMode::Spot => "spot",
    }
}

/// The database row for an observation (docs/05 §2), with provenance as JSON context.
pub fn observation_record(obs: &Observation) -> Result<ObservationRecord, AcquisitionError> {
    let (value, value_text) = match &obs.value {
        Value::Number(v) => (Some(*v), None),
        Value::State(s) => (None, Some((*s).to_owned())),
    };
    Ok(ObservationRecord {
        id: uuid::Uuid::new_v4().hyphenated().to_string(),
        crew_member_id: obs.crew_member_id.to_string(),
        device_id: Some(obs.device_id.to_string()),
        parameter: obs.parameter.spec().key.to_owned(),
        value,
        value_text,
        unit: obs.unit.ucum().to_owned(),
        measured_at_tai: obs.measured_at.0,
        received_at_tai: obs.received_at.0,
        quality: quality_name(obs.quality).to_owned(),
        classification: classification_name(obs.classification).to_owned(),
        measurement_mode: mode_name(obs.mode).to_owned(),
        algorithm_version: None,
        context: Some(
            serde_json::to_string(&obs.provenance)
                .map_err(|e| AcquisitionError::Context(e.to_string()))?,
        ),
    })
}
