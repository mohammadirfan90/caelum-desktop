//! Clinical workflows through the station (CR-019; F-CLN-01, F-CLN-02, F-CLN-04, F-CLN-05,
//! F-CLN-06, F-CLN-07, F-CLN-08, SR-014; ROLE_MATRIX §2): scope per role, immutability after
//! sign-off, privacy tiers, signed protocols, the encrypted attachment store, audit entries.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::sync::atomic::Ordering;

use caelum_core::security::role::{DeniedReason, Role};
use caelum_core::security::session::AuthError;
use caelum_core::station::clinical::{CheckinAnswers, StepState};
use caelum_core::station::{Station, StationError};
use caelum_domain::ids::CrewMemberId;
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN: &str = "ground station passphrase";
const CREW_A: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a41";
const CREW_B: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a42";

fn crew(id: &str) -> CrewMemberId {
    CrewMemberId::from_uuid(uuid::Uuid::parse_str(id).unwrap())
}

fn fixture(dir: &str, name: &str) -> (Vec<u8>, String) {
    let d = format!("{}/tests/fixtures/{dir}/", env!("CARGO_MANIFEST_DIR"));
    (
        std::fs::read(format!("{d}{name}.toml")).unwrap(),
        std::fs::read_to_string(format!("{d}{name}.sig.toml")).unwrap(),
    )
}

struct World {
    _dir: DataDir,
    clock: TestClock,
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
    clock
        .0
        .tai
        .store(2_100_000_000_000_000_000, Ordering::SeqCst);
    let mut station = open(&dir, &keys, &clock).unwrap();
    station.create_first_administrator("admin", ADMIN).unwrap();
    station.login("admin", ADMIN.as_bytes()).unwrap();
    station.reauth(ADMIN.as_bytes()).unwrap();
    let (b, s) = fixture("roster", "crew-v1");
    station.config_apply_crew(&b, &s).unwrap();
    station
        .user_create("astro.a", Role::Astronaut, Some(crew(CREW_A)), "246813")
        .unwrap();
    station
        .user_create("cmo.b", Role::Cmo, Some(crew(CREW_B)), "135792")
        .unwrap();
    station
        .user_create("fs.one", Role::FlightSurgeon, None, ADMIN)
        .unwrap();
    station
        .user_create("bme.one", Role::Bme, None, ADMIN)
        .unwrap();
    World {
        _dir: dir,
        clock,
        station,
    }
}

fn denied(r: Result<impl std::fmt::Debug, StationError>) -> Option<DeniedReason> {
    match r {
        Err(StationError::Denied(d)) => Some(d.reason),
        _ => None,
    }
}

fn actions(w: &mut World) -> Vec<String> {
    w.as_user("cmo.b", "135792");
    w.station
        .audit_list()
        .unwrap()
        .into_iter()
        .map(|e| e.action.as_str().to_owned())
        .collect()
}

#[test]
fn f_cln_05_encounters_are_signed_once_and_immutable_afterwards() {
    let mut w = world();
    w.as_user("cmo.b", "135792");
    let soap = || {
        [
            Some("tired".to_owned()),
            Some("HR 72".to_owned()),
            None,
            Some("rest".to_owned()),
        ]
    };
    let id = w
        .station
        .encounter_save(None, crew(CREW_A), soap(), &[])
        .unwrap();
    // The astronaut sees no unsigned encounter.
    w.as_user("astro.a", "246813");
    assert!(w.station.encounter_list(crew(CREW_A)).unwrap().is_empty());
    assert_eq!(
        denied(w.station.encounter_save(None, crew(CREW_A), soap(), &[])),
        Some(DeniedReason::Role)
    );
    // Only the author edits; signing needs re-authentication.
    w.as_user("fs.one", ADMIN);
    assert!(matches!(
        w.station
            .encounter_save(Some(&id), crew(CREW_A), soap(), &[]),
        Err(StationError::Rejected(_))
    ));
    w.as_user("cmo.b", "135792");
    w.station
        .encounter_save(Some(&id), crew(CREW_A), soap(), &["att-1".to_owned()])
        .unwrap();
    w.clock.advance_s(121);
    assert!(matches!(
        w.station.encounter_sign(&id),
        Err(StationError::Auth(AuthError::ReauthRequired))
    ));
    w.station.reauth(b"135792").unwrap();
    w.station.encounter_sign(&id).unwrap();
    assert!(matches!(
        w.station.encounter_sign(&id),
        Err(StationError::Rejected(_))
    ));
    assert!(matches!(
        w.station
            .encounter_save(Some(&id), crew(CREW_A), soap(), &[]),
        Err(StationError::Rejected(_))
    ));
    w.as_user("astro.a", "246813");
    let mine = w.station.encounter_list(crew(CREW_A)).unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(
        mine[0].vitals.as_deref(),
        Some("[]"),
        "snapshot of what existed: nothing"
    );
    assert_eq!(
        denied(w.station.encounter_list(crew(CREW_B))),
        Some(DeniedReason::Role)
    );
    let a = actions(&mut w);
    assert!(a.contains(&"encounter.save".into()) && a.contains(&"encounter.sign".into()));
}

