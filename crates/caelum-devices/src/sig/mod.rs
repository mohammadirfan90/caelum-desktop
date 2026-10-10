//! Decoders for the adopted Bluetooth SIG health profiles (F-ACQ-02):
//!
//! | UUID | Characteristic | Service | Mode |
//! |---|---|---|---|
//! | 0x2A37 | Heart Rate Measurement | HRS | continuous |
//! | 0x2A5E | PLX Spot-Check Measurement | PLXS | spot |
//! | 0x2A5F | PLX Continuous Measurement | PLXS | continuous |
//! | 0x2A35 | Blood Pressure Measurement | BPS | spot |
//! | 0x2A1C | Temperature Measurement | HTS | spot |
//! | 0x2A9D | Weight Measurement | WSS | spot |
//! | 0x2A18 | Glucose Measurement | GLS | spot |
//!
//! Field layouts follow the GATT Specification Supplement. Decoders only read: they never write to
//! a device (R-SEC-07). They map each value to its catalog parameter and leave every field the
//! device did not send empty; ingest then rejects readings without a value (R-CLN-02).
//!
//! Quality: where a profile reports signal or measurement status, it sets the quality. Where it
//! reports nothing, quality is `Degraded` ("not reported"), never `Good` by assumption.
//! Thermometer readings go to `body_temp_measured` with their site, never to `core_temp`.

pub mod ieee;

use caelum_domain::observation::{MeasurementMode, Quality, TemperatureSite};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::{LeapSeconds, TaiNs};
use caelum_domain::units::Unit;
use caelum_ingest::{DeviceFlags, RawValue};

use ieee::Reader;

/// Heart Rate Measurement.
pub const HEART_RATE_MEASUREMENT: u16 = 0x2A37;
/// PLX Spot-Check Measurement.
pub const PLX_SPOT_CHECK: u16 = 0x2A5E;
/// PLX Continuous Measurement.
pub const PLX_CONTINUOUS: u16 = 0x2A5F;
/// Blood Pressure Measurement.
pub const BLOOD_PRESSURE_MEASUREMENT: u16 = 0x2A35;
/// Temperature Measurement.
pub const TEMPERATURE_MEASUREMENT: u16 = 0x2A1C;
/// Weight Measurement.
pub const WEIGHT_MEASUREMENT: u16 = 0x2A9D;
/// Glucose Measurement.
pub const GLUCOSE_MEASUREMENT: u16 = 0x2A18;

/// Every characteristic with a decoder.
pub const SUPPORTED: [u16; 7] = [
    HEART_RATE_MEASUREMENT,
    PLX_SPOT_CHECK,
    PLX_CONTINUOUS,
    BLOOD_PRESSURE_MEASUREMENT,
    TEMPERATURE_MEASUREMENT,
    WEIGHT_MEASUREMENT,
    GLUCOSE_MEASUREMENT,
];

/// Decoding errors. A malformed payload yields no readings at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The payload ends inside a field.
    #[error("payload truncated in {field}: needs {needed} bytes, has {got}")]
    Truncated {
        /// The field being read.
        field: &'static str,
        /// Bytes needed up to the end of the field.
        needed: usize,
        /// Bytes received.
        got: usize,
    },
    /// No decoder for this characteristic.
    #[error("unsupported characteristic {0:#06x}")]
    Unsupported(u16),
    /// A field has a value the specification does not allow.
    #[error("invalid {0}")]
    Invalid(&'static str),
}

/// One value decoded from a characteristic.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// Catalog parameter.
    pub parameter: Parameter,
    /// The value, if the device sent a usable one.
    pub value: Option<RawValue>,
    /// The unit as the device stated it.
    pub unit: Option<Unit>,
    /// The device's timestamp, if it sent one and its clock is set.
    pub device_time: Option<TaiNs>,
    /// Quality from the device's status fields.
    pub quality: Quality,
    /// Demonstration and test flags (SR-017).
    pub flags: DeviceFlags,
    /// Thermometer site (HTS only).
    pub site: Option<TemperatureSite>,
}

