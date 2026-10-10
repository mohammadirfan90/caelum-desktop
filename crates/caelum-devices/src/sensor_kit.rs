//! CAELUM sensor-kit BLE protocol v1 (F-ACQ-05): sensor shirt and wristband frames, as defined in
//! docs/icd/ICD-SENSOR-KIT.md (DRAFT v1, CR-013). This module implements exactly that ICD.
//!
//! Sans-I/O: [`decode_frame`] decodes one Data notification, [`decode_info`] the Info
//! characteristic, and [`SequenceTracker`] counts dropped and duplicate frames (F-ACQ-10). The
//! protocol is read-only: v1 has no writable characteristic (R-SEC-07; any future write needs a
//! docs/write-exceptions.md entry).
//!
//! Quality: the ICD makes the status flags mandatory in every frame, so a frame that flags no
//! problem is `Good` because the device positively reported its sensor state, not by assumption.
//! Sensor off / lead off → `Unusable`; saturation → `Poor`. Demonstration and test data are
//! flagged (SR-017). A frame with a bad CRC, version, type or length yields an error and no output.

use caelum_domain::observation::Quality;
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_domain::units::Unit;
use caelum_ingest::{DeviceFlags, RawValue};
use uuid::Uuid;

use crate::ghs::{EcgLead, Waveform, WaveformSignal, WaveformUnit};
use crate::sig::ieee::Reader;
use crate::sig::{DecodeError, Decoded};

/// Primary service `cd8c0001-b900-4563-a6dc-fd325d5f51cd` (ICD-SENSOR-KIT §3).
pub const SERVICE_UUID: Uuid = Uuid::from_u128(0xcd8c_0001_b900_4563_a6dc_fd32_5d5f_51cd);
/// Data characteristic `cd8c0002-b900-4563-a6dc-fd325d5f51cd`: notify, one frame per notification.
pub const DATA_UUID: Uuid = Uuid::from_u128(0xcd8c_0002_b900_4563_a6dc_fd32_5d5f_51cd);
/// Info characteristic `cd8c0003-b900-4563-a6dc-fd325d5f51cd`: read.
pub const INFO_UUID: Uuid = Uuid::from_u128(0xcd8c_0003_b900_4563_a6dc_fd32_5d5f_51cd);

/// The only protocol version this decoder accepts.
pub const PROTOCOL_VERSION: u8 = 1;
/// Largest frame: ATT_MTU 247 − 3.
pub const MAX_FRAME_LEN: usize = 244;
/// Largest Info value (ATT attribute maximum).
pub const MAX_INFO_LEN: usize = 512;
/// Version, type, sequence, status, timestamp.
const HEADER_LEN: usize = 13;
const CRC_LEN: usize = 2;

/// CRC-16/CCITT-FALSE: polynomial 0x1021, initial value 0xFFFF, no reflection, no final XOR.
pub fn crc16_ccitt_false(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in bytes {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Checks the trailing CRC and returns the bytes it covers.
fn verified<'a>(bytes: &'a [u8], min: usize, field: &'static str) -> Result<&'a [u8], DecodeError> {
    if bytes.len() < min {
        return Err(DecodeError::Truncated {
            field,
            needed: min,
            got: bytes.len(),
        });
    }
    let (body, crc) = bytes.split_at(bytes.len() - CRC_LEN);
    let sent = match *crc {
        [lo, hi] => u16::from_le_bytes([lo, hi]),
        _ => return Err(DecodeError::Invalid("CRC")),
    };
    if crc16_ccitt_false(body) != sent {
        return Err(DecodeError::Invalid("CRC mismatch"));
    }
    Ok(body)
}

/// Frame type (ICD-SENSOR-KIT §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// 0x01 ECG.
    Ecg,
    /// 0x02 RIP respiration.
    Rip,
    /// 0x03 IMU.
    Imu,
    /// 0x04 PPG.
    Ppg,
    /// 0x05 temperature and heat flux.
    Temperature,
    /// 0x06 EDA.
    Eda,
    /// 0x07 device status.
    Status,
}

impl FrameType {
    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0x01 => Self::Ecg,
            0x02 => Self::Rip,
            0x03 => Self::Imu,
            0x04 => Self::Ppg,
            0x05 => Self::Temperature,
            0x06 => Self::Eda,
            0x07 => Self::Status,
            _ => return None,
        })
    }
}

/// A sensor channel, as named in the Info calibration records and the status contact bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KitChannel {
    /// 1: ECG electrodes.
    Ecg,
    /// 2: RIP thoracic band.
    RipThoracic,
    /// 3: RIP abdominal band.
    RipAbdominal,
    /// 4: accelerometer.
    Accelerometer,
    /// 5: gyroscope.
    Gyroscope,
    /// 6: PPG optical sensor.
    Ppg,
    /// 7: EDA electrodes.
    Eda,
    /// 8: proximal skin temperature sensor.
    SkinTempProximal,
    /// 9: distal skin temperature sensor.
    SkinTempDistal,
    /// 10: heat-flux sensor.
    HeatFlux,
}

impl KitChannel {
    fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            1 => Self::Ecg,
            2 => Self::RipThoracic,
            3 => Self::RipAbdominal,
            4 => Self::Accelerometer,
            5 => Self::Gyroscope,
            6 => Self::Ppg,
            7 => Self::Eda,
            8 => Self::SkinTempProximal,
            9 => Self::SkinTempDistal,
            10 => Self::HeatFlux,
            _ => return None,
        })
    }
}

/// Status-frame contact bits, bit 0 to bit 7.
const CONTACT_BITS: [KitChannel; 8] = [
    KitChannel::Ecg,
    KitChannel::RipThoracic,
    KitChannel::RipAbdominal,
    KitChannel::Ppg,
    KitChannel::Eda,
    KitChannel::SkinTempProximal,
    KitChannel::SkinTempDistal,
    KitChannel::HeatFlux,
];

