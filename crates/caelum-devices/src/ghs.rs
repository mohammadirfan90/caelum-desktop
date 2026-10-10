//! Bluetooth Generic Health Sensor Service 1.0 (GHSS): IEEE 11073-10206 ACOM observations carried
//! in the Live and Stored Health Observations characteristics (F-ACQ-03).
//!
//! Sans-I/O: [`Reassembler`] joins the segments of one observation, [`decode_observation`] turns the
//! observation body into catalog readings, waveforms and a list of observations that map to no
//! catalog parameter. Decoders only read; they never write to a device (R-SEC-07).
//!
//! Rules (CLAUDE.md rule 2):
//! - A field the device did not send stays empty. An unknown observation type is reported in
//!   [`GhsMeasurement::unmapped`], never coerced into a parameter; an unknown unit leaves the
//!   reading's unit empty (ingest then rejects it).
//! - Quality comes from the Measurement Status. Without a status the quality is `Degraded` ("not
//!   reported"), never `Good` by assumption. Test data is flagged (SR-017).
//! - A timestamp without an absolute, current UTC reference (tick counter, local time without an
//!   offset, previous timeline) yields no device time; the station's receipt time applies.
//! - Malformed observations (wrong length, truncated, reserved flags, TLVs, nested bundles) are
//!   errors with no partial output.
//!
//! This module also defines [`Waveform`] and [`WaveformSignal`], the waveform type shared with the
//! CAELUM sensor-kit decoder ([`crate::sensor_kit`]).

use caelum_domain::observation::{MeasurementMode, Quality};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::{LeapSeconds, TaiNs};
use caelum_domain::units::Unit;
use caelum_ingest::{DeviceFlags, RawValue};

use crate::sig::ieee::Reader;
use crate::sig::{DecodeError, Decoded};

/// Largest observation body: the Length field is a u16 covering the whole body.
pub const MAX_OBSERVATION_LEN: usize = u16::MAX as usize;

// ---------------------------------------------------------------------------------------------
// MDC codes (IEEE 11073-10101 nomenclature, partition << 16 | term code).
// Confirmed by the GHSS 1.0 specification examples (Appendix A):
//   MDC_SAT_O2_ART, MDC_PULS_OXIM_PULS_RATE, MDC_PRESS_BLD_NONINV(_SYS/_DIA/_MEAN),
//   MDC_ECG_ELEC_POTL_I, MDC_DIM_PER_CENT, MDC_DIM_BEAT_PER_MIN, MDC_DIM_MMHG, MDC_DIM_MILLI_VOLT,
//   MDC_MODALITY_SPOT.
// Taken from IEEE 11073-10101 and listed for verification in CR-013 before release: every other
// code below.
// ---------------------------------------------------------------------------------------------

/// MDC_ECG_ELEC_POTL: ECG, lead not specified (11073-10101; CR-013).
pub const MDC_ECG_ELEC_POTL: u32 = 0x0002_0100;
/// MDC_ECG_ELEC_POTL_I (GHSS example A.4).
pub const MDC_ECG_ELEC_POTL_I: u32 = 0x0002_0101;
/// MDC_ECG_ELEC_POTL_II (11073-10101; CR-013).
pub const MDC_ECG_ELEC_POTL_II: u32 = 0x0002_0102;
/// MDC_ECG_ELEC_POTL_V1; V2..V6 follow consecutively to 0x00020108 (11073-10101; CR-013).
pub const MDC_ECG_ELEC_POTL_V1: u32 = 0x0002_0103;
/// MDC_ECG_ELEC_POTL_III (11073-10101; CR-013).
pub const MDC_ECG_ELEC_POTL_III: u32 = 0x0002_013D;
/// MDC_ECG_ELEC_POTL_AVR (11073-10101; CR-013).
pub const MDC_ECG_ELEC_POTL_AVR: u32 = 0x0002_013E;
/// MDC_ECG_ELEC_POTL_AVL (11073-10101; CR-013).
pub const MDC_ECG_ELEC_POTL_AVL: u32 = 0x0002_013F;
/// MDC_ECG_ELEC_POTL_AVF (11073-10101; CR-013).
pub const MDC_ECG_ELEC_POTL_AVF: u32 = 0x0002_0140;
/// MDC_ECG_HEART_RATE (11073-10101; CR-013).
pub const MDC_ECG_HEART_RATE: u32 = 0x0002_4182;
/// MDC_PULS_OXIM_PULS_RATE (GHSS example A.5).
pub const MDC_PULS_OXIM_PULS_RATE: u32 = 0x0002_481A;
/// MDC_PULS_RATE_NON_INV (11073-10101; CR-013).
pub const MDC_PULS_RATE_NON_INV: u32 = 0x0002_482A;
/// MDC_PRESS_BLD_NONINV, the compound blood-pressure observation (GHSS example A.2).
pub const MDC_PRESS_BLD_NONINV: u32 = 0x0002_4A04;
/// MDC_PRESS_BLD_NONINV_SYS (GHSS example A.2).
pub const MDC_PRESS_BLD_NONINV_SYS: u32 = 0x0002_4A05;
/// MDC_PRESS_BLD_NONINV_DIA (GHSS example A.2).
pub const MDC_PRESS_BLD_NONINV_DIA: u32 = 0x0002_4A06;
/// MDC_PRESS_BLD_NONINV_MEAN (GHSS example A.2); no catalog parameter.
pub const MDC_PRESS_BLD_NONINV_MEAN: u32 = 0x0002_4A07;
/// MDC_SAT_O2_ART (GHSS example A.5).
pub const MDC_SAT_O2_ART: u32 = 0x0002_4BB8;
/// MDC_TEMP_BODY (11073-10101; CR-013).
pub const MDC_TEMP_BODY: u32 = 0x0002_4B5C;
/// MDC_RESP_RATE (11073-10101; CR-013).
pub const MDC_RESP_RATE: u32 = 0x0002_500A;
/// MDC_MASS_BODY_ACTUAL (11073-10101; CR-013).
pub const MDC_MASS_BODY_ACTUAL: u32 = 0x0002_E140;
/// MDC_MODALITY_SPOT supplemental information (GHSS example A.5).
pub const MDC_MODALITY_SPOT: u32 = 0x0002_4C3C;

/// MDC_DIM_PER_CENT (GHSS example A.5).
pub const MDC_DIM_PER_CENT: u16 = 0x0220;
/// MDC_DIM_KILO_G (11073-10101; CR-013).
pub const MDC_DIM_KILO_G: u16 = 0x06C3;
/// MDC_DIM_LB (11073-10101; CR-013).
pub const MDC_DIM_LB: u16 = 0x06E0;
/// MDC_DIM_MILLI_SEC (11073-10101; CR-013).
pub const MDC_DIM_MILLI_SEC: u16 = 0x0892;
/// MDC_DIM_BEAT_PER_MIN (GHSS example A.5).
pub const MDC_DIM_BEAT_PER_MIN: u16 = 0x0AA0;
/// MDC_DIM_RESP_PER_MIN (11073-10101; CR-013).
pub const MDC_DIM_RESP_PER_MIN: u16 = 0x0AE0;
/// MDC_DIM_KILO_PASCAL (11073-10101; CR-013).
pub const MDC_DIM_KILO_PASCAL: u16 = 0x0F03;
/// MDC_DIM_MMHG (GHSS example A.2).
pub const MDC_DIM_MMHG: u16 = 0x0F20;
/// MDC_DIM_MILLI_VOLT (GHSS example A.4).
pub const MDC_DIM_MILLI_VOLT: u16 = 0x10B2;
/// MDC_DIM_MICRO_VOLT (11073-10101; CR-013).
pub const MDC_DIM_MICRO_VOLT: u16 = 0x10B3;
/// MDC_DIM_FAHR (11073-10101; CR-013).
pub const MDC_DIM_FAHR: u16 = 0x1140;
/// MDC_DIM_DEGC (11073-10101; CR-013).
pub const MDC_DIM_DEGC: u16 = 0x17A0;

