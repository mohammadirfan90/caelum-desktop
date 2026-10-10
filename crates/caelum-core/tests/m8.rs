//! M8 remaining items through the station (F-ACQ-06, F-CLN-03, F-CLN-06, F-CLN-07; CR-022): the
//! DICOM folder watch files only what opens and moves every file aside, never deleting; an ECG
//! segment exports as EDF+ that reopens with the shifted start and the same samples; the next
//! check follows the signed schedule. Files are built here from known values (test fixtures).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::sync::atomic::Ordering;

use caelum_core::devices::files::edf::{
    self, EdfPlusPatient, EdfPlusRecordingId, EdfPlusWrite, EdfWriteSignal,
};
use caelum_core::devices::files::CivilDateTime;
use caelum_core::security::role::Role;
use caelum_core::station::Station;
use caelum_domain::ids::CrewMemberId;
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN: &str = "ground station passphrase";
const CREW_A: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a41";
const T0: i64 = 2_100_000_000_000_000_000;

fn crew() -> CrewMemberId {
    CrewMemberId::from_uuid(uuid::Uuid::parse_str(CREW_A).unwrap())
}

fn fx(dir: &str, name: &str) -> (Vec<u8>, String) {
    let d = format!("{}/tests/fixtures/{dir}/", env!("CARGO_MANIFEST_DIR"));
    (
        std::fs::read(format!("{d}{name}.toml")).unwrap(),
        std::fs::read_to_string(format!("{d}{name}.sig.toml")).unwrap(),
    )
}

fn station() -> (DataDir, Station) {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    clock.0.tai.store(T0, Ordering::SeqCst);
    let mut s = open(&dir, &keys, &clock).unwrap();
    s.create_first_administrator("admin", ADMIN).unwrap();
    s.login("admin", ADMIN.as_bytes()).unwrap();
    s.reauth(ADMIN.as_bytes()).unwrap();
    let (b, sig) = fx("roster", "crew-v1");
    s.config_apply_crew(&b, &sig).unwrap();
    s.user_create(
        "cmo.b",
        Role::Cmo,
        Some(CrewMemberId::from_uuid(
            uuid::Uuid::parse_str("0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a42").unwrap(),
        )),
        "135792",
    )
    .unwrap();
    (dir, s)
}

/// A 2×2, 8-bit monochrome Secondary Capture image built with dicom-rs.
fn dicom_image() -> Vec<u8> {
    use dicom_core::{DataElement, PrimitiveValue, VR};
    use dicom_dictionary_std::tags;
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
    let sop_class = "1.2.840.10008.5.1.4.1.1.7";
    let sop_instance = "1.2.826.0.1.3680043.8.498.10";
    let e = |t, vr, v: PrimitiveValue| DataElement::new(t, vr, v);
    let obj = InMemDicomObject::from_element_iter([
        e(tags::SOP_CLASS_UID, VR::UI, sop_class.into()),
        e(tags::SOP_INSTANCE_UID, VR::UI, sop_instance.into()),
        e(
            tags::STUDY_INSTANCE_UID,
            VR::UI,
            "1.2.826.0.1.3680043.8.498.11".into(),
        ),
        e(
            tags::SERIES_INSTANCE_UID,
            VR::UI,
            "1.2.826.0.1.3680043.8.498.12".into(),
        ),
        e(tags::MODALITY, VR::CS, "US".into()),
        e(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        e(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            "MONOCHROME2".into(),
        ),
        e(tags::ROWS, VR::US, PrimitiveValue::from(2_u16)),
        e(tags::COLUMNS, VR::US, PrimitiveValue::from(2_u16)),
        e(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(8_u16)),
        e(tags::BITS_STORED, VR::US, PrimitiveValue::from(8_u16)),
        e(tags::HIGH_BIT, VR::US, PrimitiveValue::from(7_u16)),
        e(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        e(
            tags::PIXEL_DATA,
            VR::OB,
            PrimitiveValue::from(vec![0_u8, 85, 170, 255]),
        ),
    ]);
    let file = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax("1.2.840.10008.1.2.1")
                .media_storage_sop_class_uid(sop_class)
                .media_storage_sop_instance_uid(sop_instance),
        )
        .unwrap();
    let mut out = Vec::new();
    file.write_all(&mut out).unwrap();
    out
}

#[test]
fn f_acq_06_the_folder_watch_files_what_opens_and_moves_everything_aside() {
    let (dir, mut s) = station();
    let inbox = dir.0.join("dicom-inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::write(inbox.join("good.dcm"), dicom_image()).unwrap();
    std::fs::write(inbox.join("bad.dcm"), b"not a dicom file").unwrap();
    std::fs::write(inbox.join("notes.txt"), b"ignored").unwrap();

    // Without the signed configuration nothing is touched.
    assert!(s.import_folder_scan().unwrap().imported.is_empty());
    assert!(inbox.join("good.dcm").exists());

    let (b, sig) = fx("m8", "import-v1");
    s.config_apply_import(&b, &sig).unwrap();
    let scan = s.import_folder_scan().unwrap();
    assert_eq!(scan.imported, vec!["good.dcm".to_owned()]);
    assert_eq!(scan.refused.len(), 1);
    assert!(inbox.join("imported").join("good.dcm").exists());
    assert!(inbox.join("refused").join("bad.dcm").exists());
    assert!(
        inbox.join("notes.txt").exists(),
        "other files are left alone"
    );
    s.logout().unwrap();
    s.login("cmo.b", b"135792").unwrap();
    let files = s.attachment_list(Some(crew())).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].kind, "dicom");
    let image = s.dicom_view(&files[0].id, 0, None).unwrap();
    assert_eq!((image.rows, image.columns), (2, 2));
}

