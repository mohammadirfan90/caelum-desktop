//! BLE central with btleplug (F-ACQ-01, F-ACQ-02): scan for health devices, connect, enable
//! notifications on the supported measurement characteristics, stream values, and reconnect
//! automatically under [`LinkSupervisor`].
//!
//! The only device write is enabling notifications or indications (CCCD), register entry WX-001
//! (docs/write-exceptions.md, category a). Nothing else is written: no characteristic writes, no
//! descriptor writes, no control points. Pairing and bonding are separate platform modules.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use btleplug::api::{
    Central, CentralEvent, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter,
};
use btleplug::platform::{Adapter, Manager, Peripheral, PeripheralId};
use futures::StreamExt;
use tokio::sync::mpsc;
use uuid::Uuid;

use caelum_domain::time::{LeapSeconds, TaiNs};
use caelum_ingest::SyncSample;

use crate::link::{Action, LinkSupervisor};
use crate::sig;

/// The Bluetooth SIG base UUID `0000xxxx-0000-1000-8000-00805F9B34FB`.
const SIG_BASE: u128 = 0x0000_0000_0000_1000_8000_0080_5F9B_34FB;

/// A 16-bit SIG UUID as a full UUID.
pub fn sig_uuid(short: u16) -> Uuid {
    Uuid::from_u128(SIG_BASE | (u128::from(short) << 96))
}

/// The 16-bit form of a SIG UUID, or `None` for vendor UUIDs.
pub fn short_uuid(uuid: Uuid) -> Option<u16> {
    let v = uuid.as_u128();
    let short = u16::try_from((v >> 96) & 0xFFFF).ok()?;
    (v & !(0xFFFF_u128 << 96) == SIG_BASE).then_some(short)
}

/// Health services CAELUM acquires from: HRS, PLXS, BPS, HTS, WSS, GLS.
pub const HEALTH_SERVICES: [u16; 6] = [0x180D, 0x1822, 0x1810, 0x1809, 0x181D, 0x1808];

/// A device seen while scanning.
#[derive(Debug, Clone, PartialEq)]
pub struct Discovered {
    /// Platform peripheral ID (stable per adapter).
    pub id: String,
    /// Bluetooth address (for pairing).
    pub address: String,
    /// Advertised name, if any.
    pub name: Option<String>,
    /// RSSI of the advertisement, dBm.
    pub rssi: Option<i16>,
    /// Advertised health services (16-bit).
    pub services: Vec<u16>,
}

/// Events from a supervised device.
#[derive(Debug, Clone, PartialEq)]
pub enum BleEvent {
    /// Connected and notifications enabled on these characteristics.
    Connected {
        /// Enabled measurement characteristics (16-bit).
        characteristics: Vec<u16>,
    },
    /// A measurement value arrived.
    Notification {
        /// Characteristic (16-bit).
        characteristic: u16,
        /// Raw value, for `sig::decode`.
        value: Vec<u8>,
        /// Monotonic arrival time.
        at: Instant,
    },
    /// Signal strength reported by the radio.
    Rssi(i16),
    /// The link was lost; the supervisor reconnects.
    Disconnected,
    /// An attempt failed (reason for device health).
    ConnectFailed(String),
}

/// BLE errors.
#[derive(Debug, thiserror::Error)]
pub enum BleError {
    /// No Bluetooth adapter.
    #[error("no Bluetooth adapter")]
    NoAdapter,
    /// The device offers none of the supported measurement characteristics.
    #[error("the device has no supported measurement characteristic")]
    NothingToAcquire,
    /// The device is not known to the adapter.
    #[error("device {0} not found")]
    NotFound(String),
    /// Platform failure.
    #[error("bluetooth: {0}")]
    Platform(String),
}

impl From<btleplug::Error> for BleError {
    fn from(e: btleplug::Error) -> Self {
        BleError::Platform(e.to_string())
    }
}

/// Which characteristics to enable: supported SIG measurement characteristics that can notify or
/// indicate. Everything else on the device is left alone.
pub fn measurement_characteristics<'a>(
    characteristics: impl IntoIterator<Item = (Uuid, bool)> + 'a,
) -> Vec<u16> {
    let mut out: BTreeSet<u16> = BTreeSet::new();
    for (uuid, can_notify_or_indicate) in characteristics {
        if let Some(short) = short_uuid(uuid) {
            if can_notify_or_indicate && sig::SUPPORTED.contains(&short) {
                out.insert(short);
            }
        }
    }
    out.into_iter().collect()
}

