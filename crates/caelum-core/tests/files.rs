//! M8 integration through the station (F-ACQ-06, F-ACQ-07, F-ACQ-08, F-CLN-03, F-CLN-04;
//! CR-014, SR-016, SR-017): a clinical file is kept only if it opens, ECG recordings come back
//! decoded for review, FHIR observations are stored as `imported` and evaluated, and data
//! flagged as test data is refused in release builds. Files are built here from known values (test fixtures).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::sync::atomic::Ordering;

use caelum_core::devices::files::edf::{
    self, EdfPlusPatient, EdfPlusRecordingId, EdfPlusWrite, EdfWriteSignal,
};
use caelum_core::devices::files::CivilDateTime;
use caelum_core::security::role::{DeniedReason, Role};
use caelum_core::station::{Station, StationError};
use caelum_domain::ids::CrewMemberId;
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN: &str = "ground station passphrase";
const CREW_A: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a41";
const CREW_B: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a42";

fn crew(id: &str) -> CrewMemberId {
    CrewMemberId::from_uuid(uuid::Uuid::parse_str(id).unwrap())
}

struct World {
    _dir: DataDir,
    station: Station,
}

impl World {
    fn as_user(&mut self, name: &str, secret: &str) {
        let _ = self.station.logout();
        self.station.login(name, secret.as_bytes()).unwrap();
    }
}

fn world() -> World {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    // 2024-07-18T13:19:23Z.
    clock
        .0
        .tai
        .store(2_100_000_000_000_000_000, Ordering::SeqCst);
    let mut station = open(&dir, &keys, &clock).unwrap();
    station.create_first_administrator("admin", ADMIN).unwrap();
    station.login("admin", ADMIN.as_bytes()).unwrap();
    station.reauth(ADMIN.as_bytes()).unwrap();
    let d = format!("{}/tests/fixtures/roster/", env!("CARGO_MANIFEST_DIR"));
    let body = std::fs::read(format!("{d}crew-v1.toml")).unwrap();
    let sig = std::fs::read_to_string(format!("{d}crew-v1.sig.toml")).unwrap();
    station.config_apply_crew(&body, &sig).unwrap();
    station
        .user_create("astro.a", Role::Astronaut, Some(crew(CREW_A)), "246813")
        .unwrap();
    station
        .user_create("cmo.b", Role::Cmo, Some(crew(CREW_B)), "135792")
        .unwrap();
    World { _dir: dir, station }
}

/// A 2 s, 250 Hz EDF+ ECG: 1 mV R waves at 60 bpm.
fn edf_ecg() -> Vec<u8> {
    let samples: Vec<f64> = (0..500)
        .map(|i| if i % 250 == 125 { 1.0 } else { 0.0 })
        .collect();
    edf::write_edf_plus(&EdfPlusWrite {
        patient: EdfPlusPatient {
            code: None,
            sex: None,
            birthdate: None,
            name: None,
        },
        recording: EdfPlusRecordingId::default(),
        start: CivilDateTime {
            year: 2024,
            month: 7,
            day: 18,
            hour: 13,
            minute: 0,
            second: 0,
            nanosecond: 0,
            utc_offset_minutes: None,
        },
        record_duration_s: 1.0,
        signals: vec![EdfWriteSignal::fitted("ECG II", "mV", samples, 250).unwrap()],
        annotations: vec![],
    })
    .unwrap()
}

fn fhir_hr(value: u32, extra: &str) -> String {
    format!(
        r#"{{"resourceType":"Observation","id":"o1","status":"final",{extra}
            "code":{{"coding":[{{"system":"http://loinc.org","code":"8867-4"}}]}},
            "effectiveDateTime":"2024-07-18T13:19:00Z",
            "valueQuantity":{{"value":{value},"unit":"beats/minute","system":"http://unitsofmeasure.org","code":"/min"}}}}"#
    )
}

