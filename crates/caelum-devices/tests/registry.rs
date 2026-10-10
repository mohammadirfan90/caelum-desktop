//! Device registry (F-ACQ-09, F-ACQ-10, F-ACQ-11; CR-008 §4 user slots).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use std::path::PathBuf;

use caelum_devices::registry::{DeviceIdentity, DeviceStatus, Registry, RegistryError};
use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::time::TaiNs;
use caelum_ingest::ClockOffset;
use caelum_store::{Database, DbKey};

/// An encrypted database in its own directory; closed before the directory is removed.
struct TempDb {
    db: Database,
    _guard: Guard,
}

struct Guard(PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl TempDb {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("caelum-devices-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("caelum.db"), &DbKey::from_bytes([7; 32])).unwrap();
        Self {
            db,
            _guard: Guard(dir),
        }
    }

    fn crew(&self, name: &str) -> CrewMemberId {
        let id = CrewMemberId::from_uuid(uuid::Uuid::new_v4());
        self.db
            .connection()
            .execute(
                "INSERT INTO crew_member (id, name, role, mission, active) VALUES (?1, ?2, 'crew', 'M-1', 1)",
                (id.to_string(), name),
            )
            .unwrap();
        id
    }
}

fn cuff() -> DeviceIdentity {
    DeviceIdentity {
        adapter: "ble-sig".into(),
        vendor: "Vendor".into(),
        model: "BP-1".into(),
        serial: "SN-001".into(),
        firmware: Some("1.2.3".into()),
        accuracy_class: Some("ISO 81060-2".into()),
        calibration_due: Some(TaiNs(5)),
    }
}

/// F-ACQ-09: model, serial, firmware, accuracy class and calibration due are kept; registering
/// the same device again returns the same ID.
#[test]
fn f_acq_09_register_is_idempotent_and_keeps_identity() {
    let t = TempDb::new();
    let reg = Registry::new(&t.db);
    let id = reg.register(&cuff()).unwrap();
    assert_eq!(reg.register(&cuff()).unwrap(), id);
    let list = reg.list().unwrap();
    assert_eq!(list.len(), 1);
    let d = &list[0];
    assert_eq!(
        (
            d.model.as_str(),
            d.serial.as_str(),
            d.firmware.as_deref(),
            d.accuracy_class.as_deref()
        ),
        ("BP-1", "SN-001", Some("1.2.3"), Some("ISO 81060-2"))
    );
    assert_eq!(d.calibration_due_tai, Some(5));
    assert_eq!(d.status, "registered");
    assert_eq!(d.crew_member_id, None, "a new device belongs to nobody");
}

/// F-ACQ-09: readings belong to the assigned crew member; unassigned devices belong to nobody.
#[test]
fn f_acq_09_assignment_decides_the_owner() {
    let t = TempDb::new();
    let reg = Registry::new(&t.db);
    let id = reg.register(&cuff()).unwrap();
    assert_eq!(reg.owner(id, None).unwrap(), None);
    let a = t.crew("A");
    reg.assign(id, a, None).unwrap();
    assert_eq!(reg.owner(id, None).unwrap(), Some(a));
    reg.unassign(id).unwrap();
    assert_eq!(reg.owner(id, None).unwrap(), None);
}

/// CR-008 §4: on a shared cuff, each user slot is its own crew member; other slots belong to
/// nobody, and a device without a slot does not accept readings that carry one.
#[test]
fn shared_devices_file_readings_only_under_the_assigned_slot() {
    let t = TempDb::new();
    let reg = Registry::new(&t.db);
    let id = reg.register(&cuff()).unwrap();
    let a = t.crew("A");
    reg.assign(id, a, Some(1)).unwrap();
    assert_eq!(reg.owner(id, Some(1)).unwrap(), Some(a));
    assert_eq!(reg.owner(id, Some(2)).unwrap(), None);
    assert_eq!(reg.owner(id, None).unwrap(), None);
    reg.assign(id, a, None).unwrap();
    assert_eq!(reg.owner(id, Some(1)).unwrap(), None);
    assert!(matches!(
        reg.assign(id, a, Some(0xFF)),
        Err(RegistryError::InvalidUserSlot(0xFF))
    ));
}

#[test]
fn unknown_devices_and_crew_members_are_errors() {
    let t = TempDb::new();
    let reg = Registry::new(&t.db);
    let ghost = DeviceId::from_uuid(uuid::Uuid::new_v4());
    assert!(matches!(
        reg.owner(ghost, None),
        Err(RegistryError::UnknownDevice(_))
    ));
    assert!(matches!(
        reg.assign(ghost, t.crew("A"), None),
        Err(RegistryError::UnknownDevice(_))
    ));
    let id = reg.register(&cuff()).unwrap();
    let nobody = CrewMemberId::from_uuid(uuid::Uuid::new_v4());
    assert!(matches!(
        reg.assign(id, nobody, None),
        Err(RegistryError::Store(_))
    ));
}

/// F-ACQ-10/11: link status, last data time and the measured clock offset are recorded.
#[test]
fn f_acq_10_status_last_seen_and_clock_offset_are_recorded() {
    let t = TempDb::new();
    let reg = Registry::new(&t.db);
    let id = reg.register(&cuff()).unwrap();
    let offset = ClockOffset {
        offset_ns: 1_250_000,
        residual_ns: 1_000,
        measured_at: TaiNs(9),
    };
    reg.record_status(id, DeviceStatus::Connected, Some(TaiNs(10)), Some(&offset))
        .unwrap();
    reg.record_status(id, DeviceStatus::Disconnected, None, None)
        .unwrap();
    let d = &reg.list().unwrap()[0];
    assert_eq!(d.status, "disconnected");
    assert_eq!(d.last_seen_tai, Some(10), "kept when nothing new arrived");
    assert_eq!(d.clock_offset_ms, Some(1.25));
}