#[test]
fn f_cln_07_check_ins_respect_privacy_tiers() {
    let mut w = world();
    w.as_user("astro.a", "246813");
    let answers = CheckinAnswers {
        mood: 4,
        symptoms: vec!["headache".into()],
        sleep_hours: Some(6.5),
    };
    w.station.checkin_submit(&answers, "private").unwrap();
    w.station.checkin_submit(&answers, "medical").unwrap();
    assert!(matches!(
        w.station.checkin_submit(&answers, "public"),
        Err(StationError::Rejected(_))
    ));
    assert!(matches!(
        w.station.checkin_submit(
            &CheckinAnswers {
                mood: 9,
                ..answers.clone()
            },
            "private"
        ),
        Err(StationError::Rejected(_))
    ));
    assert_eq!(w.station.checkin_list(crew(CREW_A)).unwrap().len(), 2);
    w.as_user("cmo.b", "135792");
    let seen = w.station.checkin_list(crew(CREW_A)).unwrap();
    assert_eq!(
        seen.iter()
            .map(|c| c.privacy_tier.as_str())
            .collect::<Vec<_>>(),
        ["medical"]
    );
    assert_eq!(
        denied(w.station.checkin_submit(&answers, "medical")),
        Some(DeniedReason::Role)
    );
    w.as_user("bme.one", ADMIN);
    assert_eq!(
        denied(w.station.checkin_list(crew(CREW_A))),
        Some(DeniedReason::Role)
    );
    // The audit never holds the content or the tier.
    w.as_user("cmo.b", "135792");
    let entries = w.station.audit_list().unwrap();
    let c = entries
        .iter()
        .find(|e| e.action.as_str() == "checkin.submit")
        .unwrap();
    assert_eq!(c.details.to_string(), "{}");
}

#[test]
fn f_cln_06_pvt_stores_the_median_or_nothing() {
    let mut w = world();
    w.as_user("astro.a", "246813");
    let times = [250, 260, 270, 280, 290, 300, 310, 320, 600, 700];
    let r = w.station.pvt_submit(&times).unwrap();
    assert_eq!((r.median_ms, r.lapses, r.responses), (295.0, 2, 10));
    assert!(matches!(
        w.station.pvt_submit(&times[..5]),
        Err(StationError::Rejected(_))
    ));
    assert!(matches!(
        w.station.pvt_submit(&[50; 10]),
        Err(StationError::Rejected(_))
    ));
    let v = w.station.vitals_latest(crew(CREW_A)).unwrap();
    let pvt = v
        .iter()
        .find(|x| x.observation.parameter == "pvt_median_rt")
        .unwrap();
    assert_eq!(pvt.observation.value, Some(295.0));
    assert_eq!(pvt.observation.classification, "measured");
}

/// F-CLN-02, R-CLN-04: protocols are versioned and signed; an unsigned or altered library is
/// refused.
#[test]
fn f_cln_02_protocols_are_signed_public_and_runs_are_audited_and_linked() {
    let mut w = world();
    assert!(
        w.station.protocol_list().unwrap().is_empty(),
        "no protocol is shipped"
    );
    // Administrator applies the signed library (re-authenticated at sign-in).
    w.as_user("admin", ADMIN);
    let (b, s) = fixture("protocols", "library-v1");
    let mut tampered = b.clone();
    tampered.extend(b"\n");
    assert!(matches!(
        w.station.config_apply_protocols(&tampered, &s),
        Err(StationError::Rejected(_))
    ));
    assert_eq!(w.station.config_apply_protocols(&b, &s).unwrap(), 1);
    // SR-014: readable with nobody signed in.
    w.station.logout().unwrap();
    assert_eq!(w.station.protocol_list().unwrap()[0].steps.len(), 3);
    // A run, step by step, completed only when no step is open.
    w.as_user("astro.a", "246813");
    let run = w
        .station
        .checklist_start("fixture.desaturation", Some(crew(CREW_A)), None)
        .unwrap();
    assert_eq!(
        denied(
            w.station
                .checklist_start("fixture.desaturation", Some(crew(CREW_B)), None)
        ),
        Some(DeniedReason::Role)
    );
    w.station
        .checklist_update(&run, 0, StepState::Done, false)
        .unwrap();
    assert!(matches!(
        w.station.checklist_update(&run, 1, StepState::Done, true),
        Err(StationError::Rejected(_))
    ));
    w.station
        .checklist_update(&run, 2, StepState::Skipped, false)
        .unwrap();
    w.station
        .checklist_update(&run, 1, StepState::Done, true)
        .unwrap();
    assert!(matches!(
        w.station.checklist_update(&run, 1, StepState::Done, false),
        Err(StationError::Rejected(_))
    ));
    assert!(matches!(
        w.station.checklist_start("nope", None, None),
        Err(StationError::Rejected(_))
    ));
    let runs = w.station.checklist_list().unwrap();
    assert!(runs[0].completed_at_tai.is_some());
    let a = actions(&mut w);
    for x in [
        "checklist.start",
        "checklist.step",
        "checklist.complete",
        "config.apply",
    ] {
        assert!(a.contains(&x.to_owned()), "{x}");
    }
}

