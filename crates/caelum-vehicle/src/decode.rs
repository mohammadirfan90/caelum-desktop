//! The sans-I/O decoder: datagram in, catalog readings out (F-ENV-01, F-ENV-02, F-ENV-03,
//! F-GND-11 subscribe side, R-INT-04).
//!
//! Each datagram is checked against the source allow-list, unframed, parsed, matched to a
//! configured MID and decoded field by field. Values are calibrated, converted to the catalog's
//! canonical unit and checked against the catalog's plausible range; state codes map to the
//! CR-015 vocabulary. A packet-level fault drops the whole packet; a field-level fault drops only
//! that field. Every drop is counted by reason (SR-004, F-ACQ-10); nothing panics, nothing is
//! defaulted, nothing is repeated.

use std::collections::BTreeMap;
use std::net::IpAddr;

use caelum_domain::ids::CrewMemberId;
use caelum_domain::observation::{MeasurementMode, Quality};
use caelum_domain::parameters::{Parameter, ValueType};
use caelum_domain::time::{LeapSeconds, MetEpoch, MetNs, TaiNs};
use caelum_domain::units::{convert, Substance, Unit};
use serde::Serialize;

use crate::config::{
    ByteOrder, Conversion, FieldDef, Flag, MessageKind, Scalar, ScalarType, VehicleConfig,
};
use crate::packet::{self, PacketError, TlmPacket};
use crate::time::{self, CorrelationStatus, Correlator, TimeSource};

/// Who a reading is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Subject {
    /// Cabin-wide (atmosphere, area dosimeter, SPE state), with the configured area label.
    Cabin {
        /// Area label from the MID table, if configured.
        location: Option<String>,
    },
    /// One crew member (personal dosimeter, exercise session), mapped from the vehicle crew ID.
    Crew(CrewMemberId),
}

/// A value in the catalog's canonical form.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VehicleValue {
    /// A number in the parameter's canonical unit.
    Number(f64),
    /// One of the CR-015 states.
    State(&'static str),
}

/// The calibrated value before unit conversion (provenance).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Original {
    /// Value after calibration, in the configured unit.
    pub value: f64,
    /// UCUM code of that unit.
    pub unit: &'static str,
}

/// One normalised vehicle reading, ready for the store (classification `measured`, source
/// `vehicle`) and the rule engine.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VehicleReading {
    /// Source MID.
    pub mid: u16,
    /// Message name from the MID table.
    pub message: String,
    /// Catalog parameter.
    pub parameter: Parameter,
    /// Cabin or crew member.
    pub subject: Subject,
    /// Canonical value.
    pub value: VehicleValue,
    /// The parameter's canonical unit.
    pub unit: Unit,
    /// Calibrated value before conversion (numeric fields).
    pub original: Option<Original>,
    /// Measurement time (TAI).
    pub measured_at: TaiNs,
    /// MET of `measured_at`, from the signed mission epoch.
    pub met: MetNs,
    /// Station receipt time (TAI).
    pub received_at: TaiNs,
    /// Where `measured_at` came from.
    pub time: TimeSource,
    /// GOOD when the vehicle's validity bit for the value is set; DEGRADED when the message
    /// has no validity bit for it (CR-015 §5). A cleared bit drops the value.
    pub quality: Quality,
    /// Continuous (environment, radiation) or spot (one exercise session summary).
    pub mode: MeasurementMode,
}

