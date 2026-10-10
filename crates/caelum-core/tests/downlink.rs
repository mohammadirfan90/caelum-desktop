//! M10 onboard ground downlink through the station (F-GND-01, F-GND-05, F-GND-06, F-GND-14;
//! CR-024): with a signed ground configuration that has a ground link and a signed contact
//! schedule, finished MEDICAL summary windows are sent as Space Packets to the ground endpoint
//! inside the contact window; outside it everything waits; without a contact schedule nothing is
//! sent blind.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::net::UdpSocket;
use std::sync::atomic::Ordering;
use std::time::Duration;

use caelum_core::station::Station;
use caelum_core::store::ObservationRecord;
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN: &str = "ground station passphrase";
const CREW_A: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a41";
const T0: i64 = 2_100_000_000_000_000_000;
const S: i64 = 1_000_000_000;

fn fx(dir: &str, name: &str) -> (Vec<u8>, String) {
    let d = format!("{}/tests/fixtures/{dir}/", env!("CARGO_MANIFEST_DIR"));
    (
        std::fs::read(format!("{d}{name}.toml")).unwrap(),
        std::fs::read_to_string(format!("{d}{name}.sig.toml")).unwrap(),
    )
}

fn hr(s: &Station, at: i64, v: f64) {
    s.db()
        .insert_observation(&ObservationRecord {
            id: uuid::Uuid::new_v4().to_string(),
            crew_member_id: CREW_A.into(),
            device_id: None,
            parameter: "heart_rate".into(),
            value: Some(v),
            value_text: None,
            unit: "/min".into(),
            measured_at_tai: at,
            received_at_tai: at,
            quality: "good".into(),
            classification: "measured".into(),
            measurement_mode: "continuous".into(),
            algorithm_version: None,
            context: None,
        })
        .unwrap();
}

#[test]
fn f_gnd_05_f_gnd_14_summaries_go_to_ground_only_inside_contact_windows() {
    let ground = UdpSocket::bind("127.0.0.1:47811").unwrap();
    ground
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
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
    let (b, sig) = fx("ground", "ground-dl-v1");
    s.config_apply_ground(&b, &sig).unwrap();
    assert!(s.downlink_status().configured);

    // No contact schedule: nothing is sent blind.
    for k in 0..130 {
        hr(&s, T0 + k * S, 70.0);
    }
    clock.0.tai.store(T0 + 125 * S, Ordering::SeqCst);
    s.tick().unwrap();
    let mut buf = [0_u8; 2048];
    assert!(
        ground.recv(&mut buf).is_err(),
        "no contact window: nothing leaves"
    );
    assert!(s.downlink_status().waiting > 0, "the summaries wait");

    // Inside the signed contact window, the waiting summaries go out as Space Packets.
    let (b, sig) = fx("ground", "contact-v1");
    s.config_apply_contact(&b, &sig).unwrap();
    clock.0.tai.store(T0 + 190 * S, Ordering::SeqCst);
    s.tick().unwrap();
    let n = ground
        .recv(&mut buf)
        .expect("a packet in the contact window");
    assert!(n > 6);
    // CCSDS primary header: version 0, telemetry (type bit 0), APID of medical_summary 0x310.
    assert_eq!(buf[0] >> 5, 0);
    assert_eq!((buf[0] >> 4) & 1, 0, "telemetry, never a telecommand");
    let apid = (u16::from(buf[0] & 0x07) << 8) | u16::from(buf[1]);
    assert_eq!(apid, 0x310);
}