/// The first Bluetooth adapter of the system.
pub struct BleCentral {
    adapter: Adapter,
}

impl BleCentral {
    /// Opens the first adapter.
    pub async fn open() -> Result<Self, BleError> {
        let manager = Manager::new().await?;
        let adapter = manager
            .adapters()
            .await?
            .into_iter()
            .next()
            .ok_or(BleError::NoAdapter)?;
        Ok(Self { adapter })
    }

    /// Scans for `duration` and returns devices advertising a health service.
    pub async fn scan(&self, duration: Duration) -> Result<Vec<Discovered>, BleError> {
        let filter = ScanFilter {
            services: HEALTH_SERVICES.iter().map(|s| sig_uuid(*s)).collect(),
        };
        self.adapter.start_scan(filter).await?;
        tokio::time::sleep(duration).await;
        self.adapter.stop_scan().await?;
        let mut found = Vec::new();
        for p in self.adapter.peripherals().await? {
            let Some(props) = p.properties().await? else {
                continue;
            };
            let services: Vec<u16> = props
                .services
                .iter()
                .filter_map(|u| short_uuid(*u))
                .filter(|s| HEALTH_SERVICES.contains(s))
                .collect();
            if services.is_empty() {
                continue;
            }
            found.push(Discovered {
                id: p.id().to_string(),
                address: p.address().to_string(),
                name: props.local_name,
                rssi: props.rssi,
                services,
            });
        }
        Ok(found)
    }

    async fn peripheral(&self, id: &str) -> Result<Peripheral, BleError> {
        for p in self.adapter.peripherals().await? {
            if p.id().to_string() == id {
                return Ok(p);
            }
        }
        Err(BleError::NotFound(id.to_owned()))
    }