/// Why a packet or a field was dropped.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Rejection {
    /// Datagram from an address not in `link.allowed_sources`.
    #[error("source {0} is not allow-listed")]
    Source(IpAddr),
    /// Framing or header fault.
    #[error(transparent)]
    Packet(#[from] PacketError),
    /// A MID that is not in the table (allow-list).
    #[error("MID 0x{0:04X} is not configured")]
    UnknownMid(u16),
    /// The payload is shorter than the configured fields need.
    #[error("MID 0x{mid:04X}: payload of {got} bytes, fields need {need}")]
    PayloadTooShort {
        /// MID.
        mid: u16,
        /// Bytes needed.
        need: usize,
        /// Bytes received.
        got: usize,
    },
    /// The vehicle time stamp cannot be put on the TAI scale.
    #[error("vehicle time cannot be converted to TAI")]
    TimeUnconvertible,
    /// Vehicle time more than 1 s after receipt (docs/05 §4).
    #[error("vehicle time is {ahead_ms} ms after receipt (clock fault)")]
    TimeInFuture {
        /// How far ahead, ms.
        ahead_ms: i64,
    },
    /// A vehicle crew ID the table does not map.
    #[error("vehicle crew ID {0} is not mapped to a crew member")]
    UnmappedCrew(i64),
    /// The vehicle marked the value invalid.
    #[error("{0:?}: vehicle validity bit cleared")]
    SensorInvalid(Parameter),
    /// NaN or infinite.
    #[error("{0:?}: value is not finite")]
    NotFinite(Parameter),
    /// Outside the catalog's plausible range.
    #[error("{parameter:?}: {value} outside plausible range {min}..={max}")]
    Implausible {
        /// Parameter.
        parameter: Parameter,
        /// Converted value.
        value: f64,
        /// Lowest plausible.
        min: f64,
        /// Highest plausible.
        max: f64,
    },
    /// A state code the table does not map.
    #[error("{parameter:?}: state code {code} is not mapped")]
    UnknownState {
        /// Parameter.
        parameter: Parameter,
        /// Raw code.
        code: i64,
    },
}

impl Rejection {
    /// Stable code for counters and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Source(_) => "source_not_allowed",
            Self::Packet(p) => p.code(),
            Self::UnknownMid(_) => "unknown_mid",
            Self::PayloadTooShort { .. } => "payload_too_short",
            Self::TimeUnconvertible => "time_unconvertible",
            Self::TimeInFuture { .. } => "time_in_future",
            Self::UnmappedCrew(_) => "unmapped_crew",
            Self::SensorInvalid(_) => "sensor_invalid",
            Self::NotFinite(_) => "not_finite",
            Self::Implausible { .. } => "implausible",
            Self::UnknownState { .. } => "unknown_state",
        }
    }
}

/// What one datagram produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decoded {
    /// Accepted readings.
    pub readings: Vec<VehicleReading>,
    /// Dropped packet or fields, with reasons.
    pub rejections: Vec<Rejection>,
}

/// Per-MID link health.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct MidStats {
    /// Packets accepted (headers valid, MID configured).
    pub packets: u64,
    /// Gaps in the CCSDS sequence count.
    pub sequence_gaps: u64,
    /// Receipt time of the latest accepted packet (gaps make values STALE, SR-004).
    pub last_received_at: Option<TaiNs>,
}

/// Counters since the decoder started (F-ACQ-10 link health, SR-004).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Stats {
    /// Datagrams seen.
    pub datagrams: u64,
    /// Readings produced.
    pub readings: u64,
    /// Drops by [`Rejection::code`].
    pub rejected: BTreeMap<&'static str, u64>,
    /// Per configured MID.
    pub per_mid: BTreeMap<u16, MidStats>,
}

/// A raw scalar.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Raw {
    Int(i64),
    Float(f64),
}

fn read(payload: &[u8], at: Scalar, order: ByteOrder) -> Option<Raw> {
    let bytes = payload.get(at.offset..at.offset.checked_add(at.ty.size())?)?;
    macro_rules! num {
        ($t:ty) => {{
            let arr = <[u8; std::mem::size_of::<$t>()]>::try_from(bytes).ok()?;
            match order {
                ByteOrder::Little => <$t>::from_le_bytes(arr),
                ByteOrder::Big => <$t>::from_be_bytes(arr),
            }
        }};
    }
    Some(match at.ty {
        ScalarType::U8 => Raw::Int(i64::from(num!(u8))),
        ScalarType::I8 => Raw::Int(i64::from(num!(i8))),
        ScalarType::U16 => Raw::Int(i64::from(num!(u16))),
        ScalarType::I16 => Raw::Int(i64::from(num!(i16))),
        ScalarType::U32 => Raw::Int(i64::from(num!(u32))),
        ScalarType::I32 => Raw::Int(i64::from(num!(i32))),
        ScalarType::F32 => Raw::Float(f64::from(num!(f32))),
        ScalarType::F64 => Raw::Float(num!(f64)),
    })
}

