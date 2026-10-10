//! The device runtime and the scan/pair commands (F-ACQ-01, F-ACQ-09, F-ACQ-10; CR-012 §4, §8).
//!
//! Every 5 s the runtime looks for assigned devices it is not yet running and starts one task per
//! device: a BLE device is supervised by `BleCentral` (connect, notifications per WX-001,
//! reconnect); in `test-equipment` builds only, an emulated device plays the scenario file named
//! by `CAELUM_TE_SCENARIO`. Each notification goes through the station synchronously (acquisition,
//! ingest, store, alarm engine); the window gets the alarm bus and value-free change events.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use caelum_core::devices::ble::{
    BleCentral, BleEvent, Discovered, IMMEDIATE_ALERT_SERVICE, WRISTBAND_ADAPTER,
};
use caelum_core::devices::link::{LinkSupervisor, ReconnectPolicy};
use caelum_core::devices::pairing::{self, BtAddress, PairingOutcome};
use caelum_core::devices::registry::DeviceIdentity;
use caelum_core::station::Ingested;
use caelum_core::store::DeviceRecord;
use caelum_domain::ids::DeviceId;
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event as _;

use crate::commands::{CommandError, ErrorKind};
use crate::events::{bus_events, DevicesChanged, ObservationsChanged};
use crate::state::AppState;

/// How often the runtime looks for devices to start.
const POLL: Duration = Duration::from_secs(5);
/// Scan length for `device_scan` and for finding a not-yet-known peripheral.
const SCAN: Duration = Duration::from_secs(5);

/// Address prefix of emulated devices (test equipment only).
pub const TEST_EQUIPMENT_ADDRESS: &str = "TE:00:00:00:00:01";

/// Runtime state managed by Tauri.
#[derive(Default)]
pub struct DeviceRuntime {
    central: tokio::sync::OnceCell<Option<Arc<BleCentral>>>,
    running: Mutex<HashSet<String>>,
}

impl DeviceRuntime {
    async fn central(&self) -> Option<Arc<BleCentral>> {
        self.central
            .get_or_init(|| async { BleCentral::open().await.ok().map(Arc::new) })
            .await
            .clone()
    }

    fn claim(&self, id: &str) -> bool {
        self.running
            .lock()
            .is_ok_and(|mut r| r.insert(id.to_owned()))
    }

    fn release(&self, id: &str) {
        if let Ok(mut r) = self.running.lock() {
            r.remove(id);
        }
    }
}

fn emit_ingested(app: &AppHandle, ingested: &Ingested) {
    for e in bus_events(&ingested.output) {
        let _ = e.emit(app);
    }
    if let Some(crew) = &ingested.crew {
        let _ = ObservationsChanged {
            crew_id: crew.clone(),
        }
        .emit(app);
    }
}

fn link(app: &AppHandle, device: DeviceId, connected: bool) {
    let _ = app
        .state::<AppState>()
        .with(|s| Ok(s.device_link(device, connected)?));
    let _ = DevicesChanged {
        device_id: device.to_string(),
    }
    .emit(app);
}

fn ingest(app: &AppHandle, device: DeviceId, characteristic: u16, value: &[u8]) {
    if let Ok(ingested) = app
        .state::<AppState>()
        .with(|s| Ok(s.ingest_notification(device, characteristic, value)?))
    {
        emit_ingested(app, &ingested);
    }
}

/// Starts the device runtime (call once in `setup`).
pub fn start_device_runtime(app: AppHandle) {
    app.manage(DeviceRuntime::default());
    tauri::async_runtime::spawn(async move {
        loop {
            let devices = app
                .state::<AppState>()
                .with(|s| Ok(s.runtime_devices()?))
                .unwrap_or_default();
            for d in devices {
                let runtime = app.state::<DeviceRuntime>();
                if runtime.claim(&d.id) {
                    start_device(app.clone(), d);
                }
            }
            tokio::time::sleep(POLL).await;
        }
    });
}