/// Observation Class Type values (GHSS 1.0).
mod class {
    pub const NUMERIC: u8 = 1;
    pub const SIMPLE_DISCRETE: u8 = 2;
    pub const STRING: u8 = 3;
    pub const SAMPLE_ARRAY: u8 = 4;
    pub const COMPOUND_DISCRETE_EVENT: u8 = 5;
    pub const COMPOUND_STATE_EVENT: u8 = 6;
    pub const COMPOUND: u8 = 7;
    pub const TLV_ENCODED: u8 = 8;
    pub const BUNDLE: u8 = 0xFF;
}

// ---------------------------------------------------------------------------------------------
// Shared waveform type.
// ---------------------------------------------------------------------------------------------

/// ECG lead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EcgLead {
    /// The device does not say which lead.
    Unspecified,
    /// Lead I.
    I,
    /// Lead II.
    II,
    /// Lead III.
    III,
    /// aVR.
    AVR,
    /// aVL.
    AVL,
    /// aVF.
    AVF,
    /// V1.
    V1,
    /// V2.
    V2,
    /// V3.
    V3,
    /// V4.
    V4,
    /// V5.
    V5,
    /// V6.
    V6,
}

/// What a waveform measures. Shared by the GHS and sensor-kit decoders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaveformSignal {
    /// ECG lead.
    Ecg(EcgLead),
    /// Respiratory inductance plethysmography, thoracic band.
    RipThoracic,
    /// Respiratory inductance plethysmography, abdominal band.
    RipAbdominal,
    /// Accelerometer X.
    AccelX,
    /// Accelerometer Y.
    AccelY,
    /// Accelerometer Z.
    AccelZ,
    /// Gyroscope X.
    GyroX,
    /// Gyroscope Y.
    GyroY,
    /// Gyroscope Z.
    GyroZ,
    /// PPG, red LED.
    PpgRed,
    /// PPG, infrared LED.
    PpgInfrared,
    /// PPG, green LED.
    PpgGreen,
    /// PPG, ambient light (LEDs off).
    PpgAmbient,
}

impl WaveformSignal {
    /// The `waveform_chunk.signal` name (e.g. `ecg_lead_ii`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Ecg(lead) => match lead {
                EcgLead::Unspecified => "ecg_lead_unspecified",
                EcgLead::I => "ecg_lead_i",
                EcgLead::II => "ecg_lead_ii",
                EcgLead::III => "ecg_lead_iii",
                EcgLead::AVR => "ecg_lead_avr",
                EcgLead::AVL => "ecg_lead_avl",
                EcgLead::AVF => "ecg_lead_avf",
                EcgLead::V1 => "ecg_lead_v1",
                EcgLead::V2 => "ecg_lead_v2",
                EcgLead::V3 => "ecg_lead_v3",
                EcgLead::V4 => "ecg_lead_v4",
                EcgLead::V5 => "ecg_lead_v5",
                EcgLead::V6 => "ecg_lead_v6",
            },
            Self::RipThoracic => "rip_thoracic",
            Self::RipAbdominal => "rip_abdominal",
            Self::AccelX => "accel_x",
            Self::AccelY => "accel_y",
            Self::AccelZ => "accel_z",
            Self::GyroX => "gyro_x",
            Self::GyroY => "gyro_y",
            Self::GyroZ => "gyro_z",
            Self::PpgRed => "ppg_red",
            Self::PpgInfrared => "ppg_infrared",
            Self::PpgGreen => "ppg_green",
            Self::PpgAmbient => "ppg_ambient",
        }
    }
}

/// Unit of a waveform's samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaveformUnit {
    /// Millivolt (ECG).
    Millivolt,
    /// Standard gravity (accelerometer).
    G,
    /// Degrees per second (gyroscope).
    DegreePerSecond,
    /// Raw, uncalibrated sensor counts (RIP, PPG): relative, no physical unit.
    Counts,
}

/// A block of evenly spaced samples from one signal, with the device's own quality and flags.
#[derive(Debug, Clone, PartialEq)]
pub struct Waveform {
    /// What the samples measure.
    pub signal: WaveformSignal,
    /// Time of the first sample on the device's clock; `None` if the device's time is unusable.
    pub start: Option<TaiNs>,
    /// Samples per second (finite, > 0).
    pub sample_rate_hz: f64,
    /// Unit of [`Self::samples`].
    pub unit: WaveformUnit,
    /// Samples, in time order.
    pub samples: Vec<f64>,
    /// Quality from the device's status.
    pub quality: Quality,
    /// Demonstration and test flags (SR-017).
    pub flags: DeviceFlags,
    /// MDC observation type the waveform came from (GHS only).
    pub source_code: Option<u32>,
}

// ---------------------------------------------------------------------------------------------
// Segmentation.
// ---------------------------------------------------------------------------------------------

/// Joins the segments of one Health Observation (GHSS Segmentation Header: bit 0 first segment,
/// bit 1 last segment, bits 2–7 rolling segment counter). Sans-I/O; one per characteristic.
///
/// Broken sequences are never stitched: a counter discontinuity, a continuation without a first
/// segment, a new first segment while an observation is incomplete, or a body above
/// [`MAX_OBSERVATION_LEN`] discards the partial body and returns an error (F-ACQ-03, F-ACQ-10).
#[derive(Debug, Default)]
pub struct Reassembler {
    body: Option<Vec<u8>>,
    counter: u8,
}

impl Reassembler {
    /// An idle reassembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether an observation is partly received.
    pub fn in_progress(&self) -> bool {
        self.body.is_some()
    }

    /// Drops any partial observation (e.g. on disconnect).
    pub fn reset(&mut self) {
        self.body = None;
    }