fn read_int(payload: &[u8], at: Scalar, order: ByteOrder) -> Option<i64> {
    match read(payload, at, order)? {
        Raw::Int(v) => Some(v),
        Raw::Float(_) => None,
    }
}

fn flag_set(payload: &[u8], flag: Flag, order: ByteOrder) -> Option<bool> {
    let v = read_int(payload, flag.at, order)?;
    // Reinterpret the field's bits; the mask was checked to fit the field width.
    let bits = u64::from_ne_bytes(v.to_ne_bytes());
    Some(bits & flag.mask == flag.mask)
}

/// The sans-I/O decoder. Owns the MID table, the time correlator and the counters.
#[derive(Debug, Clone)]
pub struct Decoder {
    config: VehicleConfig,
    leap: LeapSeconds,
    correlator: Correlator,
    last_seq: BTreeMap<u16, u16>,
    stats: Stats,
}

impl Decoder {
    /// A decoder for a validated MID table, the signed leap-second table and the signed MET epoch.
    pub fn new(config: VehicleConfig, leap: LeapSeconds, met: MetEpoch) -> Self {
        let correlator = Correlator::new(
            config.has_time_message(),
            config.time.clock_state_max_age_s,
            met,
            &leap,
        );
        Self {
            config,
            leap,
            correlator,
            last_seq: BTreeMap::new(),
            stats: Stats::default(),
        }
    }

    /// The MID table in force.
    pub fn config(&self) -> &VehicleConfig {
        &self.config
    }

    /// Counters since start.
    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Vehicle time correlation at `now`.
    pub fn correlation(&self, now: TaiNs) -> CorrelationStatus {
        self.correlator.status(now)
    }

    /// Decodes one datagram from `from`, received at station time `received_at`.
    pub fn decode(&mut self, datagram: &[u8], from: IpAddr, received_at: TaiNs) -> Decoded {
        self.stats.datagrams += 1;
        let mut out = Decoded::default();
        if let Err(r) = self.decode_into(datagram, from, received_at, &mut out) {
            out.rejections.push(r);
        }
        for r in &out.rejections {
            *self.stats.rejected.entry(r.code()).or_insert(0) += 1;
        }
        self.stats.readings += out.readings.len() as u64;
        out
    }