fn start_device(app: AppHandle, d: DeviceRecord) {
    let Ok(device) = d.id.parse::<DeviceId>() else {
        return;
    };
    #[cfg(feature = "test-equipment")]
    if d.adapter == caelum_core::acquisition::TEST_EQUIPMENT_ADAPTER {
        tauri::async_runtime::spawn(play_scenario(app, device));
        return;
    }
    if d.adapter == caelum_core::acquisition::TEST_EQUIPMENT_ADAPTER {
        // Not a test-equipment build: emulated devices are never run (CLAUDE.md rule 3).
        return;
    }
    tauri::async_runtime::spawn(async move {
        let runtime = app.state::<DeviceRuntime>();
        let Some(central) = runtime.central().await else {
            runtime.release(&d.id);
            return;
        };
        let mut peripheral = central.id_for_address(&d.serial).await.ok().flatten();
        if peripheral.is_none() {
            let _ = central.scan(SCAN).await;
            peripheral = central.id_for_address(&d.serial).await.ok().flatten();
        }
        let Some(peripheral) = peripheral else {
            // Not in range yet: try again on the next poll.
            runtime.release(&d.id);
            return;
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel::<BleEvent>(64);
        let supervisor = central.clone();
        tauri::async_runtime::spawn(async move {
            let link = LinkSupervisor::new(ReconnectPolicy::default(), 0, None);
            supervisor
                .supervise(&peripheral, link, Instant::now(), tx)
                .await;
        });
        while let Some(event) = rx.recv().await {
            match event {
                BleEvent::Connected { .. } => link(&app, device, true),
                BleEvent::Disconnected | BleEvent::ConnectFailed(_) => link(&app, device, false),
                BleEvent::Notification {
                    characteristic,
                    value,
                    ..
                } => ingest(&app, device, characteristic, &value),
                // F-ACQ-05: CAELUM sensor kit (ICD-SENSOR-KIT).
                BleEvent::KitFrame { value, .. } => {
                    if let Ok(ingested) = app
                        .state::<AppState>()
                        .with(|s| Ok(s.ingest_sensor_kit(device, &value)?))
                    {
                        emit_ingested(&app, &ingested);
                    }
                }
                BleEvent::KitInfo(value) => {
                    let _ = app
                        .state::<AppState>()
                        .with(|s| Ok(s.sensor_kit_info(device, &value)?));
                }
                BleEvent::Rssi(_) => {}
            }
        }
        runtime.release(&d.id);
    });
}

/// Test equipment only: plays `CAELUM_TE_SCENARIO` as the emulated device's notifications.
#[cfg(feature = "test-equipment")]
async fn play_scenario(app: AppHandle, device: DeviceId) {
    let steps = std::env::var("CAELUM_TE_SCENARIO")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| caelum_core::test_equipment::scenario::parse(&text).ok())
        .unwrap_or_default();
    link(&app, device, true);
    let start = Instant::now();
    for step in steps {
        let due = Duration::from_millis(step.at_ms);
        let elapsed = start.elapsed();
        if due > elapsed {
            tokio::time::sleep(due - elapsed).await;
        }
        ingest(&app, device, step.characteristic, &step.payload);
    }
}

/// A device found by a scan.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct NearbyDevice {
    /// Bluetooth address.
    pub address: String,
    /// Advertised name.
    pub name: Option<String>,
    /// Signal strength, dBm.
    pub rssi: Option<i16>,
    /// Advertised health services (16-bit UUIDs).
    pub services: Vec<u16>,
    /// An emulated device (test equipment builds only).
    pub emulated: bool,
    /// A CAELUM sensor-kit device (F-ACQ-05).
    pub sensor_kit: bool,
}

impl From<Discovered> for NearbyDevice {
    fn from(d: Discovered) -> Self {
        Self {
            address: d.address,
            name: d.name,
            rssi: d.rssi,
            services: d.services,
            emulated: false,
            sensor_kit: d.sensor_kit,
        }
    }
}

