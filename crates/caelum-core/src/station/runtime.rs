//! The station side of the device runtime (docs/03 §3 steps 1–5; F-ACQ-01, F-ACQ-09, F-ACQ-10,
//! F-ALM-06): which devices to connect, each notification through acquisition, ingest and the
//! alarm engine synchronously, and link changes into the technical monitor. Pairing registers a
//! device after the role check (CR-012 §4). The radio work itself is in the application shell.

use caelum_devices::ble::{SENSOR_KIT_ADAPTER, WRISTBAND_ADAPTER};
use caelum_devices::registry::{DeviceIdentity, DeviceStatus, Registry};
use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::Command;
use caelum_store::DeviceRecord;
use serde_json::json;

use super::{Station, StationError};
use crate::acquisition::TEST_EQUIPMENT_ADAPTER;
use crate::alarms::Output;

/// Adapter of Bluetooth SIG health devices.
pub const BLE_SIG_ADAPTER: &str = "ble-sig";

/// Adapters the device runtime connects (test equipment only exists in test builds' scans).
pub const RUNTIME_ADAPTERS: [&str; 4] = [
    BLE_SIG_ADAPTER,
    WRISTBAND_ADAPTER,
    SENSOR_KIT_ADAPTER,
    TEST_EQUIPMENT_ADAPTER,
];

/// What one notification produced.
#[derive(Debug, Default)]
pub struct Ingested {
    /// Alarm events and notices for the alarm bus.
    pub output: Output,
    /// Crew member whose data changed (for the value-free `observationsChanged` event).
    pub crew: Option<String>,
    /// Readings stored.
    pub stored: usize,
}

impl Station {
    /// Devices the runtime should keep connected: registered with a runtime adapter and
    /// assigned to a crew member (an unassigned device's readings belong to nobody).
    pub fn runtime_devices(&self) -> Result<Vec<DeviceRecord>, StationError> {
        Ok(self
            .db
            .devices()?
            .into_iter()
            .filter(|d| {
                RUNTIME_ADAPTERS.contains(&d.adapter.as_str()) && d.crew_member_id.is_some()
            })
            .collect())
    }

    /// One notification from a connected device: decode, ingest, store, evaluate (no queue
    /// between data and safety logic, docs/03 §3 step 5).
    pub fn ingest_notification(
        &mut self,
        device: DeviceId,
        characteristic: u16,
        payload: &[u8],
    ) -> Result<Ingested, StationError> {
        let now = self.now()?;
        let outcome = self
            .acquisition
            .notification(&self.db, device, characteristic, payload, now)
            .map_err(|e| StationError::Rejected(e.to_string()))?;
        let mut ingested = Ingested {
            stored: outcome.observations.len(),
            ..Ingested::default()
        };
        for obs in &outcome.observations {
            ingested.crew = Some(obs.crew_member_id.to_string());
            let out = self.observe(obs)?;
            ingested.output.events.extend(out.events);
            ingested.output.notices.extend(out.notices);
        }
        // ECG waveforms (GHS, F-ACQ-03): stored, QRS → asystole events, SQI and HR derived.
        if let Some((crew, _)) = outcome.waveforms.first() {
            ingested.crew = Some(crew.to_string());
        }
        let out = self.ingest_waveforms(device, outcome.waveforms)?;
        ingested.output.events.extend(out.events);
        ingested.output.notices.extend(out.notices);
        Ok(ingested)
    }

    /// A device connected or lost its link (F-ACQ-10): recorded in the registry, and a lost link
    /// raises `technical.device_lost` after the CR-011 limit (on the tick).
    pub fn device_link(&mut self, device: DeviceId, connected: bool) -> Result<(), StationError> {
        let now = self.now()?;
        let record = self
            .db
            .device(&device.to_string())?
            .ok_or_else(|| StationError::Rejected("unknown device".to_owned()))?;
        let crew = record
            .crew_member_id
            .as_deref()
            .and_then(|c| c.parse::<CrewMemberId>().ok());
        let status = if connected {
            DeviceStatus::Connected
        } else {
            DeviceStatus::Disconnected
        };
        Registry::new(&self.db)
            .record_status(device, status, connected.then_some(now), None)
            .map_err(|e| StationError::Rejected(e.to_string()))?;
        let changes = self.monitor.link(device, crew, connected, now);
        self.apply_technical(changes, now)
    }

    /// `device_scan`: may the caller scan for devices (the radio work is the shell's)?
    pub fn device_scan_allowed(&mut self) -> Result<(), StationError> {
        self.user_for(Command::DeviceScan).map(|_| ())
    }

    /// `device_pair`: registers a device the shell has paired (or finds it already registered).
    /// Audited.
    pub fn device_register(&mut self, identity: &DeviceIdentity) -> Result<DeviceId, StationError> {
        let user = self.user_for(Command::DevicePair)?;
        let id = Registry::new(&self.db)
            .register(identity)
            .map_err(|e| StationError::Rejected(e.to_string()))?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::DevicePaired,
            Some(&id.to_string()),
            &json!({
                "adapter": identity.adapter,
                "vendor": identity.vendor,
                "model": identity.model,
                "serial": identity.serial,
            }),
        )?;
        Ok(id)
    }
}