    fn decode_into(
        &mut self,
        datagram: &[u8],
        from: IpAddr,
        received_at: TaiNs,
        out: &mut Decoded,
    ) -> Result<(), Rejection> {
        let Self {
            config,
            leap,
            correlator,
            last_seq,
            stats,
        } = self;
        if !config.link.allowed_sources.contains(&from) {
            return Err(Rejection::Source(from));
        }
        let bytes = packet::unframe(datagram, &config.link)?;
        let pkt = packet::parse(bytes, config.link.header)?;
        let def = config
            .messages
            .get(&pkt.mid)
            .ok_or(Rejection::UnknownMid(pkt.mid))?;
        let short = Rejection::PayloadTooShort {
            mid: pkt.mid,
            need: def.min_payload,
            got: pkt.payload.len(),
        };
        if pkt.payload.len() < def.min_payload {
            return Err(short);
        }
        count(last_seq, stats, &pkt, received_at);
        let vehicle =
            time::to_tai(config.time.epoch, pkt.time, leap).ok_or(Rejection::TimeUnconvertible)?;

        if def.kind == MessageKind::Time {
            let flag = def.clock_state.ok_or(short.clone())?;
            let set = flag_set(pkt.payload, flag, def.byte_order).ok_or(short.clone())?;
            let leap_s = match def.leap_seconds {
                Some(at) => Some(read_int(pkt.payload, at, def.byte_order).ok_or(short)?),
                None => None,
            };
            correlator.report(set, leap_s, received_at);
            correlator.observe_time_packet(vehicle, received_at);
            return Ok(());
        }
        let subject = match def.crew {
            Some(at) => {
                let id = read_int(pkt.payload, at, def.byte_order).ok_or(short.clone())?;
                let member = u32::try_from(id)
                    .ok()
                    .and_then(|v| config.crew.get(&v))
                    .ok_or(Rejection::UnmappedCrew(id))?;
                Subject::Crew(*member)
            }
            None => Subject::Cabin {
                location: def.location.clone(),
            },
        };
        let (measured_at, time) =
            correlator
                .measured_at(vehicle, received_at)
                .map_err(|ahead| Rejection::TimeInFuture {
                    ahead_ms: ahead / 1_000_000,
                })?;
        let met = correlator.met(measured_at);
        let mode = match def.kind {
            MessageKind::Exercise => MeasurementMode::Spot,
            _ => MeasurementMode::Continuous,
        };
        for field in &def.fields {
            match decode_field(field, pkt.payload, def.byte_order, &short) {
                Ok((value, original, quality)) => out.readings.push(VehicleReading {
                    mid: def.mid,
                    message: def.name.clone(),
                    parameter: field.parameter,
                    subject: subject.clone(),
                    value,
                    unit: field.parameter.spec().unit,
                    original,
                    measured_at,
                    met,
                    received_at,
                    time,
                    quality,
                    mode,
                }),
                Err(r) => out.rejections.push(r),
            }
        }
        Ok(())
    }
}

/// Per-MID packet count, sequence gaps and last receipt.
fn count(
    last_seq: &mut BTreeMap<u16, u16>,
    stats: &mut Stats,
    pkt: &TlmPacket<'_>,
    received_at: TaiNs,
) {
    let gap = match last_seq.insert(pkt.mid, pkt.sequence) {
        Some(prev) => pkt.sequence != (prev + 1) & 0x3FFF,
        None => false,
    };
    let s = stats.per_mid.entry(pkt.mid).or_default();
    s.packets += 1;
    s.sequence_gaps += u64::from(gap);
    s.last_received_at = Some(received_at);
}

fn decode_field(
    field: &FieldDef,
    payload: &[u8],
    order: ByteOrder,
    short: &Rejection,
) -> Result<(VehicleValue, Option<Original>, Quality), Rejection> {
    let p = field.parameter;
    // Lengths were checked against `min_payload`, so these reads cannot fail; never a panic.
    let missing = || short.clone();
    let quality = match field.validity {
        Some(flag) => {
            if !flag_set(payload, flag, order).ok_or_else(missing)? {
                return Err(Rejection::SensorInvalid(p));
            }
            Quality::Good
        }
        None => Quality::Degraded,
    };
    let raw = read(payload, field.at, order).ok_or_else(missing)?;
    match &field.conversion {
        Conversion::States(states) => {
            let Raw::Int(code) = raw else {
                return Err(Rejection::NotFinite(p));
            };
            let state = states
                .get(&code)
                .copied()
                .ok_or(Rejection::UnknownState { parameter: p, code })?;
            Ok((VehicleValue::State(state), None, quality))
        }
        Conversion::Numeric { unit, scale, bias } => {
            let raw = match raw {
                Raw::Int(v) => v as f64,
                Raw::Float(v) => v,
            };
            let calibrated = raw * scale + bias;
            if !calibrated.is_finite() {
                return Err(Rejection::NotFinite(p));
            }
            let spec = p.spec();
            let value = convert(calibrated, *unit, spec.unit, Substance::Other)
                .map_err(|_| Rejection::NotFinite(p))?;
            if let ValueType::Numeric { min, max } = spec.value {
                if !(min..=max).contains(&value) {
                    return Err(Rejection::Implausible {
                        parameter: p,
                        value,
                        min,
                        max,
                    });
                }
            }
            Ok((
                VehicleValue::Number(value),
                Some(Original {
                    value: calibrated,
                    unit: unit.ucum(),
                }),
                quality,
            ))
        }
    }
}