    /// Adds one characteristic value. Returns the complete observation body when its last segment
    /// arrives.
    ///
    /// A first segment that arrives while an observation is incomplete discards the incomplete
    /// observation, returns an error and starts a new observation from that segment; if that
    /// segment is also a last segment, it is discarded too (the device broke the sequence).
    pub fn push(&mut self, segment: &[u8]) -> Result<Option<Vec<u8>>, DecodeError> {
        let (&header, payload) = segment.split_first().ok_or(DecodeError::Truncated {
            field: "segmentation header",
            needed: 1,
            got: 0,
        })?;
        let first = header & 0x01 != 0;
        let last = header & 0x02 != 0;
        let counter = header >> 2;
        let mut body = if first {
            if self.body.take().is_some() {
                if !last && payload.len() <= MAX_OBSERVATION_LEN {
                    self.body = Some(payload.to_vec());
                    self.counter = counter;
                }
                return Err(DecodeError::Invalid(
                    "segment: first segment while an observation was incomplete",
                ));
            }
            Vec::new()
        } else {
            let Some(body) = self.body.take() else {
                return Err(DecodeError::Invalid(
                    "segment: continuation without a first segment",
                ));
            };
            if counter != (self.counter + 1) % 64 {
                return Err(DecodeError::Invalid("segment: counter discontinuity"));
            }
            body
        };
        if body.len() + payload.len() > MAX_OBSERVATION_LEN {
            return Err(DecodeError::Invalid("segment: observation too long"));
        }
        body.extend_from_slice(payload);
        if last {
            return Ok(Some(body));
        }
        self.counter = counter;
        self.body = Some(body);
        Ok(None)
    }
}

// ---------------------------------------------------------------------------------------------
// Observation decoding.
// ---------------------------------------------------------------------------------------------

/// An observation the decoder read but did not map to a catalog parameter or waveform; for the
/// caller's log. Never coerced into a parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct UnmappedObservation {
    /// Observation class type (or component value type).
    pub class: u8,
    /// MDC observation type (or component type), if sent.
    pub type_code: Option<u32>,
    /// MDC unit code, for numeric and sample-array values.
    pub unit_code: Option<u16>,
    /// Why it was not mapped.
    pub reason: &'static str,
}

/// Everything one Health Observation carried.
#[derive(Debug, Clone, PartialEq)]
pub struct GhsMeasurement {
    /// Catalog readings, in observation order.
    pub readings: Vec<Decoded>,
    /// ECG waveforms.
    pub waveforms: Vec<Waveform>,
    /// Observations with no catalog mapping.
    pub unmapped: Vec<UnmappedObservation>,
    /// `Spot` when the observation carries MDC_MODALITY_SPOT; `None` when no modality is given
    /// (the caller applies the device profile; nothing is assumed).
    pub mode: Option<MeasurementMode>,
    /// The device's patient field, if sent.
    pub patient: Option<u8>,
    /// Raw Time Sync Source Type of the first time stamp, for provenance (not interpreted).
    pub time_sync_source: Option<u8>,
}

/// Optional attributes of an observation (or of a bundle, common to its members). `None` means
/// the field was absent; a member inherits each bundle attribute it does not carry.
#[derive(Debug, Clone, Default)]
struct Attributes {
    type_code: Option<u32>,
    /// Outer `Some`: a time stamp was sent; inner: whether it gives a usable absolute time.
    time: Option<Option<TaiNs>>,
    sync_source: Option<u8>,
    status: Option<u16>,
    patient: Option<u8>,
    supplemental: Option<Vec<u32>>,
}

impl Attributes {
    fn inherit(self, parent: &Self) -> Self {
        Self {
            type_code: self.type_code.or(parent.type_code),
            time: self.time.or(parent.time),
            sync_source: self.sync_source.or(parent.sync_source),
            status: self.status.or(parent.status),
            patient: self.patient.or(parent.patient),
            supplemental: self.supplemental.or_else(|| parent.supplemental.clone()),
        }
    }

    fn device_time(&self) -> Option<TaiNs> {
        self.time.flatten()
    }

    fn spot(&self) -> bool {
        self.supplemental
            .as_ref()
            .is_some_and(|s| s.contains(&MDC_MODALITY_SPOT))
    }
}

/// Quality and flags from the Measurement Status.
struct Status {
    quality: Quality,
    flags: DeviceFlags,
    not_available: bool,
    setting: bool,
}

/// Measurement Status bits: 0 invalid, 1 questionable, 2 not available, 3 calibrating, 4 test
/// data, 5 early estimate, 6 manually entered, 7 setting, 14 threshold error, 15 thresholding
/// disabled. No status: `Degraded` (not reported).
fn status(raw: Option<u16>) -> Status {
    let Some(s) = raw else {
        return Status {
            quality: Quality::Degraded,
            flags: DeviceFlags::default(),
            not_available: false,
            setting: false,
        };
    };
    let bit = |n: u16| s & (1 << n) != 0;
    let quality = if bit(0) || bit(3) {
        Quality::Unusable
    } else if bit(1) {
        Quality::Poor
    } else if bit(5) {
        Quality::Degraded
    } else {
        Quality::Good
    };
    Status {
        quality,
        flags: DeviceFlags {
            demonstration: false,
            test_data: bit(4),
        },
        not_available: bit(2),
        setting: bit(7),
    }
}

/// Catalog parameter for an MDC observation type; `None` for anything not listed.
fn parameter_for(code: u32) -> Option<Parameter> {
    Some(match code {
        MDC_SAT_O2_ART => Parameter::Spo2,
        MDC_PULS_OXIM_PULS_RATE | MDC_PULS_RATE_NON_INV => Parameter::PulseRate,
        MDC_PRESS_BLD_NONINV_SYS => Parameter::BpSystolic,
        MDC_PRESS_BLD_NONINV_DIA => Parameter::BpDiastolic,
        MDC_ECG_HEART_RATE => Parameter::HeartRate,
        // GHS gives no thermometer site: never core_temp (site `unknown`, set by ingest).
        MDC_TEMP_BODY => Parameter::BodyTempMeasured,
        MDC_MASS_BODY_ACTUAL => Parameter::Weight,
        MDC_RESP_RATE => Parameter::RespirationRate,
        _ => return None,
    })
}

/// Domain unit for an MDC unit code; `None` for anything not listed (never assumed).
fn unit_for(code: u16) -> Option<Unit> {
    Some(match code {
        MDC_DIM_PER_CENT => Unit::Percent,
        MDC_DIM_BEAT_PER_MIN | MDC_DIM_RESP_PER_MIN => Unit::PerMinute,
        MDC_DIM_MMHG => Unit::MmHg,
        MDC_DIM_KILO_PASCAL => Unit::KiloPascal,
        MDC_DIM_DEGC => Unit::Celsius,
        MDC_DIM_FAHR => Unit::Fahrenheit,
        MDC_DIM_KILO_G => Unit::Kilogram,
        MDC_DIM_LB => Unit::Pound,
        MDC_DIM_MILLI_SEC => Unit::Millisecond,
        _ => return None,
    })
}

fn ecg_lead(code: u32) -> Option<EcgLead> {
    Some(match code {
        MDC_ECG_ELEC_POTL => EcgLead::Unspecified,
        MDC_ECG_ELEC_POTL_I => EcgLead::I,
        MDC_ECG_ELEC_POTL_II => EcgLead::II,
        0x0002_0103 => EcgLead::V1,
        0x0002_0104 => EcgLead::V2,
        0x0002_0105 => EcgLead::V3,
        0x0002_0106 => EcgLead::V4,
        0x0002_0107 => EcgLead::V5,
        0x0002_0108 => EcgLead::V6,
        MDC_ECG_ELEC_POTL_III => EcgLead::III,
        MDC_ECG_ELEC_POTL_AVR => EcgLead::AVR,
        MDC_ECG_ELEC_POTL_AVL => EcgLead::AVL,
        MDC_ECG_ELEC_POTL_AVF => EcgLead::AVF,
        _ => return None,
    })
}