/// Everything one notification or indication carried.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    /// Values, in payload order.
    pub readings: Vec<Decoded>,
    /// Continuous or spot (from the profile).
    pub mode: MeasurementMode,
    /// The device's own user number (multi-user cuffs, scales); `None` if not sent or 0xFF.
    pub device_user: Option<u8>,
}

impl Decoded {
    fn number(parameter: Parameter, value: Option<f64>, unit: Unit) -> Self {
        Self {
            parameter,
            value: value.map(RawValue::Number),
            unit: Some(unit),
            device_time: None,
            quality: Quality::Degraded,
            flags: DeviceFlags::default(),
            site: None,
        }
    }

    fn with(mut self, time: Option<TaiNs>, quality: Quality, flags: DeviceFlags) -> Self {
        self.device_time = time;
        self.quality = quality;
        self.flags = flags;
        self
    }
}

/// Decodes one characteristic value.
pub fn decode(
    characteristic: u16,
    payload: &[u8],
    leap: &LeapSeconds,
) -> Result<Measurement, DecodeError> {
    match characteristic {
        HEART_RATE_MEASUREMENT => heart_rate(payload),
        PLX_SPOT_CHECK => plx_spot_check(payload, leap),
        PLX_CONTINUOUS => plx_continuous(payload),
        BLOOD_PRESSURE_MEASUREMENT => blood_pressure(payload, leap),
        TEMPERATURE_MEASUREMENT => temperature(payload, leap),
        WEIGHT_MEASUREMENT => weight(payload, leap),
        GLUCOSE_MEASUREMENT => glucose(payload, leap),
        other => Err(DecodeError::Unsupported(other)),
    }
}

fn bit(value: u32, n: u32) -> bool {
    value & (1 << n) != 0
}

/// HRS 0x2A37. Flags: 0 UINT16 value, 1 contact detected, 2 contact supported, 3 energy
/// expended present, 4 RR-intervals present (1/1024 s each).
fn heart_rate(payload: &[u8]) -> Result<Measurement, DecodeError> {
    let mut r = Reader::new(payload);
    let flags = u32::from(r.u8("flags")?);
    let hr = if bit(flags, 0) {
        r.u16("heart rate")?
    } else {
        u16::from(r.u8("heart rate")?)
    };
    let quality = match (bit(flags, 2), bit(flags, 1)) {
        (true, false) => Quality::Unusable, // contact supported but not detected
        (true, true) => Quality::Good,
        (false, _) => Quality::Degraded, // no contact indicator
    };
    if bit(flags, 3) {
        r.u16("energy expended")?;
    }
    let mut readings =
        vec![
            Decoded::number(Parameter::HeartRate, Some(f64::from(hr)), Unit::PerMinute).with(
                None,
                quality,
                DeviceFlags::default(),
            ),
        ];
    if bit(flags, 4) {
        if !r.remaining().is_multiple_of(2) {
            return Err(DecodeError::Truncated {
                field: "RR-interval",
                needed: payload.len() + 1,
                got: payload.len(),
            });
        }
        while r.remaining() > 0 {
            let rr = r.u16("RR-interval")?;
            let ms = f64::from(rr) * 1_000.0 / 1_024.0;
            readings.push(
                Decoded::number(Parameter::RrInterval, Some(ms), Unit::Millisecond).with(
                    None,
                    quality,
                    DeviceFlags::default(),
                ),
            );
        }
    }
    Ok(Measurement {
        readings,
        mode: MeasurementMode::Continuous,
        device_user: None,
    })
}

/// PLXS Measurement Status (16 bits) and Device and Sensor Status (24 bits) to quality and the
/// SR-017 flags. Returns `None` for the values when the measurement is unavailable (bit 13).
struct PlxStatus {
    quality: Quality,
    flags: DeviceFlags,
    unavailable: bool,
}

