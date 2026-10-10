//! The device registry: model, serial, firmware, accuracy class, calibration due and assignment to
//! a crew member (F-ACQ-09). A reading belongs to the crew member its device is assigned to; on a
//! multi-user device only the assigned user slot counts (CR-008 §4). Who may assign is decided at
//! the command layer (re-authentication required, `docs/ROLE_MATRIX.md` §4), which also audits it.

use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::time::TaiNs;
use caelum_ingest::ClockOffset;
use caelum_store::{Database, DeviceRecord, StoreError};

/// What identifies a device and what its documentation says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    /// Adapter that talks to it (e.g. `ble-sig`).
    pub adapter: String,
    /// Vendor.
    pub vendor: String,
    /// Model.
    pub model: String,
    /// Serial number.
    pub serial: String,
    /// Firmware version, if the device reports it.
    pub firmware: Option<String>,
    /// Accuracy class from the device's documentation, if known.
    pub accuracy_class: Option<String>,
    /// Calibration due date, if the device needs calibration.
    pub calibration_due: Option<TaiNs>,
}

/// Registry status of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    /// Known, not connected yet.
    Registered,
    /// Connected and delivering data.
    Connected,
    /// Was connected; link lost.
    Disconnected,
}

impl DeviceStatus {
    /// Stored name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
        }
    }
}

/// Registry errors.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// No such device.
    #[error("device {0} is not registered")]
    UnknownDevice(DeviceId),
    /// A user slot must be 0..=254 (255 means "unknown user" on the wire).
    #[error("device user slot {0} is not valid")]
    InvalidUserSlot(u8),
    /// A stored row is malformed.
    #[error("malformed device record {0}")]
    Malformed(String),
    /// Storage failure (also: unknown crew member, refused by the foreign key).
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// The registry over one database.
pub struct Registry<'a> {
    db: &'a Database,
}

impl<'a> Registry<'a> {
    /// The registry of `db`.
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Registers a device, or returns the existing one with the same vendor, model and serial.
    pub fn register(&self, identity: &DeviceIdentity) -> Result<DeviceId, RegistryError> {
        if let Some(existing) =
            self.db
                .device_by_identity(&identity.vendor, &identity.model, &identity.serial)?
        {
            return parse_id(&existing.id);
        }
        let id = DeviceId::from_uuid(uuid::Uuid::new_v4());
        self.db.insert_device(&DeviceRecord {
            id: id.to_string(),
            adapter: identity.adapter.clone(),
            model: identity.model.clone(),
            vendor: identity.vendor.clone(),
            serial: identity.serial.clone(),
            firmware: identity.firmware.clone(),
            accuracy_class: identity.accuracy_class.clone(),
            calibration_due_tai: identity.calibration_due.map(|t| t.0),
            crew_member_id: None,
            device_user: None,
            status: DeviceStatus::Registered.as_str().to_owned(),
            last_seen_tai: None,
            clock_offset_ms: None,
        })?;
        Ok(id)
    }

    /// Assigns a device (and, on a multi-user device, its user slot) to a crew member.
    pub fn assign(
        &self,
        device: DeviceId,
        crew_member: CrewMemberId,
        device_user: Option<u8>,
    ) -> Result<(), RegistryError> {
        if let Some(slot) = device_user.filter(|s| *s == 0xFF) {
            return Err(RegistryError::InvalidUserSlot(slot));
        }
        let crew = crew_member.to_string();
        if !self
            .db
            .set_device_assignment(&device.to_string(), Some(&crew), device_user)?
        {
            return Err(RegistryError::UnknownDevice(device));
        }
        Ok(())
    }

    /// Removes a device's assignment; its readings then belong to nobody and are rejected.
    pub fn unassign(&self, device: DeviceId) -> Result<(), RegistryError> {
        if !self
            .db
            .set_device_assignment(&device.to_string(), None, None)?
        {
            return Err(RegistryError::UnknownDevice(device));
        }
        Ok(())
    }

    /// The crew member a reading belongs to. `reading_user` is the user number the device sent
    /// with it. On a device with an assigned slot, only that slot counts; a device without a slot
    /// accepts only readings that carry no user number (CR-008 §4). `None`: nobody.
    pub fn owner(
        &self,
        device: DeviceId,
        reading_user: Option<u8>,
    ) -> Result<Option<CrewMemberId>, RegistryError> {
        let record = self
            .db
            .device(&device.to_string())?
            .ok_or(RegistryError::UnknownDevice(device))?;
        let Some(crew) = record.crew_member_id.as_deref() else {
            return Ok(None);
        };
        if record.device_user != reading_user {
            return Ok(None);
        }
        let crew = crew
            .parse::<CrewMemberId>()
            .map_err(|_| RegistryError::Malformed(record.id.clone()))?;
        Ok(Some(crew))
    }

    /// Records link status, the time data last arrived and the latest clock offset (F-ACQ-10/11).
    pub fn record_status(
        &self,
        device: DeviceId,
        status: DeviceStatus,
        seen: Option<TaiNs>,
        offset: Option<&ClockOffset>,
    ) -> Result<(), RegistryError> {
        let offset_ms = offset.map(|o| o.offset_ns as f64 / 1_000_000.0);
        if !self.db.set_device_status(
            &device.to_string(),
            status.as_str(),
            seen.map(|t| t.0),
            offset_ms,
        )? {
            return Err(RegistryError::UnknownDevice(device));
        }
        Ok(())
    }

    /// Every registered device.
    pub fn list(&self) -> Result<Vec<DeviceRecord>, RegistryError> {
        Ok(self.db.devices()?)
    }
}

fn parse_id(id: &str) -> Result<DeviceId, RegistryError> {
    id.parse()
        .map_err(|_| RegistryError::Malformed(id.to_owned()))
}
