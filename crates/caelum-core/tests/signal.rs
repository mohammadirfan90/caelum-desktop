//! M7 integration through the station (F-SIG-01, F-SIG-02, F-ACQ-03, F-ALM-08; CR-013): ECG
//! waveforms are stored, QRS detection drives `asystole.critical`, ECG SQI and heart rate are
//! stored as derived observations with their algorithm version, and baselines are estimated.
//! Synthetic ECG generated here is a test fixture, never product data.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::sync::atomic::Ordering;

use caelum_core::security::role::Role;
use caelum_core::station::Station;
use caelum_devices::ghs::{EcgLead, Waveform, WaveformSignal, WaveformUnit};
use caelum_devices::registry::{DeviceIdentity, Registry};
use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::observation::Quality;
use caelum_domain::time::TaiNs;
use caelum_ingest::DeviceFlags;
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN: &str = "ground station passphrase";
const CREW_A: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a41";
const T0: i64 = 2_100_000_000_000_000_000;
const FS: f64 = 250.0;

fn crew() -> CrewMemberId {
    CrewMemberId::from_uuid(uuid::Uuid::parse_str(CREW_A).unwrap())
}

/// One second of synthetic ECG (mV): Gaussian R waves at `bpm` (none when `bpm` is 0) on a
/// low-amplitude, deterministic "physiological" background.
fn second(index: usize, bpm: f64) -> Vec<f64> {
    (0..FS as usize)
        .map(|i| {
            let t = index as f64 + i as f64 / FS;
            // A true pause is not a flat line: residual 0.02 mV activity in band (CR-013 §3).
            let background = 0.01 * (2.0 * std::f64::consts::PI * 0.3 * t).sin()
                + 0.02 * (2.0 * std::f64::consts::PI * 3.1 * t).sin();
            if bpm <= 0.0 {
                return background;
            }
            let period = 60.0 / bpm;
            let phase = t % period - period / 2.0;
            let r = 1.2 * (-(phase * phase) / (2.0 * 0.012 * 0.012)).exp();
            let tw = 0.25 * (-((phase - 0.25) * (phase - 0.25)) / (2.0 * 0.04 * 0.04)).exp();
            background + r + tw
        })
        .collect()
}

fn chunk(index: usize, bpm: f64) -> Waveform {
    Waveform {
        signal: WaveformSignal::Ecg(EcgLead::II),
        start: Some(TaiNs(T0 + index as i64 * 1_000_000_000)),
        sample_rate_hz: FS,
        unit: WaveformUnit::Millivolt,
        samples: second(index, bpm),
        quality: Quality::Good,
        flags: DeviceFlags::default(),
        source_code: None,
    }
}

fn setup() -> (DataDir, TestClock, Station, DeviceId) {
    let dir = DataDir::new();
    let keys = MemoryKeyStore::default();
    let clock = TestClock::default();
    clock.0.tai.store(T0, Ordering::SeqCst);
    let mut s = open(&dir, &keys, &clock).unwrap();
    s.create_first_administrator("admin", ADMIN).unwrap();
    s.login("admin", ADMIN.as_bytes()).unwrap();
    s.reauth(ADMIN.as_bytes()).unwrap();
    let fx = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/roster/");
    s.config_apply_crew(
        &std::fs::read(format!("{fx}crew-v1.toml")).unwrap(),
        &std::fs::read_to_string(format!("{fx}crew-v1.sig.toml")).unwrap(),
    )
    .unwrap();
    s.user_create(
        "cmo.b",
        Role::Cmo,
        Some(CrewMemberId::from_uuid(
            uuid::Uuid::parse_str("0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a42").unwrap(),
        )),
        "135792",
    )
    .unwrap();
    let device = Registry::new(s.db())
        .register(&DeviceIdentity {
            adapter: "ble-sig".into(),
            vendor: "V".into(),
            model: "ECG patch".into(),
            serial: "AA:BB:CC:00:11:22".into(),
            firmware: None,
            accuracy_class: None,
            calibration_due: None,
        })
        .unwrap();
    Registry::new(s.db()).assign(device, crew(), None).unwrap();
    (dir, clock, s, device)
}