fn plx_status(measurement: Option<u16>, sensor: Option<u32>) -> PlxStatus {
    let mut quality = if measurement.is_some() || sensor.is_some() {
        Quality::Good
    } else {
        Quality::Degraded
    };
    let mut flags = DeviceFlags::default();
    let mut unavailable = false;
    if let Some(m) = measurement.map(u32::from) {
        flags.demonstration = bit(m, 10);
        flags.test_data = bit(m, 11);
        unavailable = bit(m, 13);
        if bit(m, 15) || bit(m, 12) {
            quality = Quality::Unusable; // invalid measurement; calibration ongoing
        } else if bit(m, 14) {
            quality = quality.max(Quality::Poor); // questionable measurement
        } else if bit(m, 5) || bit(m, 6) {
            quality = quality.max(Quality::Degraded); // ongoing; early estimate
        }
    }
    if let Some(s) = sensor {
        // 1 equipment malfunction, 11 unconnected to user, 12 unknown sensor, 13 displaced,
        // 14 sensor malfunctioning, 15 sensor disconnected.
        if [1, 11, 12, 13, 14, 15].iter().any(|&b| bit(s, b)) {
            quality = Quality::Unusable;
        // 2 processing irregularity, 3 inadequate, 4 poor signal, 5 low perfusion, 6 erratic,
        // 7 non-pulsatile, 8 questionable pulse, 10 interference.
        } else if [2, 3, 4, 5, 6, 7, 8, 10].iter().any(|&b| bit(s, b)) {
            quality = quality.max(Quality::Poor);
        }
    }
    PlxStatus {
        quality,
        flags,
        unavailable,
    }
}

fn plx_readings(
    spo2: Option<f64>,
    pr: Option<f64>,
    time: Option<TaiNs>,
    status: &PlxStatus,
) -> Vec<Decoded> {
    let (spo2, pr) = if status.unavailable {
        (None, None)
    } else {
        (spo2, pr)
    };
    vec![
        Decoded::number(Parameter::Spo2, spo2, Unit::Percent).with(
            time,
            status.quality,
            status.flags,
        ),
        Decoded::number(Parameter::PulseRate, pr, Unit::PerMinute).with(
            time,
            status.quality,
            status.flags,
        ),
    ]
}

/// PLXS 0x2A5E. Flags: 0 timestamp, 1 measurement status, 2 device and sensor status, 3 pulse
/// amplitude index, 4 device clock is not set.
fn plx_spot_check(payload: &[u8], leap: &LeapSeconds) -> Result<Measurement, DecodeError> {
    let mut r = Reader::new(payload);
    let flags = u32::from(r.u8("flags")?);
    let spo2 = r.sfloat("SpO2")?;
    let pr = r.sfloat("pulse rate")?;
    let mut time = if bit(flags, 0) {
        r.date_time(leap, "timestamp")?
    } else {
        None
    };
    let measurement = bit(flags, 1)
        .then(|| r.u16("measurement status"))
        .transpose()?;
    let sensor = bit(flags, 2)
        .then(|| r.u24("device and sensor status"))
        .transpose()?;
    if bit(flags, 3) {
        r.sfloat("pulse amplitude index")?;
    }
    if bit(flags, 4) {
        time = None; // the device's clock is not set: its timestamp means nothing
    }
    let status = plx_status(measurement, sensor);
    Ok(Measurement {
        readings: plx_readings(spo2, pr, time, &status),
        mode: MeasurementMode::Spot,
        device_user: None,
    })
}

/// PLXS 0x2A5F. Flags: 0 SpO2PR-Fast, 1 SpO2PR-Slow, 2 measurement status, 3 device and sensor
/// status, 4 pulse amplitude index. The normal-response values are used.
fn plx_continuous(payload: &[u8]) -> Result<Measurement, DecodeError> {
    let mut r = Reader::new(payload);
    let flags = u32::from(r.u8("flags")?);
    let spo2 = r.sfloat("SpO2")?;
    let pr = r.sfloat("pulse rate")?;
    if bit(flags, 0) {
        r.skip(4, "SpO2PR-Fast")?;
    }
    if bit(flags, 1) {
        r.skip(4, "SpO2PR-Slow")?;
    }
    let measurement = bit(flags, 2)
        .then(|| r.u16("measurement status"))
        .transpose()?;
    let sensor = bit(flags, 3)
        .then(|| r.u24("device and sensor status"))
        .transpose()?;
    if bit(flags, 4) {
        r.sfloat("pulse amplitude index")?;
    }
    let status = plx_status(measurement, sensor);
    Ok(Measurement {
        readings: plx_readings(spo2, pr, None, &status),
        mode: MeasurementMode::Continuous,
        device_user: None,
    })
}

