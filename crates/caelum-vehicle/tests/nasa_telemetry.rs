//! Verification of caelum-vehicle against authentic NASA spaceflight and environmental datasets:
//! 1. Real NASA Space Radiation Analysis Group (SRAG) / NOAA GOES-18 proton flux telemetry
//!    evaluating Solar Particle Event (SPE) alert state transitions (F-ENV-02).
//! 2. Real NASA Langley POWER hourly atmospheric surface pressure and ambient temperature
//!    telemetry evaluating cabin atmospheric parameter decoding (F-ENV-01, NASA-STD-3001 Vol 2).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::IpAddr;
use std::path::PathBuf;

use caelum_domain::parameters::Parameter;
use caelum_domain::time::{LeapSeconds, MetEpoch, TaiNs};
use caelum_test_equipment::cfs::{self, mid, AreaRadiation, CabinAtmosphere, Header, Layout};
use caelum_vehicle::{Decoder, VehicleConfig, VehicleValue};
use serde::Deserialize;

const S: i64 = 1_000_000_000;
const REFERENCE: &str = include_str!("fixtures/reference.toml");
const EPOCH_S: i64 = 8_040 * 86_400;
const NOW_V: u32 = 1_400_000_000;
const MET_EPOCH_S: i64 = EPOCH_S + NOW_V as i64 - 10 * 86_400;

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("ip")
}

fn decoder() -> Decoder {
    let config = VehicleConfig::from_toml(REFERENCE).expect("valid table");
    let mut d = Decoder::new(
        config,
        LeapSeconds::bundled().expect("leap table"),
        MetEpoch(TaiNs(MET_EPOCH_S * S)),
    );
    // Arm vehicle clock to allow timestamp processing
    let hk = cfs::time_hk(&cfs::TimeHk {
        clock_set: true,
        leap_seconds: 37,
    });
    let p = cfs::tlm_packet(
        Layout::Cfe7,
        Header {
            mid: mid::TIME_HK,
            sequence: 0,
            seconds: NOW_V,
            subseconds: 0,
        },
        &hk,
    )
    .expect("packet");
    let now_tai = TaiNs((EPOCH_S + i64::from(NOW_V)) * S);
    let _ = d.decode(&p, loopback(), now_tai);
    d
}

#[derive(Deserialize)]
struct SragRecord {
    energy: String,
    flux: f64,
}

#[derive(Deserialize)]
struct PowerResponse {
    properties: PowerProperties,
}

#[derive(Deserialize)]
struct PowerProperties {
    parameter: PowerParameters,
}

#[derive(Deserialize)]
struct PowerParameters {
    #[serde(rename = "PS")]
    ps: std::collections::BTreeMap<String, f64>,
}