/// Heat flux from the shirt, for the core-temperature estimate (F-SIG-05). Not a catalog
/// parameter: never mapped to one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeatFlux {
    /// W/m², positive out of the body.
    pub w_per_m2: f64,
    /// Device time of the measurement.
    pub time: Option<TaiNs>,
    /// Quality from the frame status.
    pub quality: Quality,
    /// Demonstration and test flags (SR-017).
    pub flags: DeviceFlags,
}

/// Electrode or sensor contact reported by a status frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Contact {
    /// The sensor.
    pub channel: KitChannel,
    /// Whether contact is detected.
    pub detected: bool,
}

/// Device status frame contents (F-ACQ-10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceStatus {
    /// Battery charge, %; `None` if the device does not know.
    pub battery_percent: Option<u8>,
    /// Contact state of each sensor the device has a contact detector for.
    pub contacts: Vec<Contact>,
}

/// Everything one frame carried.
#[derive(Debug, Clone, PartialEq)]
pub struct SensorKitFrame {
    /// Frame type.
    pub frame_type: FrameType,
    /// Per-device sequence number (feed to [`SequenceTracker`]).
    pub sequence: u16,
    /// Device time of the first sample; `None` if the device's clock is not set.
    pub device_time: Option<TaiNs>,
    /// Quality from the status flags.
    pub quality: Quality,
    /// Demonstration and test flags (SR-017).
    pub flags: DeviceFlags,
    /// Waveforms (ECG in mV; IMU in g and °/s; RIP and PPG in raw counts).
    pub waveforms: Vec<Waveform>,
    /// Catalog readings: skin temperatures, EDA, battery level. Always continuous mode.
    pub readings: Vec<Decoded>,
    /// Heat flux (temperature frames).
    pub heat_flux: Option<HeatFlux>,
    /// Device status (status frames).
    pub status: Option<DeviceStatus>,
}

/// Status flag bits (ICD-SENSOR-KIT §5.1).
const ST_DEMONSTRATION: u8 = 1 << 0;
const ST_TEST: u8 = 1 << 1;
const ST_SENSOR_OFF: u8 = 1 << 2;
const ST_SATURATION: u8 = 1 << 3;
const ST_CLOCK_NOT_SET: u8 = 1 << 4;
const ST_RESERVED: u8 = 0xE0;

/// Common context of one frame.
struct Ctx {
    time: Option<TaiNs>,
    quality: Quality,
    flags: DeviceFlags,
}

impl Ctx {
    fn waveform(
        &self,
        signal: WaveformSignal,
        rate: u16,
        unit: WaveformUnit,
        samples: Vec<f64>,
    ) -> Waveform {
        Waveform {
            signal,
            start: self.time,
            sample_rate_hz: f64::from(rate),
            unit,
            samples,
            quality: self.quality,
            flags: self.flags,
            source_code: None,
        }
    }

    fn reading(&self, parameter: Parameter, value: f64, unit: Unit) -> Decoded {
        Decoded {
            parameter,
            value: Some(RawValue::Number(value)),
            unit: Some(unit),
            device_time: self.time,
            quality: self.quality,
            flags: self.flags,
            site: None,
        }
    }
}

/// Decodes one Data notification (F-ACQ-05).
pub fn decode_frame(frame: &[u8]) -> Result<SensorKitFrame, DecodeError> {
    if frame.len() > MAX_FRAME_LEN {
        return Err(DecodeError::Invalid("frame longer than 244 bytes"));
    }
    let body = verified(frame, HEADER_LEN + CRC_LEN, "frame")?;
    let mut r = Reader::new(body);
    if r.u8("protocol version")? != PROTOCOL_VERSION {
        return Err(DecodeError::Invalid("protocol version"));
    }
    let frame_type =
        FrameType::from_code(r.u8("frame type")?).ok_or(DecodeError::Invalid("frame type"))?;
    let sequence = r.u16("sequence number")?;
    let status = r.u8("status flags")?;
    let timestamp = r.u64("device timestamp")?;
    if status & ST_RESERVED != 0 {
        return Err(DecodeError::Invalid("reserved status flags set"));
    }
    if frame_type == FrameType::Status && status & (ST_SENSOR_OFF | ST_SATURATION) != 0 {
        return Err(DecodeError::Invalid("sensor flags in a status frame"));
    }
    let time = if status & ST_CLOCK_NOT_SET != 0 || timestamp == 0 {
        None
    } else {
        Some(TaiNs(
            i64::try_from(timestamp).map_err(|_| DecodeError::Invalid("device timestamp"))?,
        ))
    };
    let quality = if status & ST_SENSOR_OFF != 0 {
        Quality::Unusable
    } else if status & ST_SATURATION != 0 {
        Quality::Poor
    } else {
        Quality::Good
    };
    let ctx = Ctx {
        time,
        quality,
        flags: DeviceFlags {
            demonstration: status & ST_DEMONSTRATION != 0,
            test_data: status & ST_TEST != 0,
        },
    };
    let mut out = SensorKitFrame {
        frame_type,
        sequence,
        device_time: time,
        quality,
        flags: ctx.flags,
        waveforms: Vec::new(),
        readings: Vec::new(),
        heat_flux: None,
        status: None,
    };
    match frame_type {
        FrameType::Ecg => out.waveforms = ecg(&mut r, &ctx)?,
        FrameType::Rip => out.waveforms = rip(&mut r, &ctx)?,
        FrameType::Imu => out.waveforms = imu(&mut r, &ctx)?,
        FrameType::Ppg => out.waveforms = ppg(&mut r, &ctx)?,
        FrameType::Temperature => temperature(&mut r, &ctx, &mut out)?,
        FrameType::Eda => {
            let raw = r.u16("EDA")?;
            if raw != 0xFFFF {
                out.readings.push(ctx.reading(
                    Parameter::Eda,
                    f64::from(raw) * 0.01,
                    Unit::Microsiemens,
                ));
            }
        }
        FrameType::Status => status_frame(&mut r, &ctx, &mut out)?,
    }
    if r.remaining() != 0 {
        return Err(DecodeError::Invalid("frame length"));
    }
    Ok(out)
}