/// Seconds from 1970-01-01 to 2000-01-01, the Elapsed Time epoch.
const UNIX_2000_S: i64 = 946_684_800;

/// GSS Elapsed Time (9 bytes): flags, uint48 time value, time sync source type, TZ/DST offset.
///
/// Flags: bit 0 tick counter (else time of day), bit 1 UTC (else local time), bits 2–3 resolution
/// (1 s, 100 ms, 1 ms, 100 µs), bit 4 TZ/DST offset used, bit 5 current timeline.
///
/// A usable time needs a time of day on the current timeline, in UTC or as local time with its
/// TZ/DST offset (15-minute units; -128 = not known). The UTC count is POSIX-style (no leap
/// seconds) since 2000-01-01T00:00:00 and is converted to TAI with the leap-second table.
fn elapsed_time(
    r: &mut Reader<'_>,
    leap: &LeapSeconds,
) -> Result<(Option<TaiNs>, u8), DecodeError> {
    let flags = r.u8("time stamp flags")?;
    let value = r.u48("time stamp value")?;
    let sync_source = r.u8("time sync source type")?;
    let tz = i8::from_le_bytes([r.u8("TZ/DST offset")?]);
    let tick_counter = flags & 0x01 != 0;
    let utc = flags & 0x02 != 0;
    let tz_used = flags & 0x10 != 0;
    let current = flags & 0x20 != 0;
    if tick_counter || !current {
        return Ok((None, sync_source));
    }
    let offset_s: i64 = if utc {
        0
    } else if tz_used && tz != i8::MIN {
        i64::from(tz) * 15 * 60
    } else {
        return Ok((None, sync_source)); // local time in an unknown zone
    };
    let resolution_ns: i64 = match (flags >> 2) & 0x03 {
        0 => 1_000_000_000,
        1 => 100_000_000,
        2 => 1_000_000,
        _ => 100_000,
    };
    let time = i64::try_from(value)
        .ok()
        .and_then(|v| v.checked_mul(resolution_ns))
        .and_then(|v| v.checked_add((UNIX_2000_S - offset_s).checked_mul(1_000_000_000)?))
        .and_then(|unix_ns| leap.utc_to_tai(unix_ns).ok())
        .map(|(tai, _validity)| tai);
    Ok((time, sync_source))
}

fn mdc_list(r: &mut Reader<'_>, field: &'static str) -> Result<Vec<u32>, DecodeError> {
    let count = r.u8(field)?;
    (0..count).map(|_| r.u32(field)).collect()
}

/// Reads the flags and the optional fields that follow them, in specification order.
fn attributes(r: &mut Reader<'_>, leap: &LeapSeconds) -> Result<Attributes, DecodeError> {
    let flags = r.u16("flags")?;
    let bit = |n: u16| flags & (1 << n) != 0;
    if flags & 0xFC00 != 0 {
        return Err(DecodeError::Invalid("reserved observation flags set"));
    }
    if bit(9) {
        // TLV fields: the layout of each TLV is not decoded; never guessed.
        return Err(DecodeError::Invalid("TLV fields (unsupported)"));
    }
    let mut a = Attributes::default();
    if bit(0) {
        a.type_code = Some(r.u32("observation type")?);
    }
    if bit(1) {
        let (time, sync) = elapsed_time(r, leap)?;
        a.time = Some(time);
        a.sync_source = Some(sync);
    }
    if bit(2) {
        r.float("measurement duration")?;
    }
    if bit(3) {
        a.status = Some(r.u16("measurement status")?);
    }
    if bit(4) {
        r.u32("observation id")?;
    }
    if bit(5) {
        a.patient = Some(r.u8("patient")?);
    }
    if bit(6) {
        a.supplemental = Some(mdc_list(r, "supplemental information")?);
    }
    if bit(7) {
        mdc_list(r, "derived from")?;
    }
    if bit(8) {
        mdc_list(r, "is member of")?;
    }
    Ok(a)
}

#[derive(Default)]
struct Collector {
    readings: Vec<Decoded>,
    waveforms: Vec<Waveform>,
    unmapped: Vec<UnmappedObservation>,
    spot: Vec<bool>,
    patient: Option<u8>,
    time_sync_source: Option<u8>,
}

impl Collector {
    fn unmapped(
        &mut self,
        class: u8,
        type_code: Option<u32>,
        unit: Option<u16>,
        reason: &'static str,
    ) {
        self.unmapped.push(UnmappedObservation {
            class,
            type_code,
            unit_code: unit,
            reason,
        });
    }

    fn observation(&mut self, attrs: &Attributes) -> Result<(), DecodeError> {
        self.spot.push(attrs.spot());
        if let Some(p) = attrs.patient {
            if self.patient.is_some_and(|q| q != p) {
                return Err(DecodeError::Invalid(
                    "bundle members name different patients",
                ));
            }
            self.patient = Some(p);
        }
        if self.time_sync_source.is_none() {
            self.time_sync_source = attrs.sync_source;
        }
        Ok(())
    }

    fn finish(self) -> Result<GhsMeasurement, DecodeError> {
        let mode = if self.spot.iter().all(|&s| s) && !self.spot.is_empty() {
            Some(MeasurementMode::Spot)
        } else if self.spot.iter().any(|&s| s) {
            return Err(DecodeError::Invalid(
                "bundle mixes spot and other modalities",
            ));
        } else {
            None
        };
        Ok(GhsMeasurement {
            readings: self.readings,
            waveforms: self.waveforms,
            unmapped: self.unmapped,
            mode,
            patient: self.patient,
            time_sync_source: self.time_sync_source,
        })
    }
}

/// Decodes one complete Health Observation body (after reassembly). The Length field must equal
/// the body length. Errors yield no partial output (F-ACQ-03).
pub fn decode_observation(body: &[u8], leap: &LeapSeconds) -> Result<GhsMeasurement, DecodeError> {
    let mut out = Collector::default();
    observation(body, leap, None, &mut out)?;
    out.finish()
}

fn observation(
    body: &[u8],
    leap: &LeapSeconds,
    bundle: Option<&Attributes>,
    out: &mut Collector,
) -> Result<(), DecodeError> {
    let mut r = Reader::new(body);
    let class = r.u8("observation class type")?;
    let length = r.u16("length")?;
    if usize::from(length) != body.len() {
        return Err(DecodeError::Invalid("observation length field"));
    }
    let own = attributes(&mut r, leap)?;
    if class == class::BUNDLE {
        if bundle.is_some() {
            return Err(DecodeError::Invalid("nested observation bundle"));
        }
        let count = r.u8("number of observations")?;
        for _ in 0..count {
            let rest = r.rest();
            let len = match (rest.get(1), rest.get(2)) {
                (Some(&lo), Some(&hi)) => usize::from(u16::from_le_bytes([lo, hi])),
                _ => {
                    return Err(DecodeError::Truncated {
                        field: "bundle member header",
                        needed: body.len() - rest.len() + 3,
                        got: body.len(),
                    })
                }
            };
            let member = r.bytes(len, "bundle member")?;
            observation(member, leap, Some(&own), out)?;
        }
    } else {
        let attrs = match bundle {
            Some(parent) => own.inherit(parent),
            None => own,
        };
        out.observation(&attrs)?;
        value(&mut r, class, attrs.type_code, &attrs, true, out)?;
    }
    if r.remaining() != 0 {
        return Err(DecodeError::Invalid("bytes after the observation value"));
    }
    Ok(())
}

