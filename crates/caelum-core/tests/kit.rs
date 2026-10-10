//! M7 integration of the sensor kit and the remaining signal algorithms (F-ACQ-05, F-SIG-03,
//! F-SIG-04, F-SIG-05, F-SIG-07; CR-013, CR-021). Frames are built here byte by byte per
//! ICD-SENSOR-KIT v1 from known signals (test fixtures, never product data), and go through the
//! station's sensor-kit intake: decode, ingest, buffering, windows, derived observations with
//! their algorithm versions, alarm evaluation.

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

mod common;

use std::f64::consts::PI;
use std::sync::atomic::Ordering;

use caelum_core::acquisition::observation_record;
use caelum_core::devices::ble::SENSOR_KIT_ADAPTER;
use caelum_core::devices::sensor_kit::crc16_ccitt_false;
use caelum_core::security::role::Role;
use caelum_core::station::Station;
use caelum_devices::ghs::{EcgLead, Waveform, WaveformSignal, WaveformUnit};
use caelum_devices::registry::{DeviceIdentity, Registry};
use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::observation::{Classification, MeasurementMode, Quality, TemperatureSite};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_ingest::{DeviceFlags, Observation, Provenance, TimeSource, Value};
use common::{open, DataDir, MemoryKeyStore, TestClock};

const ADMIN: &str = "ground station passphrase";
const CREW_A: &str = "0b6f3c1e-6a59-4c3e-9a51-6f7c1d2e3a41";
const T0: i64 = 2_100_000_000_000_000_000;
const S: i64 = 1_000_000_000;

fn crew() -> CrewMemberId {
    CrewMemberId::from_uuid(uuid::Uuid::parse_str(CREW_A).unwrap())
}

struct Kit {
    _dir: DataDir,
    clock: TestClock,
    station: Station,
    device: DeviceId,
    seq: u16,
}

impl Kit {
    fn new() -> Self {
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
                adapter: SENSOR_KIT_ADAPTER.into(),
                vendor: "CAELUM".into(),
                model: "sensor shirt".into(),
                serial: "KIT-0001".into(),
                firmware: None,
                accuracy_class: None,
                calibration_due: None,
            })
            .unwrap();
        Registry::new(s.db()).assign(device, crew(), None).unwrap();
        Self {
            _dir: dir,
            clock,
            station: s,
            device,
            seq: 0,
        }
    }

    /// One frame (ICD-SENSOR-KIT §5.1): header, payload, CRC-16/CCITT-FALSE.
    fn send(&mut self, kind: u8, status: u8, at: i64, payload: &[u8]) {
        let mut f = vec![1, kind];
        f.extend_from_slice(&self.seq.to_le_bytes());
        f.push(status);
        f.extend_from_slice(&u64::try_from(at).unwrap().to_le_bytes());
        f.extend_from_slice(payload);
        let crc = crc16_ccitt_false(&f);
        f.extend_from_slice(&crc.to_le_bytes());
        self.seq = self.seq.wrapping_add(1);
        self.clock.0.tai.store(at + S, Ordering::SeqCst);
        self.station.ingest_sensor_kit(self.device, &f).unwrap();
    }

    fn values(&self, parameter: &str) -> Vec<caelum_core::store::ObservationRecord> {
        self.station.db().observations(CREW_A, parameter).unwrap()
    }
}

/// RIP thoracic band (§5.3): 25 Hz, one channel, `per` samples per frame.
fn rip_frame(samples: &[u16]) -> Vec<u8> {
    let mut p = 25_u16.to_le_bytes().to_vec();
    p.push(0b01);
    for v in samples {
        p.extend_from_slice(&v.to_le_bytes());
    }
    p
}

/// Breathing at `bpm` breaths/min, 25 Hz, `seconds` long, in frames of one second.
fn breathe(k: &mut Kit, bpm: f64, seconds: i64, status: u8, from: i64) {
    for sec in 0..seconds {
        let samples: Vec<u16> = (0..25)
            .map(|i| {
                let t = (sec * 25 + i) as f64 / 25.0;
                (30_000.0 + 2_000.0 * (2.0 * PI * bpm / 60.0 * t).sin()) as u16
            })
            .collect();
        k.send(0x02, status, from + sec * S, &rip_frame(&samples));
    }
}

#[test]
fn f_sig_03_rip_frames_give_the_respiration_rate_with_its_algorithm_and_quality() {
    let mut k = Kit::new();
    breathe(&mut k, 15.0, 60, 0, T0);
    let rr = k.values("respiration_rate");
    assert_eq!(rr.len(), 2, "one per 30 s window");
    for r in &rr {
        assert!((r.value.unwrap() - 15.0).abs() < 1.0, "{:?}", r.value);
        assert_eq!(r.classification, "derived");
        assert_eq!(r.algorithm_version.as_deref(), Some("resp-count-adv/1"));
    }
    let sqi = k.values("sqi_resp");
    assert_eq!(sqi.len(), 2);
    assert!(sqi.iter().all(|r| r.quality == "good"), "{sqi:?}");
}