fn rate(r: &mut Reader<'_>) -> Result<u16, DecodeError> {
    match r.u16("sample rate")? {
        0 => Err(DecodeError::Invalid("sample rate")),
        hz => Ok(hz),
    }
}

/// Splits the remaining bytes into `channels` interleaved sample streams of `width` bytes each.
fn interleaved(
    r: &mut Reader<'_>,
    channels: usize,
    width: usize,
    sample: impl Fn(&[u8]) -> f64,
) -> Result<Vec<Vec<f64>>, DecodeError> {
    let set = channels * width;
    let n = r.remaining();
    if channels == 0 || n == 0 || !n.is_multiple_of(set) {
        return Err(DecodeError::Invalid(
            "sample bytes not a whole number of sample sets",
        ));
    }
    let raw = r.bytes(n, "samples")?;
    let mut out = vec![Vec::with_capacity(n / set); channels];
    for row in raw.chunks_exact(set) {
        for (stream, value) in out.iter_mut().zip(row.chunks_exact(width)) {
            stream.push(sample(value));
        }
    }
    Ok(out)
}

/// Unsigned little-endian integer of up to 4 bytes.
fn unsigned(b: &[u8]) -> u32 {
    b.iter()
        .rev()
        .fold(0u32, |acc, &x| (acc << 8) | u32::from(x))
}

fn unsigned_of(b: &[u8]) -> f64 {
    f64::from(unsigned(b))
}

/// Two's-complement little-endian integer of 1 to 4 bytes.
fn signed_of(b: &[u8]) -> f64 {
    let value = i64::from(unsigned(b));
    let bits = 8 * b.len().min(4);
    let sign = 1i64 << bits.saturating_sub(1);
    if bits > 0 && value >= sign {
        (value - (sign << 1)) as f64
    } else {
        value as f64
    }
}

fn ecg_lead(id: u8) -> Option<EcgLead> {
    Some(match id {
        1 => EcgLead::I,
        2 => EcgLead::II,
        3 => EcgLead::III,
        4 => EcgLead::V1,
        5 => EcgLead::V2,
        6 => EcgLead::V3,
        7 => EcgLead::V4,
        8 => EcgLead::V5,
        9 => EcgLead::V6,
        _ => return None,
    })
}

/// 0x01: rate u16 Hz, lead count u8 (1–3), lead ids, resolution u16 nV/LSB, interleaved i16.
fn ecg(r: &mut Reader<'_>, ctx: &Ctx) -> Result<Vec<Waveform>, DecodeError> {
    let hz = rate(r)?;
    let count = r.u8("lead count")?;
    if !(1..=3).contains(&count) {
        return Err(DecodeError::Invalid("lead count"));
    }
    let mut leads = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let lead = ecg_lead(r.u8("lead id")?).ok_or(DecodeError::Invalid("lead id"))?;
        if leads.contains(&lead) {
            return Err(DecodeError::Invalid("duplicate lead id"));
        }
        leads.push(lead);
    }
    let nv = r.u16("ECG resolution")?;
    if nv == 0 {
        return Err(DecodeError::Invalid("ECG resolution"));
    }
    let mv_per_lsb = f64::from(nv) / 1e6;
    let streams = interleaved(r, leads.len(), 2, |b| signed_of(b) * mv_per_lsb)?;
    Ok(leads
        .into_iter()
        .zip(streams)
        .map(|(lead, s)| ctx.waveform(WaveformSignal::Ecg(lead), hz, WaveformUnit::Millivolt, s))
        .collect())
}

/// Channels selected by `mask` (bit order), rejecting reserved bits and an empty mask.
fn channels(mask: u8, signals: &[WaveformSignal]) -> Result<Vec<WaveformSignal>, DecodeError> {
    let valid = u8::try_from((1u16 << signals.len()) - 1)
        .map_err(|_| DecodeError::Invalid("channel mask"))?;
    if mask == 0 || mask & !valid != 0 {
        return Err(DecodeError::Invalid("channel mask"));
    }
    Ok(signals
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, s)| *s)
        .collect())
}

/// 0x02: rate u16 Hz, channel mask u8 (bit 0 thoracic, bit 1 abdominal), interleaved u16 raw
/// counts (relative, uncalibrated).
fn rip(r: &mut Reader<'_>, ctx: &Ctx) -> Result<Vec<Waveform>, DecodeError> {
    let hz = rate(r)?;
    let signals = channels(
        r.u8("channel mask")?,
        &[WaveformSignal::RipThoracic, WaveformSignal::RipAbdominal],
    )?;
    let streams = interleaved(r, signals.len(), 2, unsigned_of)?;
    Ok(signals
        .into_iter()
        .zip(streams)
        .map(|(sig, s)| ctx.waveform(sig, hz, WaveformUnit::Counts, s))
        .collect())
}