fn device_user(id: u8) -> Option<u8> {
    (id != 0xFF).then_some(id) // 0xFF: unknown user
}

/// BPS 0x2A35. Flags: 0 units kPa, 1 timestamp, 2 pulse rate, 3 user ID, 4 measurement status
/// (0 body movement, 1 cuff too loose, 2 irregular pulse, 3–4 pulse rate range, 5 improper
/// position).
fn blood_pressure(payload: &[u8], leap: &LeapSeconds) -> Result<Measurement, DecodeError> {
    let mut r = Reader::new(payload);
    let flags = u32::from(r.u8("flags")?);
    let unit = if bit(flags, 0) {
        Unit::KiloPascal
    } else {
        Unit::MmHg
    };
    let systolic = r.sfloat("systolic")?;
    let diastolic = r.sfloat("diastolic")?;
    r.sfloat("mean arterial pressure")?;
    let time = if bit(flags, 1) {
        r.date_time(leap, "timestamp")?
    } else {
        None
    };
    let pulse = bit(flags, 2).then(|| r.sfloat("pulse rate")).transpose()?;
    let user = if bit(flags, 3) {
        device_user(r.u8("user ID")?)
    } else {
        None
    };
    let quality = if bit(flags, 4) {
        let s = u32::from(r.u16("measurement status")?);
        if bit(s, 0) || bit(s, 1) || bit(s, 5) {
            Quality::Poor // movement, loose cuff or wrong position: the value is unreliable
        } else {
            Quality::Good
        }
    } else {
        Quality::Degraded
    };
    let none = DeviceFlags::default();
    let mut readings = vec![
        Decoded::number(Parameter::BpSystolic, systolic, unit).with(time, quality, none),
        Decoded::number(Parameter::BpDiastolic, diastolic, unit).with(time, quality, none),
    ];
    if let Some(pr) = pulse {
        readings.push(
            Decoded::number(Parameter::PulseRate, pr, Unit::PerMinute).with(time, quality, none),
        );
    }
    Ok(Measurement {
        readings,
        mode: MeasurementMode::Spot,
        device_user: user,
    })
}

/// HTS 0x2A1C. Flags: 0 Fahrenheit, 1 timestamp, 2 temperature type (site). Always
/// `body_temp_measured`; no site means `unknown` (set by ingest), never a guess.
fn temperature(payload: &[u8], leap: &LeapSeconds) -> Result<Measurement, DecodeError> {
    let mut r = Reader::new(payload);
    let flags = u32::from(r.u8("flags")?);
    let value = r.float("temperature")?;
    let unit = if bit(flags, 0) {
        Unit::Fahrenheit
    } else {
        Unit::Celsius
    };
    let time = if bit(flags, 1) {
        r.date_time(leap, "timestamp")?
    } else {
        None
    };
    let site = if bit(flags, 2) {
        Some(TemperatureSite::from_hts_code(r.u8("temperature type")?))
    } else {
        None
    };
    let mut reading = Decoded::number(Parameter::BodyTempMeasured, value, unit).with(
        time,
        Quality::Degraded,
        DeviceFlags::default(),
    );
    reading.site = site;
    Ok(Measurement {
        readings: vec![reading],
        mode: MeasurementMode::Spot,
        device_user: None,
    })
}

