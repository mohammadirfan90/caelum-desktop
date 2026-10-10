//! M2 gate: devices stream through decode and ingest into the database; missing fields produce no
//! observation; demonstration/test readings are rejected; no thermometer reading reaches
//! core_temp (F-ACQ-02, F-ACQ-09, F-ACQ-10, R-CLN-02, SR-017).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use std::path::PathBuf;

use caelum_core::acquisition::{Acquisition, TEST_EQUIPMENT_ADAPTER};
use caelum_devices::registry::{DeviceIdentity, Registry};
use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::time::TaiNs;
use caelum_ingest::Rejection;
use caelum_store::{Database, DbKey};
use caelum_test_equipment::sig::{self as emu, uuid as gatt};

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
        let dir = std::env::temp_dir().join(format!("caelum-acq-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("caelum.db"), &DbKey::from_bytes([3; 32])).unwrap();
        Self {
            db,
            _guard: Guard(dir),
        }
    }

    fn crew(&self) -> CrewMemberId {
        let id = CrewMemberId::from_uuid(uuid::Uuid::new_v4());
        self.db
            .connection()
            .execute(
                "INSERT INTO crew_member (id, name, role, mission, active) VALUES (?1, 'A', 'crew', 'M-1', 1)",
                [id.to_string()],
            )
            .unwrap();
        id
    }

    fn device(
        &self,
        adapter: &str,
        serial: &str,
        crew: Option<CrewMemberId>,
        slot: Option<u8>,
    ) -> DeviceId {
        let reg = Registry::new(&self.db);
        let id = reg
            .register(&DeviceIdentity {
                adapter: adapter.into(),
                vendor: "Vendor".into(),
                model: "Model".into(),
                serial: serial.into(),
                firmware: None,
                accuracy_class: None,
                calibration_due: None,
            })
            .unwrap();
        if let Some(c) = crew {
            reg.assign(id, c, slot).unwrap();
        }
        id
    }

    fn count(&self, parameter: &str) -> i64 {
        self.db
            .connection()
            .query_row(
                "SELECT count(*) FROM observation WHERE parameter = ?1",
                [parameter],
                |r| r.get(0),
            )
            .unwrap()
    }
}

const T: TaiNs = TaiNs(2_100_000_000_000_000_000);

/// The six profiles a crew member's devices send in one session.
fn session() -> Vec<(u16, Vec<u8>, &'static str)> {
    vec![
        (
            gatt::HEART_RATE_MEASUREMENT,
            emu::heart_rate(64, Some(true), &[960]),
            "heart_rate",
        ),
        (
            gatt::PLX_CONTINUOUS,
            emu::plx_continuous(emu::sfloat(97, 0), emu::sfloat(64, 0), 0),
            "spo2",
        ),
        (
            gatt::BLOOD_PRESSURE_MEASUREMENT,
            emu::blood_pressure(118, 76, 90, 64, 1),
            "bp_systolic",
        ),
        (
            gatt::TEMPERATURE_MEASUREMENT,
            emu::temperature(emu::float(368, -1), Some(6)),
            "body_temp_measured",
        ),
        (gatt::WEIGHT_MEASUREMENT, emu::weight(13_800, 1), "weight"),
        (
            gatt::GLUCOSE_MEASUREMENT,
            emu::glucose(1, emu::sfloat(92, -5), 0x11),
            "glucose",
        ),
    ]
}

/// A real (non-emulated) device path: every profile streams into the database as measured data.
#[test]
fn f_acq_02_device_profiles_stream_through_ingest_into_the_database() {
    let t = TempDb::new();
    let crew = t.crew();
    let mut acq = Acquisition::new().unwrap();
    for (i, (characteristic, payload, parameter)) in session().into_iter().enumerate() {
        let slot = matches!(
            characteristic,
            gatt::BLOOD_PRESSURE_MEASUREMENT | gatt::WEIGHT_MEASUREMENT
        )
        .then_some(1);
        let device = t.device("ble-sig", &format!("SN-{i}"), Some(crew), slot);
        let out = acq
            .notification(&t.db, device, characteristic, &payload, T)
            .unwrap();
        assert!(
            out.rejected.is_empty() && out.malformed.is_none(),
            "{parameter}: {out:?}"
        );
        assert!(t.count(parameter) >= 1, "{parameter} stored");
    }
    let rows =
        t.db.observations(&crew.to_string(), "body_temp_measured")
            .unwrap();
    assert_eq!(rows[0].classification, "measured");
    assert!(rows[0]
        .context
        .as_deref()
        .unwrap()
        .contains("\"site\":\"mouth\""));
    assert_eq!(
        t.count("core_temp"),
        0,
        "no thermometer reading reaches core_temp"
    );
    assert_eq!(t.count("rr_interval"), 1);
    assert_eq!(t.count("pulse_rate"), 2, "oximeter and cuff");
}