/// 0x03: rate u16 Hz, accel resolution u16 µg/LSB, gyro resolution u16 m°/s per LSB, then
/// 6 × i16 per sample (ax ay az gx gy gz).
fn imu(r: &mut Reader<'_>, ctx: &Ctx) -> Result<Vec<Waveform>, DecodeError> {
    let hz = rate(r)?;
    let accel = r.u16("accelerometer resolution")?;
    let gyro = r.u16("gyroscope resolution")?;
    if accel == 0 || gyro == 0 {
        return Err(DecodeError::Invalid("IMU resolution"));
    }
    let streams = interleaved(r, 6, 2, signed_of)?;
    let g = f64::from(accel) / 1e6;
    let dps = f64::from(gyro) / 1e3;
    let axes = [
        (WaveformSignal::AccelX, g, WaveformUnit::G),
        (WaveformSignal::AccelY, g, WaveformUnit::G),
        (WaveformSignal::AccelZ, g, WaveformUnit::G),
        (WaveformSignal::GyroX, dps, WaveformUnit::DegreePerSecond),
        (WaveformSignal::GyroY, dps, WaveformUnit::DegreePerSecond),
        (WaveformSignal::GyroZ, dps, WaveformUnit::DegreePerSecond),
    ];
    Ok(axes
        .into_iter()
        .zip(streams)
        .map(|((sig, scale, unit), s)| {
            ctx.waveform(sig, hz, unit, s.into_iter().map(|v| v * scale).collect())
        })
        .collect())
}

/// 0x04: rate u16 Hz, channel mask u8 (bit 0 red, 1 infrared, 2 green, 3 ambient), interleaved
/// u24 raw ADC counts.
fn ppg(r: &mut Reader<'_>, ctx: &Ctx) -> Result<Vec<Waveform>, DecodeError> {
    let hz = rate(r)?;
    let signals = channels(
        r.u8("channel mask")?,
        &[
            WaveformSignal::PpgRed,
            WaveformSignal::PpgInfrared,
            WaveformSignal::PpgGreen,
            WaveformSignal::PpgAmbient,
        ],
    )?;
    let streams = interleaved(r, signals.len(), 3, unsigned_of)?;
    Ok(signals
        .into_iter()
        .zip(streams)
        .map(|(sig, s)| ctx.waveform(sig, hz, WaveformUnit::Counts, s))
        .collect())
}

/// Not measured (i16 temperature-frame fields).
const NOT_MEASURED_I16: i16 = i16::MIN;

/// 0x05: skin proximal i16 0.01 °C, skin distal i16 0.01 °C, heat flux i16 0.1 W/m²;
/// 0x8000 = not measured (no value).
fn temperature(r: &mut Reader<'_>, ctx: &Ctx, out: &mut SensorKitFrame) -> Result<(), DecodeError> {
    let proximal = r.i16("skin temperature proximal")?;
    let distal = r.i16("skin temperature distal")?;
    let flux = r.i16("heat flux")?;
    for (raw, parameter) in [
        (proximal, Parameter::SkinTempProximal),
        (distal, Parameter::SkinTempDistal),
    ] {
        if raw != NOT_MEASURED_I16 {
            out.readings
                .push(ctx.reading(parameter, f64::from(raw) * 0.01, Unit::Celsius));
        }
    }
    if flux != NOT_MEASURED_I16 {
        out.heat_flux = Some(HeatFlux {
            w_per_m2: f64::from(flux) * 0.1,
            time: ctx.time,
            quality: ctx.quality,
            flags: ctx.flags,
        });
    }
    Ok(())
}

/// 0x07: battery u8 % (0xFF unknown), contact supported u8, contact detected u8.
fn status_frame(
    r: &mut Reader<'_>,
    ctx: &Ctx,
    out: &mut SensorKitFrame,
) -> Result<(), DecodeError> {
    let battery = r.u8("battery level")?;
    let supported = r.u8("contact supported")?;
    let detected = r.u8("contact detected")?;
    let battery_percent = match battery {
        0xFF => None,
        0..=100 => Some(battery),
        _ => return Err(DecodeError::Invalid("battery level")),
    };
    if detected & !supported != 0 {
        return Err(DecodeError::Invalid(
            "contact detected on an unsupported sensor",
        ));
    }
    if let Some(pct) = battery_percent {
        out.readings
            .push(ctx.reading(Parameter::BatteryLevel, f64::from(pct), Unit::Percent));
    }
    out.status = Some(DeviceStatus {
        battery_percent,
        contacts: CONTACT_BITS
            .iter()
            .enumerate()
            .filter(|(i, _)| supported & (1 << i) != 0)
            .map(|(i, &channel)| Contact {
                channel,
                detected: detected & (1 << i) != 0,
            })
            .collect(),
    });
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Info characteristic.
// ---------------------------------------------------------------------------------------------

/// Kind of sensor-kit device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// 1: sensor shirt.
    Shirt,
    /// 2: wristband.
    Wristband,
}

/// What a calibration constant is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationKind {
    /// 1: gain.
    Gain,
    /// 2: offset.
    Offset,
}

/// One calibration constant. The device has already applied it to the samples it sends; CAELUM
/// records it for provenance and maintenance and never applies it again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    /// Channel.
    pub channel: KitChannel,
    /// Gain or offset.
    pub kind: CalibrationKind,
    /// The value (IEEE 11073-20601 FLOAT on the wire).
    pub value: f64,
}

/// Info characteristic contents.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceInfo {
    /// Protocol version (always [`PROTOCOL_VERSION`]).
    pub protocol_version: u8,
    /// Shirt or wristband.
    pub kind: DeviceKind,
    /// Firmware version (major, minor, patch).
    pub firmware: (u8, u8, u16),
    /// Serial number (printable ASCII).
    pub serial: String,
    /// Calibration constants.
    pub calibration: Vec<Calibration>,
}

const SERIAL_LEN: usize = 16;
const INFO_FIXED_LEN: usize = 6 + SERIAL_LEN + 1;