/// Decodes one value of `class` (observation class or compound component value type).
fn value(
    r: &mut Reader<'_>,
    class: u8,
    type_code: Option<u32>,
    attrs: &Attributes,
    top_level: bool,
    out: &mut Collector,
) -> Result<(), DecodeError> {
    match class {
        class::NUMERIC => numeric(r, type_code, attrs, out),
        class::SIMPLE_DISCRETE => {
            r.u32("simple discrete value")?;
            out.unmapped(
                class,
                type_code,
                None,
                "discrete value: no catalog parameter",
            );
            Ok(())
        }
        class::STRING => {
            let len = r.u16("string length")?;
            let text = r.bytes(usize::from(len), "string value")?;
            std::str::from_utf8(text)
                .map_err(|_| DecodeError::Invalid("string value is not UTF-8"))?;
            out.unmapped(class, type_code, None, "string value: no catalog parameter");
            Ok(())
        }
        class::SAMPLE_ARRAY => sample_array(r, type_code, attrs, out),
        class::COMPOUND_DISCRETE_EVENT => {
            mdc_list(r, "compound discrete event")?;
            out.unmapped(
                class,
                type_code,
                None,
                "discrete events: no catalog parameter",
            );
            Ok(())
        }
        class::COMPOUND_STATE_EVENT => {
            let size = usize::from(r.u8("compound state/event size")?);
            r.skip(3 * size, "compound state/event value")?;
            out.unmapped(
                class,
                type_code,
                None,
                "state/event value: no catalog parameter",
            );
            Ok(())
        }
        class::COMPOUND if top_level => {
            let count = r.u8("number of components")?;
            for _ in 0..count {
                let component = r.u32("component type")?;
                let value_type = r.u8("component value type")?;
                if !(class::NUMERIC..=class::COMPOUND_STATE_EVENT).contains(&value_type) {
                    return Err(DecodeError::Invalid("component value type"));
                }
                value(r, value_type, Some(component), attrs, false, out)?;
            }
            Ok(())
        }
        class::TLV_ENCODED => Err(DecodeError::Invalid(
            "TLV-encoded observation (unsupported)",
        )),
        _ => Err(DecodeError::Invalid("observation class type")),
    }
}

fn numeric(
    r: &mut Reader<'_>,
    type_code: Option<u32>,
    attrs: &Attributes,
    out: &mut Collector,
) -> Result<(), DecodeError> {
    let unit_code = r.u16("unit code")?;
    let number = r.float("numeric value")?;
    let st = status(attrs.status);
    let Some(parameter) = type_code.and_then(parameter_for) else {
        out.unmapped(
            class::NUMERIC,
            type_code,
            Some(unit_code),
            "no catalog parameter",
        );
        return Ok(());
    };
    if st.setting {
        out.unmapped(
            class::NUMERIC,
            type_code,
            Some(unit_code),
            "device setting, not a measurement",
        );
        return Ok(());
    }
    let number = if st.not_available { None } else { number };
    out.readings.push(Decoded {
        parameter,
        value: number.map(RawValue::Number),
        unit: unit_for(unit_code),
        device_time: attrs.device_time(),
        quality: st.quality,
        flags: st.flags,
        site: None,
    });
    Ok(())
}

fn finite_float(r: &mut Reader<'_>, field: &'static str) -> Result<f64, DecodeError> {
    r.float(field)?
        .filter(|v| v.is_finite())
        .ok_or(DecodeError::Invalid(field))
}