/// Result of pairing.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum PairResult {
    /// Paired (or already paired) and registered; assign it next.
    #[serde(rename_all = "camelCase")]
    Registered {
        /// Device ID.
        device_id: String,
    },
    /// Pair it in the operating system settings first, then retry.
    #[serde(rename_all = "camelCase")]
    NeedsOsPairing {
        /// Why.
        reason: String,
        /// Steps for this platform.
        steps: Vec<String>,
    },
}

fn failed(message: String) -> CommandError {
    CommandError {
        kind: ErrorKind::Failed,
        message,
        retry_after_s: None,
    }
}

/// Scans for health devices nearby (CMO, BME, administrator).
#[tauri::command]
#[specta::specta]
pub async fn device_scan(
    state: State<'_, AppState>,
    runtime: State<'_, DeviceRuntime>,
) -> Result<Vec<NearbyDevice>, CommandError> {
    state.with(|s| Ok(s.device_scan_allowed()?))?;
    #[allow(unused_mut)]
    let mut found: Vec<NearbyDevice> = match runtime.central().await {
        Some(central) => central
            .scan(SCAN)
            .await
            .map_err(|e| failed(e.to_string()))?
            .into_iter()
            .map(Into::into)
            .collect(),
        None => Vec::new(),
    };
    #[cfg(feature = "test-equipment")]
    found.push(NearbyDevice {
        address: TEST_EQUIPMENT_ADDRESS.to_owned(),
        name: Some("Test equipment oximeter".to_owned()),
        rssi: None,
        services: vec![0x1822],
        emulated: true,
        sensor_kit: false,
    });
    Ok(found)
}

/// Pairs (OS bonding) and registers a scanned device (CMO, BME, administrator; audited).
#[tauri::command]
#[specta::specta]
pub async fn device_pair(
    state: State<'_, AppState>,
    app: AppHandle,
    address: String,
    name: Option<String>,
    services: Vec<u16>,
    sensor_kit: bool,
) -> Result<PairResult, CommandError> {
    state.with(|s| Ok(s.device_scan_allowed()?))?;
    let emulated = address == TEST_EQUIPMENT_ADDRESS;
    let adapter = if emulated {
        if !cfg!(feature = "test-equipment") {
            return Err(failed(
                "emulated devices exist only in test builds".to_owned(),
            ));
        }
        caelum_core::acquisition::TEST_EQUIPMENT_ADAPTER
    } else if sensor_kit {
        caelum_core::devices::ble::SENSOR_KIT_ADAPTER
    } else if services.contains(&IMMEDIATE_ALERT_SERVICE) {
        WRISTBAND_ADAPTER
    } else {
        caelum_core::station::BLE_SIG_ADAPTER
    };
    if !emulated {
        let parsed = BtAddress::parse(&address).ok_or_else(|| CommandError {
            kind: ErrorKind::Rejected,
            message: "not a Bluetooth address".to_owned(),
            retry_after_s: None,
        })?;
        match pairing::pair(parsed)
            .await
            .map_err(|e| failed(e.to_string()))?
        {
            PairingOutcome::Paired
            | PairingOutcome::AlreadyPaired
            | PairingOutcome::OnFirstAccess => {}
            PairingOutcome::NeedsOsPairing(reason) => {
                return Ok(PairResult::NeedsOsPairing {
                    reason,
                    steps: pairing::os_pairing_guide()
                        .steps
                        .iter()
                        .map(|s| (*s).to_owned())
                        .collect(),
                })
            }
        }
    }
    // Missing identity is "unknown", never made up (SR-018); the address identifies the device.
    let identity = DeviceIdentity {
        adapter: adapter.to_owned(),
        vendor: "unknown".to_owned(),
        model: name.unwrap_or_else(|| "unknown".to_owned()),
        serial: address,
        firmware: None,
        accuracy_class: None,
        calibration_due: None,
    };
    let id = state.with(|s| Ok(s.device_register(&identity)?))?;
    let _ = DevicesChanged {
        device_id: id.to_string(),
    }
    .emit(&app);
    Ok(PairResult::Registered {
        device_id: id.to_string(),
    })
}