    /// Supervises one device until `events` is closed: connects, streams, and reconnects after a
    /// loss according to `link` (F-ACQ-01). `origin` is the monotonic zero of `link`.
    pub async fn supervise(
        &self,
        id: &str,
        mut link: LinkSupervisor,
        origin: Instant,
        events: mpsc::Sender<BleEvent>,
    ) {
        let now = || u64::try_from(origin.elapsed().as_nanos()).unwrap_or(u64::MAX);
        while !events.is_closed() {
            match link.poll(now()) {
                Action::WaitUntil(t) => {
                    tokio::time::sleep(Duration::from_nanos(t.saturating_sub(now()))).await;
                }
                Action::Receive => {
                    // `stream` returned while connected only on a transport error: treat as loss.
                    link.disconnected(now());
                }
                Action::Connect => match self.connect(id).await {
                    Ok((peripheral, enabled)) => {
                        link.connected(now());
                        if events
                            .send(BleEvent::Connected {
                                characteristics: enabled,
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                        self.stream(&peripheral, &mut link, &now, &events).await;
                        link.disconnected(now());
                        let _ = events.send(BleEvent::Disconnected).await;
                    }
                    Err(e) => {
                        link.connect_failed(now());
                        let _ = events.send(BleEvent::ConnectFailed(e.to_string())).await;
                    }
                },
            }
        }
    }

    /// Connects, discovers services and enables the measurement characteristics.
    async fn connect(&self, id: &str) -> Result<(Peripheral, Vec<u16>), BleError> {
        let peripheral = self.peripheral(id).await?;
        if !peripheral.is_connected().await? {
            peripheral.connect().await?;
        }
        peripheral.discover_services().await?;
        let characteristics: Vec<Characteristic> =
            peripheral.characteristics().into_iter().collect();
        let wanted = measurement_characteristics(characteristics.iter().map(|c| {
            let flags = c.properties;
            (
                c.uuid,
                flags.contains(CharPropFlags::NOTIFY) || flags.contains(CharPropFlags::INDICATE),
            )
        }));
        if wanted.is_empty() {
            let _ = peripheral.disconnect().await;
            return Err(BleError::NothingToAcquire);
        }
        for c in characteristics
            .iter()
            .filter(|c| short_uuid(c.uuid).is_some_and(|s| wanted.contains(&s)))
        {
            enable_notifications(&peripheral, c).await?;
        }
        Ok((peripheral, wanted))
    }

    /// One clock exchange with a connected device through its Current Time characteristic (a
    /// read). `station_now` gives station TAI. `None` if the device has no Current Time Service
    /// or its clock is not set.
    pub async fn clock_sample(
        &self,
        id: &str,
        leap: &LeapSeconds,
        station_now: impl Fn() -> Option<TaiNs>,
    ) -> Result<Option<SyncSample>, BleError> {
        let peripheral = self.peripheral(id).await?;
        let Some(cts) = peripheral
            .characteristics()
            .into_iter()
            .find(|c| short_uuid(c.uuid) == Some(sig::CURRENT_TIME))
        else {
            return Ok(None);
        };
        let Some(station_send) = station_now() else {
            return Ok(None);
        };
        let value = peripheral.read(&cts).await?;
        let Some(station_recv) = station_now() else {
            return Ok(None);
        };
        let device_time =
            sig::current_time(&value, leap).map_err(|e| BleError::Platform(e.to_string()))?;
        Ok(device_time.map(|device_time| SyncSample {
            station_send,
            device_time,
            station_recv,
        }))
    }

    /// Forwards notifications and RSSI until the device disconnects.
    async fn stream(
        &self,
        peripheral: &Peripheral,
        link: &mut LinkSupervisor,
        now: &impl Fn() -> u64,
        events: &mpsc::Sender<BleEvent>,
    ) {
        let id: PeripheralId = peripheral.id();
        let Ok(mut values) = peripheral.notifications().await else {
            return;
        };
        let Ok(mut central) = self.adapter.events().await else {
            return;
        };
        loop {
            tokio::select! {
                value = values.next() => {
                    let Some(v) = value else { return };
                    let Some(characteristic) = short_uuid(v.uuid) else { continue };
                    link.notification(now());
                    let event = BleEvent::Notification { characteristic, value: v.value, at: Instant::now() };
                    if events.send(event).await.is_err() {
                        return;
                    }
                }
                event = central.next() => match event {
                    Some(CentralEvent::DeviceDisconnected(d)) if d == id => return,
                    Some(CentralEvent::RssiUpdate { id: d, rssi }) if d == id => {
                        link.rssi(f64::from(rssi));
                        let _ = events.send(BleEvent::Rssi(rssi)).await;
                    }
                    Some(_) => {}
                    None => return,
                },
            }
        }
    }
}

/// Enables notifications or indications (CCCD write) on one measurement characteristic.
async fn enable_notifications(
    peripheral: &Peripheral,
    characteristic: &Characteristic,
) -> Result<(), BleError> {
    // WRITE-EXCEPTION: WX-001
    peripheral.subscribe(characteristic).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sig_uuids_round_trip_and_vendor_uuids_are_not_sig() {
        let hr = sig_uuid(0x2A37);
        assert_eq!(hr.to_string(), "00002a37-0000-1000-8000-00805f9b34fb");
        assert_eq!(short_uuid(hr), Some(0x2A37));
        let vendor = Uuid::from_u128(0x6E40_0001_B5A3_F393_E0A9_E50E_24DC_CA9E);
        assert_eq!(short_uuid(vendor), None);
    }

    /// WX-001 scope: only supported SIG measurement characteristics that notify or indicate are
    /// enabled; control points, vendor characteristics and battery are never touched.
    #[test]
    fn only_supported_measurement_characteristics_are_enabled() {
        let chars = [
            (sig_uuid(0x2A37), true),  // heart rate measurement
            (sig_uuid(0x2A39), false), // heart rate control point (write): never
            (sig_uuid(0x2A52), true),  // record access control point: not in WX-001
            (sig_uuid(0x2A19), true),  // battery level: not a measurement
            (sig_uuid(0x2A35), true),  // blood pressure measurement (indicate)
            (sig_uuid(0x2A5F), false), // continuous PLX without notify: cannot be enabled
            (Uuid::from_u128(1), true),
        ];
        assert_eq!(measurement_characteristics(chars), [0x2A35, 0x2A37]);
    }
}