/// Sample Array: unit code, scale factor, offset, sample period (s), samples per period, bytes per
/// sample (1, 2 or 4), number of samples, then unsigned little-endian scaled samples. Physical
/// value = scaled × scale factor + offset; sample rate = samples per period / sample period.
fn sample_array(
    r: &mut Reader<'_>,
    type_code: Option<u32>,
    attrs: &Attributes,
    out: &mut Collector,
) -> Result<(), DecodeError> {
    let unit_code = r.u16("unit code")?;
    let scale = finite_float(r, "sample array scale factor")?;
    let offset = finite_float(r, "sample array offset")?;
    let period = finite_float(r, "sample period")?;
    if period <= 0.0 {
        return Err(DecodeError::Invalid("sample period"));
    }
    let per_period = r.u8("samples per period")?;
    if per_period == 0 {
        return Err(DecodeError::Invalid("samples per period"));
    }
    let width = r.u8("bytes per sample")?;
    if !matches!(width, 1 | 2 | 4) {
        return Err(DecodeError::Invalid("bytes per sample"));
    }
    let count = usize::try_from(r.u32("number of samples")?)
        .map_err(|_| DecodeError::Invalid("number of samples"))?;
    let len = count
        .checked_mul(usize::from(width))
        .ok_or(DecodeError::Invalid("number of samples"))?;
    let raw = r.bytes(len, "samples")?;
    let rate = f64::from(per_period) / period;
    if !rate.is_finite() || rate <= 0.0 {
        return Err(DecodeError::Invalid("sample rate"));
    }
    let st = status(attrs.status);
    let unmapped = |out: &mut Collector, reason: &'static str| -> Result<(), DecodeError> {
        out.unmapped(class::SAMPLE_ARRAY, type_code, Some(unit_code), reason);
        Ok(())
    };
    let Some(lead) = type_code.and_then(ecg_lead) else {
        return unmapped(out, "no waveform mapping");
    };
    let to_mv = match unit_code {
        MDC_DIM_MILLI_VOLT => 1.0,
        MDC_DIM_MICRO_VOLT => 1e-3,
        _ => return unmapped(out, "unsupported waveform unit"),
    };
    if st.setting {
        return unmapped(out, "device setting, not a measurement");
    }
    if st.not_available {
        return unmapped(out, "device reports measurement not available");
    }
    if count == 0 {
        return unmapped(out, "no samples");
    }
    let samples = raw
        .chunks_exact(usize::from(width))
        .map(|c| {
            // Unsigned little-endian of 1, 2 or 4 bytes.
            let scaled = c
                .iter()
                .rev()
                .fold(0u32, |acc, &b| (acc << 8) | u32::from(b));
            (f64::from(scaled) * scale + offset) * to_mv
        })
        .collect();
    out.waveforms.push(Waveform {
        signal: WaveformSignal::Ecg(lead),
        start: attrs.device_time(),
        sample_rate_hz: rate,
        unit: WaveformUnit::Millivolt,
        samples,
        quality: st.quality,
        flags: st.flags,
        source_code: type_code,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leap() -> LeapSeconds {
        LeapSeconds::bundled().unwrap()
    }

    /// Builds an observation body: class, computed length, flags, then `rest`.
    fn body(class: u8, flags: u16, rest: &[u8]) -> Vec<u8> {
        let len = u16::try_from(5 + rest.len()).unwrap();
        let mut b = vec![class];
        b.extend_from_slice(&len.to_le_bytes());
        b.extend_from_slice(&flags.to_le_bytes());
        b.extend_from_slice(rest);
        b
    }

    fn le32(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    /// FLOAT m × 10^e.
    fn flt(m: i32, e: i8) -> [u8; 4] {
        let raw = (u32::from(e.to_le_bytes()[0]) << 24) | (m as u32 & 0x00FF_FFFF);
        raw.to_le_bytes()
    }

    fn numeric_body(flags: u16, opts: &[u8], unit: u16, value: [u8; 4]) -> Vec<u8> {
        let mut rest = opts.to_vec();
        rest.extend_from_slice(&unit.to_le_bytes());
        rest.extend_from_slice(&value);
        body(class::NUMERIC, flags, &rest)
    }

    const SPEC_TIME: [u8; 9] = [0x22, 0x72, 0x9D, 0x2B, 0x29, 0x00, 0x00, 0x06, 0x00];

    fn spec_time_tai() -> TaiNs {
        let unix = (UNIX_2000_S + 0x292B_9D72) * 1_000_000_000;
        leap().utc_to_tai(unix).unwrap().0
    }

    /// GHSS example A.4: ECG lead I sample array. The specification lists Length 151 with 116
    /// two-byte samples, which cannot add up (38 header bytes + 232 sample bytes = 270); this
    /// fixture keeps every field of the example and uses the consistent length 270. The scale
    /// factor is built from first principles (1612 × 10^-3: LE 4C 06 00 FD).
    fn spec_a4() -> Vec<u8> {
        let mut rest = Vec::new();
        rest.extend_from_slice(&[0x01, 0x01, 0x02, 0x00]); // MDC_ECG_ELEC_POTL_I
        rest.extend_from_slice(&SPEC_TIME);
        rest.extend_from_slice(&[0xB2, 0x10]); // MDC_DIM_MILLI_VOLT
        rest.extend_from_slice(&[0x4C, 0x06, 0x00, 0xFD]); // scale 1.612
        rest.extend_from_slice(&[0x00, 0x08, 0x00, 0x00]); // offset 2048
        rest.extend_from_slice(&[0x0A, 0x00, 0x00, 0xFD]); // period 0.010 s
        rest.extend_from_slice(&[0x01, 0x02]); // 1 sample per period, 2 bytes per sample
        rest.extend_from_slice(&[0x74, 0x00, 0x00, 0x00]); // 116 samples
        for i in 0..116u16 {
            rest.extend_from_slice(&(2000 + i).to_le_bytes());
        }
        body(class::SAMPLE_ARRAY, 0x0003, &rest)
    }

    /// GHSS example A.2: blood pressure as a compound observation (type, observation id
    /// 123456, SYS/DIA/MEAN 100/60/80 mmHg). The brief's Length 53 could not be reproduced with
    /// the documented component layout (type u32, value type u8, unit u16, FLOAT); this fixture
    /// is self-consistent (47 bytes).
    fn spec_a2() -> Vec<u8> {
        let mut rest = Vec::new();
        rest.extend_from_slice(&le32(MDC_PRESS_BLD_NONINV));
        rest.extend_from_slice(&le32(123_456));
        rest.push(3);
        for (code, v) in [
            (MDC_PRESS_BLD_NONINV_SYS, 100),
            (MDC_PRESS_BLD_NONINV_DIA, 60),
            (MDC_PRESS_BLD_NONINV_MEAN, 80),
        ] {
            rest.extend_from_slice(&le32(code));
            rest.push(class::NUMERIC);
            rest.extend_from_slice(&MDC_DIM_MMHG.to_le_bytes());
            rest.extend_from_slice(&flt(v, 0));
        }
        body(class::COMPOUND, 0x0011, &rest)
    }

    /// GHSS example A.5: bundle (time stamp + MDC_MODALITY_SPOT) with SpO2 98 % and pulse rate
    /// 98 /min, each member 15 bytes. The brief's bundle Length 53 does not add up with two
    /// 15-byte members (20 + 30 = 50); this fixture is self-consistent.
    fn spec_a5() -> Vec<u8> {
        let spo2 = numeric_body(
            0x0001,
            &le32(MDC_SAT_O2_ART),
            MDC_DIM_PER_CENT,
            [0x62, 0, 0, 0],
        );
        let pr = numeric_body(
            0x0001,
            &le32(MDC_PULS_OXIM_PULS_RATE),
            MDC_DIM_BEAT_PER_MIN,
            [0x62, 0, 0, 0],
        );
        assert_eq!(spo2.len(), 15);
        let mut rest = SPEC_TIME.to_vec();
        rest.push(1);
        rest.extend_from_slice(&le32(MDC_MODALITY_SPOT));
        rest.push(2);
        rest.extend_from_slice(&spo2);
        rest.extend_from_slice(&pr);
        body(class::BUNDLE, 0x0042, &rest)
    }

    #[test]
    fn f_acq_03_spec_a4_ecg_sample_array() {
        let b = spec_a4();
        assert_eq!(b.len(), 270);
        let m = decode_observation(&b, &leap()).unwrap();
        assert!(m.readings.is_empty() && m.unmapped.is_empty());
        let w = &m.waveforms[0];
        assert_eq!(w.signal, WaveformSignal::Ecg(EcgLead::I));
        assert_eq!(w.signal.name(), "ecg_lead_i");
        assert_eq!(w.unit, WaveformUnit::Millivolt);
        assert!((w.sample_rate_hz - 100.0).abs() < 1e-9);
        assert_eq!(w.samples.len(), 116);
        assert!((w.samples[0] - (2000.0 * 1.612 + 2048.0)).abs() < 1e-9);
        assert!((w.samples[115] - (2115.0 * 1.612 + 2048.0)).abs() < 1e-9);
        assert_eq!(w.start, Some(spec_time_tai()));
        assert_eq!(w.quality, Quality::Degraded, "no status: not reported");
        assert_eq!(w.source_code, Some(MDC_ECG_ELEC_POTL_I));
        assert_eq!(m.time_sync_source, Some(6));
        assert_eq!(m.mode, None);
    }

    #[test]
    fn f_acq_03_spec_a2_blood_pressure_compound() {
        let m = decode_observation(&spec_a2(), &leap()).unwrap();
        let got: Vec<_> = m
            .readings
            .iter()
            .map(|d| (d.parameter, d.value.clone(), d.unit))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    Parameter::BpSystolic,
                    Some(RawValue::Number(100.0)),
                    Some(Unit::MmHg)
                ),
                (
                    Parameter::BpDiastolic,
                    Some(RawValue::Number(60.0)),
                    Some(Unit::MmHg)
                ),
            ]
        );
        assert_eq!(m.unmapped.len(), 1);
        assert_eq!(m.unmapped[0].type_code, Some(MDC_PRESS_BLD_NONINV_MEAN));
        assert!(m.readings.iter().all(|d| d.device_time.is_none()));
    }

    #[test]
    fn f_acq_03_spec_a5_bundle_inherits_time_and_spot_modality() {
        let m = decode_observation(&spec_a5(), &leap()).unwrap();
        assert_eq!(m.mode, Some(MeasurementMode::Spot));
        assert_eq!(m.readings.len(), 2);
        assert_eq!(m.readings[0].parameter, Parameter::Spo2);
        assert_eq!(m.readings[0].unit, Some(Unit::Percent));
        assert_eq!(m.readings[1].parameter, Parameter::PulseRate);
        assert_eq!(m.readings[1].unit, Some(Unit::PerMinute));
        for d in &m.readings {
            assert_eq!(d.value, Some(RawValue::Number(98.0)));
            assert_eq!(d.device_time, Some(spec_time_tai()));
        }
    }

    #[test]
    fn f_acq_03_sr_017_measurement_status_maps_quality_and_test_flag() {
        let opts = |s: u16| {
            let mut o = le32(MDC_SAT_O2_ART).to_vec();
            o.extend_from_slice(&s.to_le_bytes());
            o
        };
        let decode = |s: u16| {
            let b = numeric_body(0x0009, &opts(s), MDC_DIM_PER_CENT, flt(97, 0));
            decode_observation(&b, &leap()).unwrap().readings.remove(0)
        };
        assert_eq!(decode(0).quality, Quality::Good);
        assert_eq!(decode(1 << 0).quality, Quality::Unusable);
        assert_eq!(decode(1 << 3).quality, Quality::Unusable);
        assert_eq!(decode(1 << 1).quality, Quality::Poor);
        assert_eq!(decode(1 << 5).quality, Quality::Degraded);
        assert_eq!(decode(1 << 2).value, None, "not available");
        let test = decode(1 << 4);
        assert!(test.flags.test_data && !test.flags.demonstration);
        let setting = numeric_body(0x0009, &opts(1 << 7), MDC_DIM_PER_CENT, flt(97, 0));
        let m = decode_observation(&setting, &leap()).unwrap();
        assert!(m.readings.is_empty() && m.unmapped.len() == 1);
    }

    #[test]
    fn f_acq_03_special_float_unknown_unit_and_unknown_type() {
        let nan = numeric_body(
            0x0001,
            &le32(MDC_SAT_O2_ART),
            MDC_DIM_PER_CENT,
            [0xFF, 0xFF, 0x7F, 0x00],
        );
        assert_eq!(
            decode_observation(&nan, &leap()).unwrap().readings[0].value,
            None
        );
        let unit = numeric_body(0x0001, &le32(MDC_SAT_O2_ART), 0x1234, flt(97, 0));
        assert_eq!(
            decode_observation(&unit, &leap()).unwrap().readings[0].unit,
            None
        );
        let unknown = numeric_body(0x0001, &le32(0x0002_9999), MDC_DIM_PER_CENT, flt(97, 0));
        let m = decode_observation(&unknown, &leap()).unwrap();
        assert!(m.readings.is_empty());
        assert_eq!(m.unmapped[0].type_code, Some(0x0002_9999));
        let no_type = numeric_body(0x0000, &[], MDC_DIM_PER_CENT, flt(97, 0));
        assert_eq!(
            decode_observation(&no_type, &leap()).unwrap().unmapped[0].type_code,
            None
        );
    }

    #[test]
    fn f_acq_03_body_temperature_has_no_site_and_is_never_core_temp() {
        let b = numeric_body(0x0001, &le32(MDC_TEMP_BODY), MDC_DIM_DEGC, flt(370, -1));
        let d = decode_observation(&b, &leap()).unwrap().readings.remove(0);
        assert_eq!(d.parameter, Parameter::BodyTempMeasured);
        assert_eq!(d.site, None);
        assert_eq!(d.unit, Some(Unit::Celsius));
    }

    #[test]
    fn f_acq_03_truncated_bodies_are_errors_not_panics() {
        for fixture in [spec_a2(), spec_a4(), spec_a5()] {
            for n in 0..fixture.len() {
                assert!(
                    decode_observation(&fixture[..n], &leap()).is_err(),
                    "prefix {n}"
                );
            }
            // Byte flips never panic; with a correct length field they may decode or fail.
            for i in 0..fixture.len() {
                for flip in [0x01u8, 0x80, 0xFF] {
                    let mut f = fixture.clone();
                    f[i] ^= flip;
                    let _ = decode_observation(&f, &leap());
                }
            }
        }
    }

    #[test]
    fn f_acq_03_length_mismatch_and_trailing_bytes_are_errors() {
        let mut b = spec_a2();
        b.push(0);
        assert!(
            decode_observation(&b, &leap()).is_err(),
            "length field too small"
        );
        let mut c = spec_a2();
        c[1] = c[1].wrapping_add(1);
        c.push(0);
        assert_eq!(
            decode_observation(&c, &leap()),
            Err(DecodeError::Invalid("bytes after the observation value"))
        );
    }

    #[test]
    fn f_acq_03_nested_bundle_reserved_flags_and_tlv_are_errors() {
        let mut rest = vec![1];
        rest.extend_from_slice(&spec_a5());
        let nested = body(class::BUNDLE, 0x0000, &rest);
        assert_eq!(
            decode_observation(&nested, &leap()),
            Err(DecodeError::Invalid("nested observation bundle"))
        );
        let reserved = numeric_body(0x0400, &[], MDC_DIM_PER_CENT, flt(97, 0));
        assert!(decode_observation(&reserved, &leap()).is_err());
        let tlv = numeric_body(0x0200, &[0], MDC_DIM_PER_CENT, flt(97, 0));
        assert!(decode_observation(&tlv, &leap()).is_err());
        assert!(decode_observation(&body(class::TLV_ENCODED, 0, &[]), &leap()).is_err());
        assert!(decode_observation(&body(9, 0, &[]), &leap()).is_err());
    }

    #[test]
    fn f_acq_03_unusable_timestamps_give_no_device_time() {
        let with_time = |t: [u8; 9]| {
            let mut o = le32(MDC_SAT_O2_ART).to_vec();
            o.extend_from_slice(&t);
            let b = numeric_body(0x0003, &o, MDC_DIM_PER_CENT, flt(97, 0));
            decode_observation(&b, &leap()).unwrap().readings[0].device_time
        };
        let tick = [0x21, 1, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(with_time(tick), None, "tick counter");
        let local = [0x20, 0x72, 0x9D, 0x2B, 0x29, 0, 0, 6, 0];
        assert_eq!(with_time(local), None, "local time, zone unknown");
        let old_timeline = [0x02, 0x72, 0x9D, 0x2B, 0x29, 0, 0, 6, 0];
        assert_eq!(with_time(old_timeline), None, "previous timeline");
        let unknown_tz = [0x30, 0x72, 0x9D, 0x2B, 0x29, 0, 0, 6, 0x80];
        assert_eq!(with_time(unknown_tz), None, "TZ offset not known");
        // Local time UTC+1 h (4 × 15 min) with the offset given: one hour earlier in UTC.
        let local_tz = [0x30, 0x72, 0x9D, 0x2B, 0x29, 0, 0, 6, 4];
        assert_eq!(
            with_time(local_tz),
            Some(TaiNs(spec_time_tai().0 - 3_600_000_000_000))
        );
        // 1 ms resolution.
        let ms = [0x2A, 0xE8, 0x03, 0, 0, 0, 0, 6, 0];
        let expected = leap()
            .utc_to_tai((UNIX_2000_S + 1) * 1_000_000_000)
            .unwrap()
            .0;
        assert_eq!(with_time(ms), Some(expected));
        assert_eq!(with_time(SPEC_TIME), Some(spec_time_tai()));
    }

    #[test]
    fn f_acq_03_sample_array_rejects_bad_parameters() {
        let array =
            |scale: [u8; 4], period: [u8; 4], per: u8, width: u8, count: u32, data: &[u8]| {
                let mut rest = le32(MDC_ECG_ELEC_POTL_II).to_vec();
                rest.extend_from_slice(&MDC_DIM_MICRO_VOLT.to_le_bytes());
                rest.extend_from_slice(&scale);
                rest.extend_from_slice(&flt(0, 0));
                rest.extend_from_slice(&period);
                rest.push(per);
                rest.push(width);
                rest.extend_from_slice(&count.to_le_bytes());
                rest.extend_from_slice(data);
                decode_observation(&body(class::SAMPLE_ARRAY, 0x0001, &rest), &leap())
            };
        let one = flt(1, 0);
        let ms4 = flt(4, -3);
        let ok = array(one, ms4, 1, 4, 2, &[0xE8, 0x03, 0, 0, 0xD0, 0x07, 0, 0]).unwrap();
        let w = &ok.waveforms[0];
        assert_eq!(w.signal, WaveformSignal::Ecg(EcgLead::II));
        assert_eq!(w.samples, vec![1.0, 2.0], "µV converted to mV");
        assert!((w.sample_rate_hz - 250.0).abs() < 1e-9);
        assert!(
            array([0xFF, 0xFF, 0x7F, 0x00], ms4, 1, 1, 1, &[1]).is_err(),
            "NaN scale"
        );
        assert!(array(one, flt(0, 0), 1, 1, 1, &[1]).is_err(), "zero period");
        assert!(
            array(one, flt(-1, 0), 1, 1, 1, &[1]).is_err(),
            "negative period"
        );
        assert!(
            array(one, ms4, 0, 1, 1, &[1]).is_err(),
            "no samples per period"
        );
        assert!(
            array(one, ms4, 1, 3, 1, &[1, 2, 3]).is_err(),
            "3 bytes per sample"
        );
        assert!(
            array(one, ms4, 1, 2, 2, &[1, 2, 3]).is_err(),
            "count/bytes mismatch"
        );
        assert!(array(one, ms4, 1, 1, 1, &[1, 2]).is_err(), "extra bytes");
        assert!(array(one, ms4, 1, 4, u32::MAX, &[]).is_err(), "huge count");
    }

    #[test]
    fn f_acq_03_other_value_classes_are_decoded_but_unmapped() {
        let mut s = le32(0x0002_1111).to_vec();
        s.extend_from_slice(&3u16.to_le_bytes());
        s.extend_from_slice(b"abc");
        let m = decode_observation(&body(class::STRING, 0x0001, &s), &leap()).unwrap();
        assert_eq!(m.unmapped[0].class, class::STRING);
        let bad = [&le32(1)[..], &2u16.to_le_bytes(), &[0xFF, 0xFE]].concat();
        assert!(decode_observation(&body(class::STRING, 0x0001, &bad), &leap()).is_err());
        let d = decode_observation(&body(class::SIMPLE_DISCRETE, 0, &le32(7)), &leap()).unwrap();
        assert_eq!(d.unmapped[0].class, class::SIMPLE_DISCRETE);
        let e = decode_observation(
            &body(class::COMPOUND_DISCRETE_EVENT, 0, &[1, 1, 2, 3, 4]),
            &leap(),
        )
        .unwrap();
        assert_eq!(e.unmapped.len(), 1);
        let se = decode_observation(
            &body(class::COMPOUND_STATE_EVENT, 0, &[2, 1, 1, 2, 2, 3, 3]),
            &leap(),
        )
        .unwrap();
        assert_eq!(se.unmapped.len(), 1);
        // A compound component cannot itself be a compound observation.
        let comp = [&[1][..], &le32(1), &[class::COMPOUND, 0]].concat();
        assert!(decode_observation(&body(class::COMPOUND, 0, &comp), &leap()).is_err());
    }

    #[test]
    fn f_acq_03_segments_reassemble_in_order_with_counter_rollover() {
        let obs = spec_a4();
        let chunks: Vec<&[u8]> = obs.chunks(100).collect();
        assert_eq!(chunks.len(), 3);
        let mut r = Reassembler::new();
        let mut counter = 62u8; // rolls over 63 → 0
        let mut out = None;
        for (i, c) in chunks.iter().enumerate() {
            let mut header = counter << 2;
            if i == 0 {
                header |= 1;
            }
            if i == chunks.len() - 1 {
                header |= 2;
            }
            let seg = [&[header][..], c].concat();
            out = r.push(&seg).unwrap();
            counter = (counter + 1) % 64;
        }
        assert_eq!(out.as_deref(), Some(&obs[..]));
        assert!(!r.in_progress());
        assert_eq!(
            r.push(&[0x03, 9, 9]).unwrap(),
            Some(vec![9, 9]),
            "single segment"
        );
    }

    #[test]
    fn f_acq_03_broken_segment_sequences_are_discarded() {
        let mut r = Reassembler::new();
        assert!(r.push(&[]).is_err());
        assert!(r.push(&[0x04, 1]).is_err(), "continuation without first");
        r.push(&[0x01, 1]).unwrap();
        assert!(r.push(&[0x0C, 2]).is_err(), "counter 0 → 3");
        assert!(!r.in_progress(), "partial body discarded");
        r.push(&[0x01, 1]).unwrap();
        assert!(r.push(&[0x05, 2]).is_err(), "first while in progress");
        assert_eq!(
            r.push(&[0x0A, 3]).unwrap(),
            Some(vec![2, 3]),
            "new body started"
        );
        let mut big = Reassembler::new();
        let seg = [&[0x01][..], &vec![0u8; 40_000]].concat();
        big.push(&seg).unwrap();
        let next = [&[0x04][..], &vec![0u8; 40_000]].concat();
        assert!(big.push(&next).is_err(), "over 65535 bytes");
        assert!(!big.in_progress());
    }

    #[test]
    fn f_acq_03_bundle_with_conflicting_patients_or_mixed_modality_is_rejected() {
        let member = |patient: u8| {
            let o = [&le32(MDC_SAT_O2_ART)[..], &[patient]].concat();
            numeric_body(0x0021, &o, MDC_DIM_PER_CENT, flt(97, 0))
        };
        let rest = [&[2][..], &member(1), &member(2)].concat();
        assert!(decode_observation(&body(class::BUNDLE, 0, &rest), &leap()).is_err());
        let spot = [&le32(MDC_SAT_O2_ART)[..], &[1], &le32(MDC_MODALITY_SPOT)].concat();
        let spot_member = numeric_body(0x0041, &spot, MDC_DIM_PER_CENT, flt(97, 0));
        let plain = numeric_body(0x0001, &le32(MDC_SAT_O2_ART), MDC_DIM_PER_CENT, flt(97, 0));
        let mixed = [&[2][..], &spot_member, &plain].concat();
        assert!(decode_observation(&body(class::BUNDLE, 0, &mixed), &leap()).is_err());
        let same = [&[2][..], &member(1), &member(1)].concat();
        let ok = decode_observation(&body(class::BUNDLE, 0, &same), &leap()).unwrap();
        assert_eq!(ok.patient, Some(1));
    }
}