#[test]
fn f_cln_08_medication_log_scoped_and_validated() {
    let mut w = world();
    w.as_user("cmo.b", "135792");
    w.station
        .medication_add(
            Some(crew(CREW_A)),
            "dose",
            "Item X",
            Some(200.0),
            Some("mg".into()),
            None,
        )
        .unwrap();
    w.station
        .medication_add(
            None,
            "kit_restock",
            "Item Y",
            Some(10.0),
            Some("{tbl}".into()),
            None,
        )
        .unwrap();
    assert!(matches!(
        w.station
            .medication_add(None, "dose", "Item X", None, None, None),
        Err(StationError::Rejected(_))
    ));
    assert!(matches!(
        w.station
            .medication_add(Some(crew(CREW_A)), "dose", "Item X", Some(1.0), None, None),
        Err(StationError::Rejected(_))
    ));
    w.as_user("astro.a", "246813");
    assert_eq!(
        w.station.medication_log().unwrap().len(),
        1,
        "own doses only"
    );
    assert_eq!(
        denied(
            w.station
                .medication_add(None, "kit_use", "Item Y", None, None, None)
        ),
        Some(DeniedReason::Role)
    );
    w.as_user("fs.one", ADMIN);
    assert_eq!(w.station.medication_log().unwrap().len(), 2);
}

#[test]
fn f_cln_04_attachments_are_encrypted_and_checked() {
    let mut w = world();
    w.as_user("cmo.b", "135792");
    let id = w
        .station
        .attachment_import(
            crew(CREW_A),
            "file",
            "note.txt",
            b"ultrasound report",
            serde_json::json!({}),
        )
        .unwrap();
    let (rec, bytes) = w.station.attachment_get(&id).unwrap();
    assert_eq!(bytes, b"ultrasound report");
    assert_eq!(rec.filename.as_deref(), Some("note.txt"));
    assert_eq!(
        w.station.attachment_list(Some(crew(CREW_A))).unwrap().len(),
        1
    );
    assert!(matches!(
        w.station
            .attachment_import(crew(CREW_A), "exe", "x", b"x", serde_json::json!({})),
        Err(StationError::Rejected(_))
    ));
    // The file on disk is ciphertext; tampering is detected.
    let path = w._dir.0.join("attachments").join(format!("{id}.bin"));
    let raw = std::fs::read(&path).unwrap();
    assert!(!raw.windows(10).any(|x| x == b"ultrasound"));
    let mut bad = raw.clone();
    let n = bad.len() - 1;
    bad[n] ^= 0xFF;
    std::fs::write(&path, bad).unwrap();
    assert!(matches!(
        w.station.attachment_get(&id),
        Err(StationError::Rejected(_))
    ));
    w.as_user("astro.a", "246813");
    assert_eq!(
        denied(w.station.attachment_list(None)),
        Some(DeniedReason::Role)
    );
}

#[test]
fn f_cln_01_crew_overview_shows_every_active_member() {
    let mut w = world();
    w.as_user("astro.a", "246813");
    w.station.pvt_submit(&[300; 10]).unwrap();
    assert_eq!(denied(w.station.crew_overview()), Some(DeniedReason::Role));
    w.as_user("fs.one", ADMIN);
    let o = w.station.crew_overview().unwrap();
    assert_eq!(o.len(), 2);
    assert!(
        o.iter()
            .all(|c| c.top_priority.is_none() && c.vitals.is_empty()),
        "PVT is not a key vital"
    );
}