fn alarm_rules(s: &Station) -> Vec<(String, String)> {
    let mut stmt = s
        .db()
        .connection()
        .prepare("SELECT rule_id, state FROM alarm")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn f_sig_01_ecg_gives_heart_rate_and_sqi_with_algorithm_versions() {
    let (_d, clock, mut s, device) = setup();
    for i in 0..20 {
        clock
            .0
            .tai
            .store(T0 + (i as i64 + 1) * 1_000_000_000, Ordering::SeqCst);
        s.ingest_waveforms(device, vec![(crew(), chunk(i, 60.0))])
            .unwrap();
        s.tick().unwrap();
    }
    let rows = s.db().observations(CREW_A, "heart_rate").unwrap();
    let last = rows.last().expect("derived heart rate");
    assert!((last.value.unwrap() - 60.0).abs() < 2.0, "{:?}", last.value);
    assert_eq!(last.classification, "derived");
    assert!(last
        .algorithm_version
        .as_deref()
        .is_some_and(|a| a.starts_with("hr-")));
    let sqi = s.db().observations(CREW_A, "sqi_ecg").unwrap();
    assert!(
        sqi.last().unwrap().value.unwrap() >= 0.7,
        "clean ECG has good quality"
    );
    let chunks = s.db().waveform_chunks(CREW_A, "ecg", T0).unwrap();
    assert_eq!(chunks.len(), 20);
    assert!(alarm_rules(&s)
        .iter()
        .all(|(r, _)| r != "asystole.critical"));
}

#[test]
fn f_sig_02_asystole_fires_on_a_pause_with_good_signal() {
    let (_d, clock, mut s, device) = setup();
    for i in 0..30 {
        let bpm = if i < 15 { 70.0 } else { 0.0 };
        clock
            .0
            .tai
            .store(T0 + (i as i64 + 1) * 1_000_000_000, Ordering::SeqCst);
        s.ingest_waveforms(device, vec![(crew(), chunk(i, bpm))])
            .unwrap();
        s.tick().unwrap();
    }
    let rules = alarm_rules(&s);
    assert!(
        rules
            .iter()
            .any(|(r, st)| r == "asystole.critical" && st == "active"),
        "{rules:?}"
    );
}

#[test]
fn f_sig_02b_asystole_fires_on_an_isoelectric_flatline() {
    let (_d, clock, mut s, device) = setup();
    for i in 0..30 {
        let samples = if i < 15 {
            second(i, 70.0)
        } else {
            vec![0.0; FS as usize] // Isoelectric flatline (AUDIT-001 / CR-013 §3)
        };
        let c = Waveform {
            signal: WaveformSignal::Ecg(EcgLead::II),
            start: Some(TaiNs(T0 + i as i64 * 1_000_000_000)),
            sample_rate_hz: FS,
            unit: WaveformUnit::Millivolt,
            samples,
            quality: Quality::Good,
            flags: DeviceFlags::default(),
            source_code: None,
        };
        clock
            .0
            .tai
            .store(T0 + (i as i64 + 1) * 1_000_000_000, Ordering::SeqCst);
        s.ingest_waveforms(device, vec![(crew(), c)])
            .unwrap();
        s.tick().unwrap();
    }
    let rules = alarm_rules(&s);
    assert!(
        rules
            .iter()
            .any(|(r, st)| r == "asystole.critical" && st == "active"),
        "{rules:?}"
    );
}


#[test]
fn sr_005_an_unusable_ecg_restarts_the_stream_and_raises_no_asystole() {
    let (_d, clock, mut s, device) = setup();
    for i in 0..30 {
        let mut c = chunk(i, if i < 10 { 70.0 } else { 0.0 });
        if i >= 10 {
            c.quality = Quality::Unusable;
        }
        clock
            .0
            .tai
            .store(T0 + (i as i64 + 1) * 1_000_000_000, Ordering::SeqCst);
        s.ingest_waveforms(device, vec![(crew(), c)]).unwrap();
        s.tick().unwrap();
    }
    assert!(
        alarm_rules(&s)
            .iter()
            .all(|(r, _)| r != "asystole.critical"),
        "lead off is not asystole"
    );
}

/// One GHS observation body (class, length, flags, rest), as on the air (IEEE 11073-10206).
fn ghs_body(class: u8, flags: u16, rest: &[u8]) -> Vec<u8> {
    let len = u16::try_from(5 + rest.len()).unwrap();
    let mut b = vec![class];
    b.extend_from_slice(&len.to_le_bytes());
    b.extend_from_slice(&flags.to_le_bytes());
    b.extend_from_slice(rest);
    b
}

/// M7 gate, F-ACQ-03, R-INT-03: a GHS ECG stream, as segmented notifications of the Live Health
/// Observations characteristic, is joined, decoded, stored and returned by the waveform read
/// command the Body screen draws from (`ui/tests/crew.test.tsx` renders such chunks).
#[test]
fn f_acq_03_a_ghs_ecg_stream_reaches_the_waveform_read_command() {
    let (_d, clock, mut s, device) = setup();
    // Lead II, µV, scale 1, offset 0, period 4 ms (250 Hz), 1 sample per period, 2 bytes each:
    // one second with a 1 mV R wave at sample 125.
    let mut rest = 0x0002_0102_u32.to_le_bytes().to_vec(); // MDC_ECG_ELEC_POTL_II
    rest.extend_from_slice(&0x10B3_u16.to_le_bytes()); // MDC_DIM_MICRO_VOLT
    rest.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // scale 1
    rest.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // offset 0
    rest.extend_from_slice(&[0x04, 0x00, 0x00, 0xFD]); // period 4 × 10^-3 s
    rest.extend_from_slice(&[1, 2]);
    rest.extend_from_slice(&250_u32.to_le_bytes());
    for i in 0..250_u16 {
        let uv: u16 = if i == 125 { 1000 } else { 0 };
        rest.extend_from_slice(&uv.to_le_bytes());
    }
    let body = ghs_body(4, 0x0001, &rest); // class 4: sample array
                                           // Three segments: first (bit 0), continuation, last (bit 1); counter in bits 2–7.
    let parts: Vec<&[u8]> = body.chunks(200).collect();
    assert_eq!(parts.len(), 3);
    for (k, part) in parts.iter().enumerate() {
        let first = u8::from(k == 0);
        let last = if k == parts.len() - 1 { 0x02 } else { 0 };
        let header = first | last | (u8::try_from(k).unwrap() << 2);
        let mut seg = vec![header];
        seg.extend_from_slice(part);
        clock.0.tai.store(T0 + 1_000_000_000, Ordering::SeqCst);
        s.ingest_notification(device, 0x2B8B, &seg).unwrap();
    }
    s.logout().unwrap();
    s.login("cmo.b", b"135792").unwrap();
    let chunks = s.waveform_recent(crew(), "ecg", 60).unwrap();
    assert_eq!(chunks.len(), 1, "{chunks:?}");
    let c = &chunks[0];
    assert_eq!((c.encoding.as_str(), c.sample_rate), ("i16le", 250.0));
    let samples: Vec<i16> = c
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b))
        .collect();
    assert_eq!(samples.len(), 250);
    assert_eq!(samples[125], 1000, "µV preserved from the device");
    assert_eq!(samples.iter().filter(|&&v| v != 0).count(), 1);
}