/// Decodes the Info characteristic (F-ACQ-05).
pub fn decode_info(bytes: &[u8]) -> Result<DeviceInfo, DecodeError> {
    if bytes.len() > MAX_INFO_LEN {
        return Err(DecodeError::Invalid("info longer than 512 bytes"));
    }
    let body = verified(bytes, INFO_FIXED_LEN + CRC_LEN, "info")?;
    let mut r = Reader::new(body);
    let protocol_version = r.u8("protocol version")?;
    if protocol_version != PROTOCOL_VERSION {
        return Err(DecodeError::Invalid("protocol version"));
    }
    let kind = match r.u8("device kind")? {
        1 => DeviceKind::Shirt,
        2 => DeviceKind::Wristband,
        _ => return Err(DecodeError::Invalid("device kind")),
    };
    let firmware = (
        r.u8("firmware major")?,
        r.u8("firmware minor")?,
        r.u16("firmware patch")?,
    );
    let raw_serial = r.bytes(SERIAL_LEN, "serial number")?;
    let end = raw_serial
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(SERIAL_LEN);
    let (serial, padding) = raw_serial.split_at(end);
    if serial.is_empty()
        || !serial.iter().all(|b| b.is_ascii_graphic() || *b == b' ')
        || padding.iter().any(|&b| b != 0)
    {
        return Err(DecodeError::Invalid("serial number"));
    }
    let serial = String::from_utf8_lossy(serial).into_owned();
    let count = r.u8("calibration count")?;
    let mut calibration = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let channel = KitChannel::from_id(r.u8("calibration channel")?)
            .ok_or(DecodeError::Invalid("calibration channel"))?;
        let kind = match r.u8("calibration kind")? {
            1 => CalibrationKind::Gain,
            2 => CalibrationKind::Offset,
            _ => return Err(DecodeError::Invalid("calibration kind")),
        };
        let value = r
            .float("calibration value")?
            .ok_or(DecodeError::Invalid("calibration value"))?;
        calibration.push(Calibration {
            channel,
            kind,
            value,
        });
    }
    if r.remaining() != 0 {
        return Err(DecodeError::Invalid("info length"));
    }
    Ok(DeviceInfo {
        protocol_version,
        kind,
        firmware,
        serial,
        calibration,
    })
}

// ---------------------------------------------------------------------------------------------
// Sequence tracking.
// ---------------------------------------------------------------------------------------------

/// What one sequence number means relative to the previous one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequenceEvent {
    /// The first frame seen.
    First,
    /// The next number (including the wrap 0xFFFF → 0).
    InOrder,
    /// `missing` frames were lost before this one.
    Gap {
        /// Frames lost.
        missing: u16,
    },
    /// The same number as the previous frame: drop this frame.
    Duplicate,
    /// The number went backwards (or jumped by exactly half the range): the device restarted its
    /// counter. Tracking restarts here; nothing is counted as dropped.
    Restart,
}

/// Per-device frame sequence accounting for the dropped-packet counters (F-ACQ-10). Sans-I/O; call
/// [`Self::reset`] on reconnection.
#[derive(Debug, Clone, Default)]
pub struct SequenceTracker {
    last: Option<u16>,
    /// Frames accepted (first, in order or after a gap or restart).
    pub received: u64,
    /// Frames lost in gaps.
    pub dropped: u64,
    /// Duplicate frames.
    pub duplicates: u64,
    /// Wraps 0xFFFF → 0.
    pub wraps: u64,
    /// Counter restarts.
    pub restarts: u64,
}