/// SR-017, docs/05 §3: emulated devices are test data. Release builds reject every reading;
/// test-equipment builds store them as simulated only.
#[test]
fn emulated_devices_are_test_data() {
    let t = TempDb::new();
    let crew = t.crew();
    let mut acq = Acquisition::new().unwrap();
    let device = t.device(TEST_EQUIPMENT_ADAPTER, "EMU-1", Some(crew), None);
    let payload = emu::heart_rate(64, Some(true), &[]);
    let out = acq
        .notification(&t.db, device, gatt::HEART_RATE_MEASUREMENT, &payload, T)
        .unwrap();
    if cfg!(feature = "test-equipment") {
        assert_eq!(out.stored.len(), 1);
        let rows = t.db.observations(&crew.to_string(), "heart_rate").unwrap();
        assert_eq!(rows[0].classification, "simulated");
    } else {
        assert_eq!(out.rejected, [Rejection::TestData]);
        assert_eq!(t.count("heart_rate"), 0);
    }
}

/// SR-017: an oximeter flagging demonstration data stores nothing; the drop is counted.
#[test]
fn sr_017_demonstration_readings_store_nothing_and_are_counted() {
    let t = TempDb::new();
    let crew = t.crew();
    let mut acq = Acquisition::new().unwrap();
    let device = t.device("ble-sig", "PLX-1", Some(crew), None);
    let payload = emu::plx_spot_check(emu::sfloat(97, 0), emu::sfloat(64, 0), 1 << 10);
    let out = acq
        .notification(&t.db, device, gatt::PLX_SPOT_CHECK, &payload, T)
        .unwrap();
    if cfg!(not(feature = "test-equipment")) {
        assert_eq!(out.stored.len(), 0);
        assert_eq!(acq.dropped(device), [("demonstration_data", 2)]);
        assert_eq!(t.count("spo2"), 0);
    }
}

/// R-CLN-02: missing values, unassigned devices, wrong user slots and malformed payloads store
/// nothing and are counted per device (F-ACQ-10).
#[test]
fn r_cln_02_missing_or_unowned_readings_store_nothing() {
    let t = TempDb::new();
    let crew = t.crew();
    let mut acq = Acquisition::new().unwrap();

    let ox = t.device("ble-sig", "PLX-2", Some(crew), None);
    let nan = emu::plx_continuous(emu::SFLOAT_NAN, emu::SFLOAT_NAN, 0);
    let out = acq
        .notification(&t.db, ox, gatt::PLX_CONTINUOUS, &nan, T)
        .unwrap();
    assert_eq!(
        out.rejected,
        [Rejection::MissingValue, Rejection::MissingValue]
    );

    let loose = t.device("ble-sig", "HR-9", None, None);
    let hr = emu::heart_rate(70, Some(true), &[]);
    let out = acq
        .notification(&t.db, loose, gatt::HEART_RATE_MEASUREMENT, &hr, T)
        .unwrap();
    assert_eq!(out.rejected, [Rejection::Unassigned]);

    let cuff = t.device("ble-sig", "BP-9", Some(crew), Some(1));
    let other_user = emu::blood_pressure(118, 76, 90, 64, 2);
    let out = acq
        .notification(
            &t.db,
            cuff,
            gatt::BLOOD_PRESSURE_MEASUREMENT,
            &other_user,
            T,
        )
        .unwrap();
    assert!(out.rejected.iter().all(|r| *r == Rejection::Unassigned));

    let out = acq
        .notification(&t.db, ox, gatt::PLX_CONTINUOUS, &[0x04, 97], T)
        .unwrap();
    assert!(out.malformed.is_some());
    assert_eq!(acq.dropped(ox), [("malformed", 1), ("missing_value", 2)]);

    let total: i64 =
        t.db.connection()
            .query_row("SELECT count(*) FROM observation", [], |r| r.get(0))
            .unwrap();
    assert_eq!(total, 0);
    let d = t.db.device(&ox.to_string()).unwrap().unwrap();
    assert_eq!(
        (d.status.as_str(), d.last_seen_tai),
        ("connected", Some(T.0))
    );
}