#[test]
fn sr_005_a_lead_off_rip_frame_marks_respiration_unusable_and_restarts_the_window() {
    let mut k = Kit::new();
    breathe(&mut k, 15.0, 20, 0, T0);
    // Status bit 2: sensor off. The 20 s collected so far are dropped, never stitched.
    breathe(&mut k, 15.0, 1, 1 << 2, T0 + 20 * S);
    let sqi = k.values("sqi_resp");
    assert_eq!(sqi.len(), 1);
    assert_eq!(sqi[0].quality, "unusable");
    breathe(&mut k, 15.0, 20, 0, T0 + 21 * S);
    assert!(k.values("respiration_rate").is_empty(), "no window spans the lead-off");
}

#[test]
fn f_acq_10_duplicates_are_dropped_and_a_gap_restarts_the_buffer() {
    let mut k = Kit::new();
    breathe(&mut k, 15.0, 10, 0, T0);
    // The same frame again (same sequence number) changes nothing.
    k.seq -= 1;
    breathe(&mut k, 15.0, 1, 0, T0 + 9 * S);
    // Five frames lost (sequence jumps), and the next one is 5 s later: a new window starts.
    k.seq += 5;
    breathe(&mut k, 15.0, 25, 0, T0 + 15 * S);
    assert!(k.values("respiration_rate").is_empty(), "the gap restarted the window");
    breathe(&mut k, 15.0, 5, 0, T0 + 40 * S);
    assert_eq!(k.values("respiration_rate").len(), 1);
}

/// PPG (§5.4): 50 Hz, red and infrared, u24 counts, 25 sample sets per frame.
fn ppg(k: &mut Kit, bpm: f64, seconds: i64) {
    for half in 0..seconds * 2 {
        let mut p = 50_u16.to_le_bytes().to_vec();
        p.push(0b11);
        for i in 0..25 {
            let t = (half * 25 + i) as f64 / 50.0;
            let pulse = (2.0 * PI * bpm / 60.0 * t).sin();
            for (dc, ac) in [(120_000.0, 600.0), (150_000.0, 1_200.0)] {
                let v = (dc - ac * pulse) as u32;
                p.extend_from_slice(&v.to_le_bytes()[..3]);
            }
        }
        k.send(0x04, 0, T0 + half * S / 2, &p);
    }
}

#[test]
fn f_sig_04_ppg_gives_pulse_rate_and_never_an_uncalibrated_spo2() {
    let mut k = Kit::new();
    ppg(&mut k, 72.0, 16);
    let pr = k.values("pulse_rate");
    assert_eq!(pr.len(), 2, "one per 8 s window");
    for r in &pr {
        assert!((r.value.unwrap() - 72.0).abs() < 3.0, "{:?}", r.value);
        assert_eq!(r.algorithm_version.as_deref(), Some("pr-ppg-acf/1"));
    }
    assert_eq!(k.values("sqi_ppg").len(), 2);
    // ICD-SENSOR-KIT v1 carries no SpO₂ calibration (OI-3): no SpO₂ is made up.
    assert!(k.values("spo2").is_empty());
}

/// IMU (§5.4): 20 Hz, 1 mg per LSB, 10 sample sets per frame (0.5 s).
fn imu(k: &mut Kit, seconds: i64, amplitude_g: f64, from: i64) {
    for half in 0..seconds * 2 {
        let mut p = 20_u16.to_le_bytes().to_vec();
        p.extend_from_slice(&1_000_u16.to_le_bytes()); // µg per LSB
        p.extend_from_slice(&1_000_u16.to_le_bytes()); // m°/s per LSB
        for i in 0..10 {
            let t = (half * 10 + i) as f64 / 20.0;
            let ax = amplitude_g * (2.0 * PI * 2.0 * t).sin();
            for v in [ax, 0.0, 1.0, 0.0, 0.0, 0.0] {
                p.extend_from_slice(&((v * 1_000.0).round() as i16).to_le_bytes());
            }
        }
        k.send(0x03, 0, from + half * S / 2, &p);
    }
}

#[test]
fn f_sig_07_movement_gives_activity_and_eleven_quiet_epochs_give_sleep() {
    let mut k = Kit::new();
    imu(&mut k, 60, 0.2, T0);
    let a = k.values("activity_level");
    assert_eq!(a.len(), 1, "one per 60 s epoch");
    assert!(a[0].value.unwrap() > 0.1, "{:?}", a[0].value);
    assert_eq!(a[0].algorithm_version.as_deref(), Some("activity-motion/1"));
    let wake = k.values("sleep_stage");
    assert_eq!(wake.last().unwrap().value_text.as_deref(), Some("wake"));

    // Eleven quiet epochs: sleep, unstaged (no stage is claimed from actigraphy).
    imu(&mut k, 11 * 60, 0.0, T0 + 60 * S);
    let last = k.values("sleep_stage");
    assert_eq!(last.last().unwrap().value_text.as_deref(), Some("sleep"));
    assert_eq!(
        last.last().unwrap().algorithm_version.as_deref(),
        Some("sleep-wake-actigraphy-hr/1")
    );
}