/// WSS 0x2A9D. Flags: 0 imperial (lb, resolution 0.01) else SI (kg, resolution 0.005), 1
/// timestamp, 2 user ID, 3 BMI and height. 0xFFFF: measurement unsuccessful.
fn weight(payload: &[u8], leap: &LeapSeconds) -> Result<Measurement, DecodeError> {
    let mut r = Reader::new(payload);
    let flags = u32::from(r.u8("flags")?);
    let raw = r.u16("weight")?;
    let (unit, resolution) = if bit(flags, 0) {
        (Unit::Pound, 0.01)
    } else {
        (Unit::Kilogram, 0.005)
    };
    let value = (raw != 0xFFFF).then(|| f64::from(raw) * resolution);
    let time = if bit(flags, 1) {
        r.date_time(leap, "timestamp")?
    } else {
        None
    };
    let user = if bit(flags, 2) {
        device_user(r.u8("user ID")?)
    } else {
        None
    };
    if bit(flags, 3) {
        r.skip(4, "BMI and height")?;
    }
    Ok(Measurement {
        readings: vec![Decoded::number(Parameter::Weight, value, unit).with(
            time,
            Quality::Degraded,
            DeviceFlags::default(),
        )],
        mode: MeasurementMode::Spot,
        device_user: user,
    })
}

/// GLS sample type 10: control solution, a device check, not a person (SR-017: test data).
const GLS_CONTROL_SOLUTION: u8 = 0x0A;

/// GLS 0x2A18. Flags: 0 time offset, 1 concentration + type/location, 2 units mol/L (else kg/L),
/// 3 sensor status annunciation, 4 context follows. Base time is always present.
fn glucose(payload: &[u8], leap: &LeapSeconds) -> Result<Measurement, DecodeError> {
    let mut r = Reader::new(payload);
    let flags = u32::from(r.u8("flags")?);
    r.u16("sequence number")?;
    let base = r.date_time(leap, "base time")?;
    let offset_min = bit(flags, 0).then(|| r.i16("time offset")).transpose()?;
    if !bit(flags, 1) {
        // No concentration in this record: nothing to report (context-only records).
        return Ok(Measurement {
            readings: Vec::new(),
            mode: MeasurementMode::Spot,
            device_user: None,
        });
    }
    let concentration = r.sfloat("glucose concentration")?;
    let type_location = r.u8("type and sample location")?;
    let status = bit(flags, 3)
        .then(|| r.u16("sensor status annunciation"))
        .transpose()?
        .map(u32::from);

    let mut time = match (base, offset_min) {
        (Some(t), Some(m)) => Some(TaiNs(t.0 + i64::from(m) * 60_000_000_000)),
        (t, None) => t,
        (None, Some(_)) => None,
    };
    // kg/L → mg/dL × 100 000; mol/L → mmol/L × 1 000 (exact unit scalings).
    let (value, unit) = if bit(flags, 2) {
        (concentration.map(|c| c * 1_000.0), Unit::MmolPerL)
    } else {
        (concentration.map(|c| c * 100_000.0), Unit::MgPerDl)
    };
    let flags_out = DeviceFlags {
        demonstration: false,
        test_data: type_location & 0x0F == GLS_CONTROL_SOLUTION,
    };
    let mut quality = if status.is_some() {
        Quality::Good
    } else {
        Quality::Degraded
    };
    if let Some(s) = status {
        // 1 sensor malfunction, 2 sample too small, 3 strip insertion error, 4 wrong strip type,
        // 5 result above range, 6 result below range, 9 read interrupted, 10 general fault.
        // ⚕ DRAFT (CR-008): an out-of-range result is not a value, so it is unusable; whether it
        // should raise its own alarm is for the medical lead.
        if [1, 2, 3, 4, 5, 6, 9, 10].iter().any(|&b| bit(s, b)) {
            quality = Quality::Unusable;
        } else if bit(s, 7) || bit(s, 8) {
            quality = Quality::Poor; // sensor temperature out of range
        }
        if bit(s, 11) {
            time = None; // time fault
        }
    }
    Ok(Measurement {
        readings: vec![
            Decoded::number(Parameter::Glucose, value, unit).with(time, quality, flags_out)
        ],
        mode: MeasurementMode::Spot,
        device_user: None,
    })
}