fn edf_ecg() -> Vec<u8> {
    let samples: Vec<f64> = (0..1_000)
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
        start: CivilDateTime::new(2024, 7, 18, 13, 0, 0, 0, None).unwrap(),
        record_duration_s: 1.0,
        signals: vec![EdfWriteSignal::fitted("ECG II", "mV", samples, 250).unwrap()],
        annotations: vec![edf::EdfAnnotation {
            onset_s: 2.0,
            duration_s: None,
            text: "chest pain".into(),
        }],
    })
    .unwrap()
}

#[test]
fn f_cln_03_events_are_shown_and_a_segment_exports_as_edf_plus_that_reopens() {
    let (_d, mut s) = station();
    s.logout().unwrap();
    s.login("cmo.b", b"135792").unwrap();
    let id = s
        .file_import(crew(), "ecg", "rest.edf", &edf_ecg())
        .unwrap();
    let rec = s.ecg_view(&id).unwrap();
    assert!(rec
        .events
        .iter()
        .any(|e| e.text == "chest pain" && (e.onset_s - 2.0).abs() < 1e-9));
    assert!(rec.start.is_some());

    let bytes = s.ecg_export_segment(&id, 1.0, 3.0).unwrap();
    let f = edf::read(&bytes).unwrap();
    assert_eq!(
        (f.start.hour, f.start.minute, f.start.second),
        (13, 0, 1),
        "start moved by the offset"
    );
    let lead = &f.signals[0];
    assert_eq!(
        (lead.label.as_str(), lead.physical_dimension.as_str()),
        ("ECG II", "mV")
    );
    assert_eq!(lead.samples.len(), 500);
    let peaks = lead.samples.iter().filter(|v| **v > 0.9).count();
    assert_eq!(peaks, 2, "the R waves of seconds 1 and 2");
    // Beyond the recording, or longer than 10 min: refused.
    assert!(s.ecg_export_segment(&id, 3.0, 9.0).is_err());
    assert!(s.ecg_export_segment(&id, 2.0, 1.0).is_err());
}

#[test]
fn f_cln_06_f_cln_07_the_next_check_follows_the_signed_schedule() {
    let (_d, mut s) = station();
    let (b, sig) = fx("m8", "schedule-v1");
    s.logout().unwrap();
    s.login("cmo.b", b"135792").unwrap();
    assert!(s.schedule_next().unwrap().is_none(), "no schedule: NO DATA");
    s.logout().unwrap();
    s.login("admin", ADMIN.as_bytes()).unwrap();
    s.reauth(ADMIN.as_bytes()).unwrap();
    s.config_apply_schedule(&b, &sig).unwrap();
    s.logout().unwrap();
    s.login("cmo.b", b"135792").unwrap();
    let next = s.schedule_next().unwrap().unwrap();
    assert!(next.at.0 > T0);
    assert!(["pvt", "checkin"].contains(&next.kind));
}