fn observe(k: &mut Kit, parameter: Parameter, value: f64, at: i64, site: Option<TemperatureSite>) {
    k.clock.0.tai.store(at, Ordering::SeqCst);
    let obs = Observation {
        crew_member_id: crew(),
        device_id: k.device,
        parameter,
        value: Value::Number(value),
        unit: parameter.spec().unit,
        measured_at: TaiNs(at),
        received_at: TaiNs(at),
        quality: Quality::Good,
        classification: Classification::Measured,
        mode: if site.is_some() {
            MeasurementMode::Spot
        } else {
            MeasurementMode::Continuous
        },
        provenance: Provenance {
            time: TimeSource::Receipt,
            original: None,
            site,
        },
    };
    k.station
        .db()
        .insert_observation(&observation_record(&obs).unwrap())
        .unwrap();
    k.station.observe(&obs).unwrap();
}

#[test]
fn f_sig_05_core_temperature_starts_only_from_a_measured_core_temperature() {
    let mut k = Kit::new();
    // Heart rate alone starts nothing: no start value is ever assumed.
    for m in 0..5 {
        observe(&mut k, Parameter::HeartRate, 80.0, T0 + m * 60 * S, None);
        k.station.tick().unwrap();
    }
    assert!(k.values("core_temp").is_empty());
    // A peripheral (armpit) reading does not start it either.
    observe(&mut k, Parameter::BodyTempMeasured, 36.6, T0 + 300 * S, Some(TemperatureSite::Armpit));
    k.station.tick().unwrap();
    assert!(k.values("core_temp").is_empty());
    // A tympanic (core-equivalent) reading does; each minute with heart rate gives an estimate.
    observe(&mut k, Parameter::BodyTempMeasured, 37.0, T0 + 310 * S, Some(TemperatureSite::Tympanum));
    for m in 1..=5 {
        let t = T0 + 310 * S + m * 60 * S;
        observe(&mut k, Parameter::HeartRate, 80.0, t - 30 * S, None);
        k.clock.0.tai.store(t, Ordering::SeqCst);
        k.station.tick().unwrap();
    }
    let ct = k.values("core_temp");
    assert_eq!(ct.len(), 5);
    assert!(ct.iter().all(|r| (r.value.unwrap() - 37.0).abs() < 1.0), "{ct:?}");
    assert_eq!(ct[0].algorithm_version.as_deref(), Some("core-temp-ekf-hr-hf/1"));
    // Without heart rate for 10 minutes the estimate stops (no extrapolation without data).
    let quiet = T0 + 310 * S + 5 * 60 * S;
    for m in 1..=12 {
        k.clock.0.tai.store(quiet + m * 60 * S, Ordering::SeqCst);
        k.station.tick().unwrap();
    }
    let n = k.values("core_temp").len();
    k.clock.0.tai.store(quiet + 20 * 60 * S, Ordering::SeqCst);
    k.station.tick().unwrap();
    assert_eq!(k.values("core_temp").len(), n, "stopped");
}

/// One second of 250 Hz ECG at 60 bpm, R wave scaled by the breathing phase (15 /min).
fn ecg_second(index: i64) -> Waveform {
    let fs = 250.0;
    let samples: Vec<f64> = (0..250)
        .map(|i| {
            let t = index as f64 + f64::from(i) / fs;
            let breath = 1.0 + 0.3 * (2.0 * PI * 0.25 * t).sin();
            let phase = t.fract();
            let r = (-((phase - 0.5) / 0.012).powi(2)).exp() * breath;
            let tw = 0.25 * (-((phase - 0.75) / 0.04).powi(2)).exp();
            r + tw + 0.02 * (2.0 * PI * 10.0 * t).sin()
        })
        .collect();
    Waveform {
        signal: WaveformSignal::Ecg(EcgLead::II),
        start: Some(TaiNs(T0 + index * S)),
        sample_rate_hz: fs,
        unit: WaveformUnit::Millivolt,
        samples,
        quality: Quality::Good,
        flags: DeviceFlags::default(),
        source_code: None,
    }
}

#[test]
fn f_sig_03_without_a_rip_band_respiration_comes_from_the_ecg() {
    let mut k = Kit::new();
    for i in 0..45 {
        k.clock.0.tai.store(T0 + (i + 1) * S, Ordering::SeqCst);
        k.station
            .ingest_waveforms(k.device, vec![(crew(), ecg_second(i))])
            .unwrap();
    }
    let rr = k.values("respiration_rate");
    assert!(!rr.is_empty(), "EDR window");
    assert_eq!(rr[0].algorithm_version.as_deref(), Some("resp-edr-ramp/1"));
    assert!((rr[0].value.unwrap() - 15.0).abs() < 2.0, "{:?}", rr[0].value);
}