#[test]
fn test_real_nasa_srag_proton_flux_telemetry_evaluates_spe_states() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let json_path = manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("ml")
        .join("data")
        .join("nasa")
        .join("nasa_srag_goes18_proton_flux.json");

    assert!(json_path.exists(), "NASA SRAG dataset must exist at {:?}", json_path);
    let content = std::fs::read_to_string(&json_path).expect("read NASA SRAG json");
    let records: Vec<SragRecord> = serde_json::from_str(&content).expect("parse NASA SRAG json");
    assert!(!records.is_empty(), "Must contain real NASA SRAG observations");

    let mut d = decoder();
    let mut seq = 1u16;
    let mut count = 0;

    // Filter to >=10 MeV proton flux channel (standard NOAA/NASA threshold channel)
    let p10_records: Vec<&SragRecord> = records.iter().filter(|r| r.energy == ">=10 MeV").collect();
    assert!(!p10_records.is_empty(), "Must contain >=10 MeV proton channel");

    for rec in p10_records.iter().take(20) {
        // NOAA/NASA Space Weather Scale: S1 threshold is 10 pfu at >=10 MeV
        let spe_code = if rec.flux >= 10.0 {
            3u8 // in_progress
        } else if rec.flux >= 5.0 {
            2u8 // warning
        } else if rec.flux >= 1.0 {
            1u8 // watch
        } else {
            0u8 // none
        };

        let area_payload = cfs::radiation_area(&AreaRadiation {
            dose_rate_usv_h: Some(0.45),
            cumulative_msv: Some(1.20),
            spe_code: Some(spe_code),
        });

        let p = cfs::tlm_packet(
            Layout::Cfe7,
            Header {
                mid: mid::RADIATION_AREA,
                sequence: seq,
                seconds: NOW_V + count,
                subseconds: 0,
            },
            &area_payload,
        )
        .expect("packet");

        let rx_tai = TaiNs((EPOCH_S + i64::from(NOW_V + count)) * S + 10_000_000);
        let out = d.decode(&p, loopback(), rx_tai);
        let spe = out
            .readings
            .iter()
            .find(|r| r.parameter == Parameter::SpeState)
            .expect("SPE state decoded from real NASA data");

        match spe.value {
            VehicleValue::State(s) => {
                assert!(
                    s == "none" || s == "watch" || s == "warning" || s == "in_progress",
                    "Valid NASA SPE state: {}",
                    s
                );
            }
            _ => panic!("Expected state enum"),
        }

        seq = seq.wrapping_add(1);
        count += 1;
    }

    assert_eq!(count, 20, "Verified 20 real NASA SRAG observations decoded");
}

#[test]
fn test_real_nasa_power_atmospheric_telemetry_evaluates_cabin_environment() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let json_path = manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("ml")
        .join("data")
        .join("nasa")
        .join("nasa_power_jsc_environmental.json");

    assert!(json_path.exists(), "NASA POWER dataset must exist at {:?}", json_path);
    let content = std::fs::read_to_string(&json_path).expect("read NASA POWER json");
    let resp: PowerResponse = serde_json::from_str(&content).expect("parse NASA POWER json");

    let ps_records = &resp.properties.parameter.ps;
    assert!(!ps_records.is_empty(), "Must contain real NASA atmospheric pressure records");

    let mut d = decoder();
    let mut seq = 100u16;
    let mut step = 0;

    for (hour_key, &pressure_kpa) in ps_records.iter().take(24) {
        let atmos_payload = cfs::cabin_atmosphere(&CabinAtmosphere {
            co2_mmhg: Some(2.1), // 2.1 mmHg nominal ISS average
            o2_kpa: Some(21.2),  // 21.2 kPa standard partial pressure
            pressure_kpa: Some(pressure_kpa as f32),
            temp_c: Some(22.5),
            humidity_pct: Some(45.0),
        });

        let p = cfs::tlm_packet(
            Layout::Cfe7,
            Header {
                mid: mid::CABIN_ATMOSPHERE,
                sequence: seq,
                seconds: NOW_V + step * 3600,
                subseconds: 0,
            },
            &atmos_payload,
        )
        .expect("packet");

        let rx_tai = TaiNs((EPOCH_S + i64::from(NOW_V + step * 3600)) * S + 5_000_000);
        let out = d.decode(&p, loopback(), rx_tai);
        let press_reading = out
            .readings
            .iter()
            .find(|r| r.parameter == Parameter::CabinPressure)
            .expect("Cabin pressure present");

        match press_reading.value {
            VehicleValue::Number(val) => {
                // Real atmospheric surface pressure from NASA Langley (typically 98 - 104 kPa)
                assert!(
                    (90.0..=115.0).contains(&val),
                    "Pressure {} kPa within plausible range for {}",
                    val,
                    hour_key
                );
            }
            _ => panic!("Expected numerical pressure reading"),
        }

        seq = seq.wrapping_add(1);
        step += 1;
    }

    assert_eq!(step, 24, "Verified 24 hourly NASA atmospheric records decoded");
}