impl SequenceTracker {
    /// A tracker that has seen nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the last number (new connection); counters are kept.
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// Accounts for one frame's sequence number.
    pub fn observe(&mut self, seq: u16) -> SequenceEvent {
        let Some(last) = self.last else {
            self.last = Some(seq);
            self.received += 1;
            return SequenceEvent::First;
        };
        let ahead = seq.wrapping_sub(last);
        let event = match ahead {
            0 => {
                self.duplicates += 1;
                return SequenceEvent::Duplicate;
            }
            1..=0x7FFF => {
                if seq < last {
                    self.wraps += 1;
                }
                if ahead == 1 {
                    SequenceEvent::InOrder
                } else {
                    let missing = ahead - 1;
                    self.dropped += u64::from(missing);
                    SequenceEvent::Gap { missing }
                }
            }
            _ => {
                self.restarts += 1;
                SequenceEvent::Restart
            }
        };
        self.last = Some(seq);
        self.received += 1;
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only encoder: header, payload, CRC.
    fn frame(kind: u8, seq: u16, status: u8, time: u64, payload: &[u8]) -> Vec<u8> {
        let mut f = vec![PROTOCOL_VERSION, kind];
        f.extend_from_slice(&seq.to_le_bytes());
        f.push(status);
        f.extend_from_slice(&time.to_le_bytes());
        f.extend_from_slice(payload);
        let crc = crc16_ccitt_false(&f);
        f.extend_from_slice(&crc.to_le_bytes());
        f
    }

    fn le16s(values: &[i16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    const T: u64 = 2_145_916_837_000_000_000; // 2026-01-01T00:00:00 UTC as TAI ns since 1958

    #[test]
    fn f_acq_05_uuids_match_the_icd() {
        assert_eq!(
            SERVICE_UUID.to_string(),
            "cd8c0001-b900-4563-a6dc-fd325d5f51cd"
        );
        assert_eq!(
            DATA_UUID.to_string(),
            "cd8c0002-b900-4563-a6dc-fd325d5f51cd"
        );
        assert_eq!(
            INFO_UUID.to_string(),
            "cd8c0003-b900-4563-a6dc-fd325d5f51cd"
        );
    }

    #[test]
    fn f_acq_05_crc16_ccitt_false_check_value() {
        assert_eq!(crc16_ccitt_false(b"123456789"), 0x29B1);
        assert_eq!(crc16_ccitt_false(b""), 0xFFFF);
    }

    #[test]
    fn f_acq_05_icd_example_frames_decode() {
        // ICD-SENSOR-KIT §6 example 1: EDA 5.00 µS.
        let eda = [
            0x01, 0x06, 0x01, 0x00, 0x00, 0x00, 0x32, 0xEA, 0x62, 0xFE, 0xD3, 0xC7, 0x1D, 0xF4,
            0x01, 0x52, 0x4A,
        ];
        let f = decode_frame(&eda).unwrap();
        assert_eq!(f.device_time, Some(TaiNs(2_145_916_837_000_000_000)));
        assert_eq!(f.readings[0].parameter, Parameter::Eda);
        assert_eq!(f.readings[0].value, Some(RawValue::Number(5.0)));
        assert_eq!(f.readings[0].unit, Some(Unit::Microsiemens));
        assert_eq!(f.quality, Quality::Good);
        // Example 2: temperature 34.12 °C proximal, distal not measured, heat flux -12.5 W/m².
        let temp = [
            0x01, 0x05, 0x2A, 0x00, 0x00, 0x00, 0x32, 0xEA, 0x62, 0xFE, 0xD3, 0xC7, 0x1D, 0x54,
            0x0D, 0x00, 0x80, 0x83, 0xFF, 0xBD, 0xBD,
        ];
        let f = decode_frame(&temp).unwrap();
        assert_eq!(f.sequence, 42);
        assert_eq!(f.readings.len(), 1, "distal not measured: no reading");
        assert_eq!(f.readings[0].parameter, Parameter::SkinTempProximal);
        assert_eq!(f.readings[0].value, Some(RawValue::Number(34.12)));
        let flux = f.heat_flux.unwrap();
        assert!((flux.w_per_m2 + 12.5).abs() < 1e-9);
    }

    #[test]
    fn f_acq_05_ecg_round_trip() {
        let mut p = 250u16.to_le_bytes().to_vec();
        p.extend_from_slice(&[2, 1, 2]); // leads I, II
        p.extend_from_slice(&1000u16.to_le_bytes()); // 1000 nV/LSB = 1 µV
        p.extend_from_slice(&le16s(&[1000, -500, 2000, -1000]));
        let f = decode_frame(&frame(0x01, 7, 0, T, &p)).unwrap();
        assert_eq!(f.waveforms.len(), 2);
        let (i, ii) = (&f.waveforms[0], &f.waveforms[1]);
        assert_eq!(i.signal, WaveformSignal::Ecg(EcgLead::I));
        assert_eq!(ii.signal, WaveformSignal::Ecg(EcgLead::II));
        assert_eq!(i.samples, vec![1.0, 2.0]);
        assert_eq!(ii.samples, vec![-0.5, -1.0]);
        assert_eq!(i.unit, WaveformUnit::Millivolt);
        assert_eq!(i.sample_rate_hz, 250.0);
        assert_eq!(i.start, Some(TaiNs(2_145_916_837_000_000_000)));
        // Odd sample count for two leads, bad lead id, duplicate lead, zero resolution.
        let mut odd = p.clone();
        odd.extend_from_slice(&[0, 0]);
        assert!(decode_frame(&frame(0x01, 7, 0, T, &odd)).is_err());
        let bad_lead = [
            &250u16.to_le_bytes()[..],
            &[1, 10],
            &1000u16.to_le_bytes(),
            &[0, 0],
        ]
        .concat();
        assert!(decode_frame(&frame(0x01, 7, 0, T, &bad_lead)).is_err());
        let dup = [
            &250u16.to_le_bytes()[..],
            &[2, 1, 1],
            &1000u16.to_le_bytes(),
            &[0; 4],
        ]
        .concat();
        assert!(decode_frame(&frame(0x01, 7, 0, T, &dup)).is_err());
        let zero_res = [&250u16.to_le_bytes()[..], &[1, 1], &[0, 0], &[0; 2]].concat();
        assert!(decode_frame(&frame(0x01, 7, 0, T, &zero_res)).is_err());
        let no_samples = [&250u16.to_le_bytes()[..], &[1, 1], &1000u16.to_le_bytes()].concat();
        assert!(decode_frame(&frame(0x01, 7, 0, T, &no_samples)).is_err());
        let zero_rate = [&[0u8, 0][..], &[1, 1], &1000u16.to_le_bytes(), &[0; 2]].concat();
        assert!(decode_frame(&frame(0x01, 7, 0, T, &zero_rate)).is_err());
    }

    #[test]
    fn f_acq_05_rip_round_trip() {
        let mut p = 25u16.to_le_bytes().to_vec();
        p.push(0b10); // abdominal only
        p.extend_from_slice(&[0x10, 0x27, 0x11, 0x27]); // 10000, 10001
        let f = decode_frame(&frame(0x02, 1, 0, T, &p)).unwrap();
        assert_eq!(f.waveforms.len(), 1);
        assert_eq!(f.waveforms[0].signal, WaveformSignal::RipAbdominal);
        assert_eq!(f.waveforms[0].unit, WaveformUnit::Counts);
        assert_eq!(f.waveforms[0].samples, vec![10000.0, 10001.0]);
        let both = [&25u16.to_le_bytes()[..], &[0b11], &[1, 0, 2, 0, 3, 0]].concat();
        assert!(
            decode_frame(&frame(0x02, 1, 0, T, &both)).is_err(),
            "3 values, 2 channels"
        );
        let reserved = [&25u16.to_le_bytes()[..], &[0b100], &[1, 0]].concat();
        assert!(decode_frame(&frame(0x02, 1, 0, T, &reserved)).is_err());
        let empty_mask = [&25u16.to_le_bytes()[..], &[0], &[1, 0]].concat();
        assert!(decode_frame(&frame(0x02, 1, 0, T, &empty_mask)).is_err());
    }

    #[test]
    fn f_acq_05_imu_round_trip() {
        let mut p = 100u16.to_le_bytes().to_vec();
        p.extend_from_slice(&1000u16.to_le_bytes()); // 1 mg/LSB
        p.extend_from_slice(&70u16.to_le_bytes()); // 70 m°/s per LSB
        p.extend_from_slice(&le16s(&[1000, -1000, 0, 100, -100, 0]));
        let f = decode_frame(&frame(0x03, 1, 0, T, &p)).unwrap();
        let s: Vec<_> = f
            .waveforms
            .iter()
            .map(|w| (w.signal, w.unit, w.samples[0]))
            .collect();
        assert_eq!(s[0], (WaveformSignal::AccelX, WaveformUnit::G, 1.0));
        assert_eq!(s[1], (WaveformSignal::AccelY, WaveformUnit::G, -1.0));
        assert_eq!(s[3].0, WaveformSignal::GyroX);
        assert_eq!(s[3].1, WaveformUnit::DegreePerSecond);
        assert!((s[3].2 - 7.0).abs() < 1e-9);
        assert!((s[4].2 + 7.0).abs() < 1e-9);
        let mut short = p.clone();
        short.extend_from_slice(&[0; 2]);
        assert!(decode_frame(&frame(0x03, 1, 0, T, &short)).is_err());
    }

    #[test]
    fn f_acq_05_ppg_round_trip() {
        let mut p = 100u16.to_le_bytes().to_vec();
        p.push(0b1011); // red, IR, ambient
        p.extend_from_slice(&[1, 0, 0, 2, 0, 0, 0xFF, 0xFF, 0xFF]);
        let f = decode_frame(&frame(0x04, 1, 0, T, &p)).unwrap();
        let s: Vec<_> = f
            .waveforms
            .iter()
            .map(|w| (w.signal, w.samples.clone()))
            .collect();
        assert_eq!(
            s,
            vec![
                (WaveformSignal::PpgRed, vec![1.0]),
                (WaveformSignal::PpgInfrared, vec![2.0]),
                (WaveformSignal::PpgAmbient, vec![16_777_215.0]),
            ]
        );
        assert!(decode_frame(&frame(0x04, 1, 0, T, &p[..p.len() - 1])).is_err());
    }

    #[test]
    fn f_acq_05_not_measured_sentinels_give_no_value() {
        let all_missing = le16s(&[i16::MIN, i16::MIN, i16::MIN]);
        let f = decode_frame(&frame(0x05, 1, 0, T, &all_missing)).unwrap();
        assert!(f.readings.is_empty() && f.heat_flux.is_none());
        let f = decode_frame(&frame(0x06, 1, 0, T, &[0xFF, 0xFF])).unwrap();
        assert!(f.readings.is_empty());
        let f = decode_frame(&frame(0x07, 1, 0, T, &[0xFF, 0, 0])).unwrap();
        assert!(f.readings.is_empty());
        assert_eq!(f.status.unwrap().battery_percent, None);
    }

    #[test]
    fn f_acq_10_status_frame_battery_and_contacts() {
        let f = decode_frame(&frame(0x07, 1, 0, T, &[87, 0b0000_0011, 0b0000_0001])).unwrap();
        assert_eq!(f.readings[0].parameter, Parameter::BatteryLevel);
        assert_eq!(f.readings[0].value, Some(RawValue::Number(87.0)));
        assert_eq!(f.readings[0].unit, Some(Unit::Percent));
        let st = f.status.unwrap();
        assert_eq!(
            st.contacts,
            vec![
                Contact {
                    channel: KitChannel::Ecg,
                    detected: true
                },
                Contact {
                    channel: KitChannel::RipThoracic,
                    detected: false
                },
            ]
        );
        assert!(
            decode_frame(&frame(0x07, 1, 0, T, &[101, 0, 0])).is_err(),
            "battery > 100"
        );
        assert!(decode_frame(&frame(0x07, 1, 0, T, &[50, 0b01, 0b10])).is_err());
        assert!(
            decode_frame(&frame(0x07, 1, 0b100, T, &[50, 0, 0])).is_err(),
            "sensor flag"
        );
    }

    #[test]
    fn f_acq_05_sr_017_status_flags_map_quality_flags_and_time() {
        let p = [0xF4, 0x01];
        let decode = |status: u8, time: u64| decode_frame(&frame(0x06, 1, status, time, &p));
        let demo = decode(0b1, T).unwrap();
        assert!(demo.flags.demonstration && !demo.flags.test_data);
        assert!(demo.readings[0].flags.demonstration);
        let test = decode(0b10, T).unwrap();
        assert!(test.flags.test_data && test.readings[0].flags.test_data);
        assert_eq!(
            decode(0b100, T).unwrap().readings[0].quality,
            Quality::Unusable
        );
        assert_eq!(
            decode(0b1000, T).unwrap().readings[0].quality,
            Quality::Poor
        );
        assert_eq!(decode(0b1100, T).unwrap().quality, Quality::Unusable);
        let unset = decode(0b1_0000, T).unwrap();
        assert_eq!(unset.device_time, None);
        assert_eq!(unset.readings[0].device_time, None);
        assert_eq!(
            decode(0, 0).unwrap().device_time,
            None,
            "timestamp 0: not set"
        );
        assert!(decode(0b10_0000, T).is_err(), "reserved bit");
        assert!(decode(0, u64::MAX).is_err(), "timestamp beyond i64");
    }

    #[test]
    fn f_acq_05_bad_crc_version_type_and_length_are_rejected() {
        let good = frame(0x06, 1, 0, T, &[0xF4, 0x01]);
        let mut bad_crc = good.clone();
        let last = bad_crc.len() - 1;
        bad_crc[last] ^= 1;
        assert_eq!(
            decode_frame(&bad_crc),
            Err(DecodeError::Invalid("CRC mismatch"))
        );
        let mut v2 = good.clone();
        v2[0] = 2;
        let crc = crc16_ccitt_false(&v2[..v2.len() - 2]);
        let n = v2.len();
        v2[n - 2..].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(
            decode_frame(&v2),
            Err(DecodeError::Invalid("protocol version"))
        );
        assert!(
            decode_frame(&frame(0x08, 1, 0, T, &[])).is_err(),
            "unknown type"
        );
        assert!(
            decode_frame(&frame(0x06, 1, 0, T, &[0xF4, 0x01, 0])).is_err(),
            "extra byte"
        );
        let long = frame(0x01, 1, 0, T, &vec![0; 230]);
        assert!(long.len() > MAX_FRAME_LEN);
        assert!(decode_frame(&long).is_err());
        // Every truncation (with or without a recomputed CRC) fails without panicking.
        for f in [
            good,
            frame(
                0x03,
                1,
                0,
                T,
                &[100, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
        ] {
            for n in 0..f.len() {
                assert!(decode_frame(&f[..n]).is_err(), "prefix {n}");
                let mut re = f[..n.saturating_sub(2)].to_vec();
                let crc = crc16_ccitt_false(&re);
                re.extend_from_slice(&crc.to_le_bytes());
                if re.len() < f.len() {
                    assert!(decode_frame(&re).is_err(), "re-CRC prefix {n}");
                }
            }
        }
    }

    #[test]
    fn f_acq_10_sequence_gaps_duplicates_and_wraparound() {
        let mut t = SequenceTracker::new();
        assert_eq!(t.observe(0xFFFD), SequenceEvent::First);
        assert_eq!(t.observe(0xFFFE), SequenceEvent::InOrder);
        assert_eq!(t.observe(0xFFFE), SequenceEvent::Duplicate);
        assert_eq!(t.observe(0xFFFF), SequenceEvent::InOrder);
        assert_eq!(t.observe(0x0000), SequenceEvent::InOrder);
        assert_eq!(t.observe(0x0004), SequenceEvent::Gap { missing: 3 });
        assert_eq!((t.wraps, t.dropped, t.duplicates, t.received), (1, 3, 1, 5));
        assert_eq!(t.observe(0xFFFE), SequenceEvent::Restart, "backwards");
    }

    #[test]
    fn f_acq_10_sequence_wrap_inside_a_gap_and_restart() {
        let mut t = SequenceTracker::new();
        t.observe(0xFFFE);
        assert_eq!(t.observe(0x0002), SequenceEvent::Gap { missing: 3 });
        assert_eq!((t.dropped, t.wraps), (3, 1));
        assert_eq!(t.observe(0x0001), SequenceEvent::Restart);
        assert_eq!(t.observe(0x0002), SequenceEvent::InOrder);
        assert_eq!(
            (t.received, t.dropped, t.duplicates, t.restarts),
            (4, 3, 0, 1)
        );
        t.reset();
        assert_eq!(t.observe(0x9000), SequenceEvent::First);
        assert_eq!(t.observe(0x1000), SequenceEvent::Restart, "half range back");
    }

    #[test]
    fn f_acq_05_info_round_trip_and_rejections() {
        let build = |version: u8, kind: u8, serial: &[u8; 16], cal: &[u8]| {
            let mut b = vec![version, kind, 1, 2];
            b.extend_from_slice(&3u16.to_le_bytes());
            b.extend_from_slice(serial);
            b.extend_from_slice(cal);
            let crc = crc16_ccitt_false(&b);
            b.extend_from_slice(&crc.to_le_bytes());
            b
        };
        let serial = *b"CSK-SH-000123\0\0\0";
        // One record: ECG gain 1.002 (1002 × 10^-3).
        let cal = [1, 1, 1, 0xEA, 0x03, 0x00, 0xFD];
        let info = decode_info(&build(1, 1, &serial, &cal)).unwrap();
        assert_eq!(info.kind, DeviceKind::Shirt);
        assert_eq!(info.firmware, (1, 2, 3));
        assert_eq!(info.serial, "CSK-SH-000123");
        assert_eq!(
            info.calibration,
            vec![Calibration {
                channel: KitChannel::Ecg,
                kind: CalibrationKind::Gain,
                value: 1.002
            }]
        );
        assert_eq!(
            decode_info(&build(1, 2, &serial, &[0])).unwrap().kind,
            DeviceKind::Wristband
        );
        assert!(decode_info(&build(2, 1, &serial, &[0])).is_err(), "version");
        assert!(decode_info(&build(1, 3, &serial, &[0])).is_err(), "kind");
        assert!(
            decode_info(&build(1, 1, &[0; 16], &[0])).is_err(),
            "empty serial"
        );
        assert!(decode_info(&build(1, 1, b"AB\0C\0\0\0\0\0\0\0\0\0\0\0\0", &[0])).is_err());
        assert!(
            decode_info(&build(1, 1, &serial, &[1, 11, 1, 0, 0, 0, 0])).is_err(),
            "channel"
        );
        assert!(
            decode_info(&build(1, 1, &serial, &[1, 1, 3, 0, 0, 0, 0])).is_err(),
            "cal kind"
        );
        let nan = [1, 1, 1, 0xFF, 0xFF, 0x7F, 0x00];
        assert!(
            decode_info(&build(1, 1, &serial, &nan)).is_err(),
            "special FLOAT"
        );
        assert!(
            decode_info(&build(1, 1, &serial, &[2, 1, 1, 0, 0, 0, 0])).is_err(),
            "count"
        );
        let good = build(1, 1, &serial, &cal);
        for n in 0..good.len() {
            assert!(decode_info(&good[..n]).is_err(), "prefix {n}");
        }
    }
}