#[test]
fn f_acq_07_f_cln_03_an_ecg_file_is_kept_only_if_it_opens_and_is_reviewed_decoded() {
    let mut w = world();
    w.as_user("cmo.b", "135792");
    let id = w
        .station
        .file_import(crew(CREW_A), "ecg", "rest.edf", &edf_ecg())
        .unwrap();
    let rec = w
        .station
        .attachment_list(Some(crew(CREW_A)))
        .unwrap()
        .into_iter()
        .find(|a| a.id == id)
        .unwrap();
    let meta: serde_json::Value = serde_json::from_str(&rec.metadata).unwrap();
    assert_eq!(meta["format"], "edf");
    assert_eq!(meta["leads"][0], "ECG II");
    assert_eq!(meta["sample_rate_hz"], 250.0);
    assert_eq!(meta["duration_s"], 2.0);

    let view = w.station.ecg_view(&id).unwrap();
    assert_eq!(view.format, "edf");
    assert_eq!(view.leads[0].samples.len(), 500);
    let peak = view.leads[0]
        .samples
        .iter()
        .copied()
        .fold(f64::MIN, f64::max);
    assert!((peak - 1.0).abs() < 1e-3, "{peak}");

    // A file that does not open is not kept.
    let before = w.station.attachment_list(None).unwrap().len();
    for (kind, bytes) in [
        ("ecg", &b"garbage that is no ECG"[..]),
        ("dicom", &b"not dicom"[..]),
    ] {
        assert!(matches!(
            w.station.file_import(crew(CREW_A), kind, "x", bytes),
            Err(StationError::Rejected(_))
        ));
    }
    assert_eq!(w.station.attachment_list(None).unwrap().len(), before);

    // A plain file is not shown as an ECG or an image.
    let note = w
        .station
        .file_import(crew(CREW_A), "file", "note.txt", b"report")
        .unwrap();
    assert!(matches!(
        w.station.ecg_view(&note),
        Err(StationError::Rejected(_))
    ));
    assert!(matches!(
        w.station.dicom_view(&note, 0, None),
        Err(StationError::Rejected(_))
    ));

    w.as_user("astro.a", "246813");
    assert!(matches!(
        w.station.ecg_view(&id),
        Err(StationError::Denied(d)) if d.reason == DeniedReason::Role
    ));
}

#[test]
fn f_acq_08_sr_016_fhir_observations_are_stored_as_imported_and_evaluated() {
    let mut w = world();
    w.as_user("cmo.b", "135792");
    let (n, rejected, _out) = w
        .station
        .fhir_import(crew(CREW_A), "obs.json", fhir_hr(64, "").as_bytes())
        .unwrap();
    assert_eq!((n, rejected.len()), (1, 0), "{rejected:?}");
    let hr = w
        .station
        .vitals_latest(crew(CREW_A))
        .unwrap()
        .into_iter()
        .find(|v| v.observation.parameter == "heart_rate")
        .unwrap();
    assert_eq!(hr.observation.value, Some(64.0));
    assert_eq!(hr.observation.classification, "imported");
    assert_eq!(hr.observation.quality, "degraded");

    // Not JSON: refused as a whole.
    assert!(matches!(
        w.station
            .fhir_import(crew(CREW_A), "obs.json", b"{not json"),
        Err(StationError::Rejected(_))
    ));

    // Off the roster: refused.
    let stranger = crew("0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a4f");
    assert!(matches!(
        w.station
            .fhir_import(stranger, "obs.json", fhir_hr(64, "").as_bytes()),
        Err(StationError::Rejected(_))
    ));

    // Astronauts cannot import.
    w.as_user("astro.a", "246813");
    assert!(matches!(
        w.station.fhir_import(crew(CREW_A), "obs.json", fhir_hr(64, "").as_bytes()),
        Err(StationError::Denied(d)) if d.reason == DeniedReason::Role
    ));
}

#[test]
fn sr_017_fhir_observations_marked_as_test_data_are_refused() {
    let mut w = world();
    w.as_user("cmo.b", "135792");
    let tagged = fhir_hr(
        200,
        r#""meta":{"security":[{"system":"http://terminology.hl7.org/CodeSystem/v3-ActReason","code":"HTEST"}]},"#,
    );
    let (n, rejected, out) = w
        .station
        .fhir_import(crew(CREW_A), "obs.json", tagged.as_bytes())
        .unwrap();
    if cfg!(feature = "test-equipment") {
        // Test-equipment builds keep it, classified as simulated (never exported).
        assert_eq!(n, 1);
        let hr = w
            .station
            .vitals_latest(crew(CREW_A))
            .unwrap()
            .into_iter()
            .find(|v| v.observation.parameter == "heart_rate")
            .unwrap();
        assert_eq!(hr.observation.classification, "simulated");
    } else {
        assert_eq!(n, 0);
        assert_eq!(rejected.len(), 1, "{rejected:?}");
        assert!(out.events.is_empty());
    }
}
