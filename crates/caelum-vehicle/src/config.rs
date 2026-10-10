//! The vehicle MID table: which cFS messages CAELUM reads and how each field maps to the parameter
//! catalog (R-INT-04, F-GND-11, CR-015 §3, docs/icd/ICD-VEH.md §5).
//!
//! The table arrives as a signed configuration bundle of kind `vehicle` (verified by the caller,
//! CR-009/CR-012 envelope); this module parses and validates the verified bytes. Nothing is
//! defaulted: every MID, offset, type, unit and state code comes from the file, unknown keys are
//! refused, and every inconsistency is an error at load time rather than a surprise per packet.
//! Only telemetry MIDs are accepted; a command MID cannot be configured (SR-011).

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};

use caelum_domain::ids::CrewMemberId;
use caelum_domain::parameters::{Parameter, ValueType};
use caelum_domain::time::TaiNs;
use caelum_domain::units::{convert, Substance, Unit};
use serde::Deserialize;

use crate::spe::SPE_STATES;

/// cFS message ID bits (CCSDS primary-header stream ID with the version bits cleared).
pub(crate) const MID_TYPE_BIT: u16 = 0x1000;
pub(crate) const MID_SECHDR_BIT: u16 = 0x0800;
pub(crate) const MID_VERSION_BITS: u16 = 0xE000;

/// Unix seconds of 1972-01-01, the first instant a UTC epoch can be converted (leap table).
const UNIX_1972: i64 = 63_072_000;
/// Days from 1958-01-01 to 1970-01-01.
const DAYS_1958_TO_1970: i64 = 4_383;

/// A configuration that cannot be used. The whole table is refused (SR-009 keeps the previous one).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("vehicle configuration: {0}")]
pub struct ConfigError(pub String);

fn err<T>(msg: impl Into<String>) -> Result<T, ConfigError> {
    Err(ConfigError(msg.into()))
}

/// How datagrams on the link are framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framing {
    /// One CCSDS space packet per datagram (cFS UDP telemetry output, e.g. TO_LAB).
    Ccsds,
    /// SBN UDP module datagrams; only application messages carrying a packet are read.
    Sbn,
}

/// cFS telemetry header layout (CCSDS primary header plus cFS secondary header).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderLayout {
    /// cFE 7 (Draco and later): 6-byte time plus 4 spare bytes; payload at byte 16.
    Cfe7,
    /// cFE 6: 6-byte time; payload at byte 12.
    Cfe6,
}

impl HeaderLayout {
    /// Byte offset of the payload from the start of the packet.
    pub fn payload_offset(self) -> usize {
        match self {
            Self::Cfe7 => 16,
            Self::Cfe6 => 12,
        }
    }
}

/// Time scale of the vehicle clock (cFE `CFE_MISSION_TIME_CFG_DEFAULT_TAI` / `_UTC`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeScale {
    /// Uniform seconds since the epoch.
    Tai,
    /// UTC seconds since the epoch, leap seconds not counted.
    Utc,
}

/// Payload byte order (cFS payloads are in the producing processor's native order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ByteOrder {
    /// Little-endian.
    Little,
    /// Big-endian.
    Big,
}

/// Field encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarType {
    /// Unsigned 8-bit.
    U8,
    /// Signed 8-bit.
    I8,
    /// Unsigned 16-bit.
    U16,
    /// Signed 16-bit.
    I16,
    /// Unsigned 32-bit.
    U32,
    /// Signed 32-bit.
    I32,
    /// IEEE 754 single.
    F32,
    /// IEEE 754 double.
    F64,
}

impl ScalarType {
    /// Size in bytes.
    pub fn size(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }

    /// Whether the type is an integer (flags, codes and crew IDs must be).
    pub fn is_integer(self) -> bool {
        !matches!(self, Self::F32 | Self::F64)
    }
}

/// What a message carries (ICD-VEH §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// Cabin atmosphere (F-ENV-01).
    Environment,
    /// Area or personal dosimetry and SPE state (F-ENV-02).
    Radiation,
    /// Exercise device sessions (F-ENV-03), one message per session, per crew member.
    Exercise,
    /// Vehicle time service status (clock state, leap seconds), e.g. cFE TIME housekeeping.
    Time,
}

impl MessageKind {
    fn allows(self, p: Parameter) -> bool {
        use Parameter as P;
        match self {
            Self::Environment => matches!(
                p,
                P::CabinCo2 | P::CabinO2 | P::CabinPressure | P::CabinTemp | P::CabinHumidity
            ),
            Self::Radiation => matches!(
                p,
                P::RadiationDoseRate | P::RadiationDoseCumulative | P::SpeState
            ),
            Self::Exercise => matches!(p, P::ExerciseLoad | P::ExerciseDuration),
            Self::Time => false,
        }
    }
}

/// A scalar at a payload offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scalar {
    /// Byte offset in the payload (after the cFS headers).
    pub offset: usize,
    /// Encoding.
    pub ty: ScalarType,
}

impl Scalar {
    fn end(self) -> usize {
        self.offset + self.ty.size()
    }
}

/// An integer field tested against a bit mask (validity bits, clock state).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flag {
    /// Where the bits are.
    pub at: Scalar,
    /// The bits that must all be set.
    pub mask: u64,
}

/// How a raw field becomes a catalog value.
#[derive(Debug, Clone, PartialEq)]
pub enum Conversion {
    /// `raw × scale + bias`, in `unit`, then converted to the parameter's canonical unit.
    Numeric {
        /// Unit of `raw × scale + bias` (UCUM, a catalog unit).
        unit: Unit,
        /// Multiplier.
        scale: f64,
        /// Added after scaling.
        bias: f64,
    },
    /// Integer code → catalog state.
    States(BTreeMap<i64, &'static str>),
}

/// One field of a message.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    /// Catalog parameter.
    pub parameter: Parameter,
    /// Where the raw value is.
    pub at: Scalar,
    /// Calibration and unit, or state codes.
    pub conversion: Conversion,
    /// Vehicle validity bits for this value, if the message has them.
    pub validity: Option<Flag>,
}

/// One configured MID.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageDef {
    /// cFS message ID (telemetry, secondary header present).
    pub mid: u16,
    /// Name from the vehicle mission database (for logs and the ICD).
    pub name: String,
    /// What it carries.
    pub kind: MessageKind,
    /// Payload byte order.
    pub byte_order: ByteOrder,
    /// Vehicle crew ID field (personal dosimeter, exercise device); none for cabin-wide data.
    pub crew: Option<Scalar>,
    /// Area label for cabin-wide data (e.g. module name), if configured.
    pub location: Option<String>,
    /// Time messages: the clock-set bits.
    pub clock_state: Option<Flag>,
    /// Time messages: the vehicle's TAI−UTC (leap seconds) field, if any.
    pub leap_seconds: Option<Scalar>,
    /// Data fields.
    pub fields: Vec<FieldDef>,
    /// Smallest payload that holds every configured field.
    pub min_payload: usize,
}

/// The vehicle time epoch (ICD-VEH §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleEpoch {
    /// TAI scale: the epoch as TAI since the CCSDS epoch.
    Tai(TaiNs),
    /// UTC scale: the epoch as Unix seconds (UTC); vehicle seconds do not count leap seconds.
    Utc {
        /// Unix seconds of the epoch.
        unix_s: i64,
    },
}

/// Link settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkConfig {
    /// Local address the receiver binds.
    pub listen: SocketAddr,
    /// Datagram framing.
    pub framing: Framing,
    /// cFS header layout.
    pub header: HeaderLayout,
    /// Source addresses accepted; datagrams from anyone else are dropped and counted.
    pub allowed_sources: Vec<IpAddr>,
    /// SBN processor IDs accepted (SBN framing only); `None` accepts any peer on an allowed source.
    pub sbn_peers: Option<Vec<u32>>,
}

/// Vehicle time settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeConfig {
    /// Epoch and scale of the vehicle clock.
    pub epoch: VehicleEpoch,
    /// Age after which the last reported clock state no longer counts (vehicle time unused).
    pub clock_state_max_age_s: u32,
}

/// A validated MID table.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleConfig {
    /// Configuration version (audited when applied).
    pub version: String,
    /// Link.
    pub link: LinkConfig,
    /// Vehicle time.
    pub time: TimeConfig,
    /// Vehicle crew ID → CAELUM crew member.
    pub crew: BTreeMap<u32, CrewMemberId>,
    /// MID → message definition.
    pub messages: BTreeMap<u16, MessageDef>,
}

// ---------------------------------------------------------------- raw TOML

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    vehicle: RawVehicle,
    link: RawLink,
    time: RawTime,
    #[serde(default)]
    crew: Vec<RawCrew>,
    #[serde(default)]
    message: Vec<RawMessage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVehicle {
    version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLink {
    listen: SocketAddr,
    framing: Framing,
    header: HeaderLayout,
    allowed_sources: Vec<IpAddr>,
    sbn_peers: Option<Vec<u32>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTime {
    epoch: String,
    scale: TimeScale,
    clock_state_max_age_s: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCrew {
    vehicle_id: u32,
    crew_member: String,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
struct RawScalar {
    offset: usize,
    #[serde(rename = "type")]
    ty: ScalarType,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
struct RawFlag {
    offset: usize,
    #[serde(rename = "type")]
    ty: ScalarType,
    mask: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawState {
    code: i64,
    state: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawField {
    parameter: String,
    offset: usize,
    #[serde(rename = "type")]
    ty: ScalarType,
    unit: Option<String>,
    scale: Option<f64>,
    bias: Option<f64>,
    validity: Option<RawFlag>,
    states: Option<Vec<RawState>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMessage {
    mid: u16,
    name: String,
    kind: MessageKind,
    byte_order: ByteOrder,
    crew_field: Option<RawScalar>,
    location: Option<String>,
    clock_state: Option<RawFlag>,
    leap_seconds: Option<RawScalar>,
    #[serde(default)]
    field: Vec<RawField>,
}

// ---------------------------------------------------------------- validation

/// Days from 1970-01-01 to a civil date (proleptic Gregorian).
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parses `YYYY-MM-DDTHH:MM:SS` into (days since 1970-01-01, seconds of day).
fn parse_calendar(text: &str) -> Result<(i64, i64), ConfigError> {
    let bad = || {
        ConfigError(format!(
            "time.epoch must be YYYY-MM-DDTHH:MM:SS, got {text:?}"
        ))
    };
    let (date, clock) = text.split_once('T').ok_or_else(bad)?;
    let d: Vec<&str> = date.split('-').collect();
    let c: Vec<&str> = clock.split(':').collect();
    let ([y, mo, da], [h, mi, s]) = (d.as_slice(), c.as_slice()) else {
        return Err(bad());
    };
    let num = |p: &str, len: usize| -> Result<i64, ConfigError> {
        if p.len() != len || !p.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        p.parse().map_err(|_| bad())
    };
    let (y, mo, da) = (num(y, 4)?, num(mo, 2)?, num(da, 2)?);
    let (h, mi, s) = (num(h, 2)?, num(mi, 2)?, num(s, 2)?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let max_day = usize::try_from(mo - 1)
        .ok()
        .and_then(|i| month_days.get(i))
        .copied()
        .ok_or_else(bad)?;
    if da < 1 || da > max_day || h > 23 || mi > 59 || s > 59 {
        return Err(bad());
    }
    Ok((days_from_civil(y, mo, da), h * 3600 + mi * 60 + s))
}

fn epoch(raw: &RawTime) -> Result<VehicleEpoch, ConfigError> {
    let (days, sod) = parse_calendar(&raw.epoch)?;
    match raw.scale {
        TimeScale::Tai => {
            let s = (days + DAYS_1958_TO_1970) * 86_400 + sod;
            if s < 0 {
                return err("time.epoch must not precede the CCSDS epoch 1958-01-01");
            }
            Ok(VehicleEpoch::Tai(TaiNs(s * 1_000_000_000)))
        }
        TimeScale::Utc => {
            let unix_s = days * 86_400 + sod;
            if unix_s < UNIX_1972 {
                return err("a UTC time.epoch must not precede 1972-01-01 (leap-second table)");
            }
            Ok(VehicleEpoch::Utc { unix_s })
        }
    }
}

fn flag(raw: RawFlag, what: &str) -> Result<Flag, ConfigError> {
    if !raw.ty.is_integer() {
        return err(format!("{what}: flag fields must be integers"));
    }
    if raw.mask == 0 {
        return err(format!("{what}: mask must not be zero"));
    }
    let bits = raw.ty.size() * 8;
    if bits < 64 && raw.mask >> bits != 0 {
        return err(format!("{what}: mask is wider than the field"));
    }
    Ok(Flag {
        at: Scalar {
            offset: raw.offset,
            ty: raw.ty,
        },
        mask: raw.mask,
    })
}

fn field(raw: RawField, kind: MessageKind, what: &str) -> Result<FieldDef, ConfigError> {
    let parameter = Parameter::from_key(&raw.parameter)
        .ok_or_else(|| ConfigError(format!("{what}: unknown parameter {:?}", raw.parameter)))?;
    if !kind.allows(parameter) {
        return err(format!(
            "{what}: parameter {} does not belong in a {kind:?} message",
            raw.parameter
        ));
    }
    let at = Scalar {
        offset: raw.offset,
        ty: raw.ty,
    };
    let what = format!("{what} ({})", raw.parameter);
    let conversion = if parameter == Parameter::SpeState {
        // CR-015 §4: states are CAELUM's vocabulary; codes come from the vehicle database.
        if raw.unit.is_some() || raw.scale.is_some() || raw.bias.is_some() {
            return err(format!("{what}: a state field has no unit, scale or bias"));
        }
        if !raw.ty.is_integer() {
            return err(format!("{what}: state codes must be integers"));
        }
        let states = raw
            .states
            .ok_or_else(|| ConfigError(format!("{what}: states are required")))?;
        if states.is_empty() {
            return err(format!("{what}: states must not be empty"));
        }
        let mut map = BTreeMap::new();
        for s in states {
            let known = SPE_STATES
                .iter()
                .copied()
                .find(|k| *k == s.state)
                .ok_or_else(|| ConfigError(format!("{what}: unknown state {:?}", s.state)))?;
            if map.insert(s.code, known).is_some() {
                return err(format!("{what}: code {} given twice", s.code));
            }
        }
        Conversion::States(map)
    } else {
        if raw.states.is_some() {
            return err(format!("{what}: numeric fields have no states"));
        }
        let ValueType::Numeric { .. } = parameter.spec().value else {
            return err(format!("{what}: not a numeric parameter"));
        };
        let code = raw
            .unit
            .ok_or_else(|| ConfigError(format!("{what}: unit is required (never assumed)")))?;
        let unit = Unit::from_ucum(&code)
            .ok_or_else(|| ConfigError(format!("{what}: unknown UCUM unit {code:?}")))?;
        let canonical = parameter.spec().unit;
        if convert(1.0, unit, canonical, Substance::Other).is_err() {
            return err(format!(
                "{what}: no conversion from {} to {}",
                unit.ucum(),
                canonical.ucum()
            ));
        }
        let scale = raw.scale.unwrap_or(1.0);
        let bias = raw.bias.unwrap_or(0.0);
        if !scale.is_finite() || scale == 0.0 || !bias.is_finite() {
            return err(format!(
                "{what}: scale must be finite and non-zero, bias finite"
            ));
        }
        Conversion::Numeric { unit, scale, bias }
    };
    let validity = raw
        .validity
        .map(|v| flag(v, &format!("{what} validity")))
        .transpose()?;
    Ok(FieldDef {
        parameter,
        at,
        conversion,
        validity,
    })
}

fn message(raw: RawMessage) -> Result<MessageDef, ConfigError> {
    let what = format!("message 0x{:04X}", raw.mid);
    if raw.mid & MID_VERSION_BITS != 0 {
        return err(format!("{what}: version bits must be zero"));
    }
    if raw.mid & MID_TYPE_BIT != 0 {
        return err(format!(
            "{what}: is a command MID; CAELUM reads telemetry only (SR-011)"
        ));
    }
    if raw.mid & MID_SECHDR_BIT == 0 {
        return err(format!("{what}: cFS telemetry has a secondary header"));
    }
    if raw.name.trim().is_empty() {
        return err(format!("{what}: name must not be empty"));
    }
    if raw.location.as_deref().is_some_and(|l| l.trim().is_empty()) {
        return err(format!("{what}: location must not be empty when given"));
    }
    let crew = match raw.crew_field {
        Some(c) if !c.ty.is_integer() => {
            return err(format!("{what}: crew_field must be an integer"))
        }
        Some(c) => Some(Scalar {
            offset: c.offset,
            ty: c.ty,
        }),
        None => None,
    };
    let is_time = raw.kind == MessageKind::Time;
    if is_time != raw.clock_state.is_some() {
        return err(format!(
            "{what}: clock_state is required for time messages and only for them"
        ));
    }
    if !is_time && raw.leap_seconds.is_some() {
        return err(format!(
            "{what}: leap_seconds only belongs in a time message"
        ));
    }
    match raw.kind {
        MessageKind::Time if !raw.field.is_empty() || crew.is_some() || raw.location.is_some() => {
            return err(format!(
                "{what}: a time message has no data fields, crew or location"
            ))
        }
        MessageKind::Exercise if crew.is_none() => {
            return err(format!("{what}: exercise sessions need a crew_field"))
        }
        MessageKind::Environment if crew.is_some() => {
            return err(format!("{what}: cabin environment is not per crew member"))
        }
        MessageKind::Environment | MessageKind::Radiation | MessageKind::Exercise
            if raw.field.is_empty() =>
        {
            return err(format!("{what}: no fields"))
        }
        _ => {}
    }
    let clock_state = raw
        .clock_state
        .map(|f| flag(f, &format!("{what} clock_state")))
        .transpose()?;
    let leap_seconds = match raw.leap_seconds {
        Some(l) if !l.ty.is_integer() => {
            return err(format!("{what}: leap_seconds must be an integer"))
        }
        Some(l) => Some(Scalar {
            offset: l.offset,
            ty: l.ty,
        }),
        None => None,
    };
    let mut seen = BTreeSet::new();
    let mut fields = Vec::with_capacity(raw.field.len());
    for f in raw.field {
        let def = field(f, raw.kind, &what)?;
        if !seen.insert(def.parameter) {
            return err(format!(
                "{what}: parameter {:?} mapped twice",
                def.parameter
            ));
        }
        fields.push(def);
    }
    let ends = fields
        .iter()
        .flat_map(|f| [Some(f.at.end()), f.validity.map(|v| v.at.end())])
        .chain([
            crew.map(Scalar::end),
            clock_state.map(|c| c.at.end()),
            leap_seconds.map(Scalar::end),
        ])
        .flatten();
    let min_payload = ends.max().unwrap_or(0);
    if min_payload > usize::from(u16::MAX) {
        return err(format!(
            "{what}: fields lie beyond the largest CCSDS packet"
        ));
    }
    Ok(MessageDef {
        mid: raw.mid,
        name: raw.name,
        kind: raw.kind,
        byte_order: raw.byte_order,
        crew,
        location: raw.location,
        clock_state,
        leap_seconds,
        fields,
        min_payload,
    })
}

impl VehicleConfig {
    /// Parses and validates the (already signature-verified) TOML bytes of a `vehicle` bundle.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError(e.to_string()))?;
        if raw.vehicle.version.trim().is_empty() {
            return err("vehicle.version must not be empty");
        }
        if raw.link.allowed_sources.is_empty() {
            return err("link.allowed_sources must list at least one vehicle address");
        }
        match (raw.link.framing, &raw.link.sbn_peers) {
            (Framing::Ccsds, Some(_)) => return err("link.sbn_peers only applies to SBN framing"),
            (Framing::Sbn, Some(p)) if p.is_empty() => {
                return err("link.sbn_peers must not be empty when given")
            }
            _ => {}
        }
        if raw.time.clock_state_max_age_s == 0 {
            return err("time.clock_state_max_age_s must be positive");
        }
        let time = TimeConfig {
            epoch: epoch(&raw.time)?,
            clock_state_max_age_s: raw.time.clock_state_max_age_s,
        };

        let mut crew = BTreeMap::new();
        let mut members = BTreeSet::new();
        for c in raw.crew {
            let id: CrewMemberId = c.crew_member.parse().map_err(|_| {
                ConfigError(format!("crew {}: invalid crew_member UUID", c.vehicle_id))
            })?;
            if !members.insert(id) {
                return err(format!("crew member {id} mapped twice"));
            }
            if crew.insert(c.vehicle_id, id).is_some() {
                return err(format!("vehicle crew ID {} mapped twice", c.vehicle_id));
            }
        }

        if raw.message.is_empty() {
            return err("no messages configured");
        }
        let mut messages = BTreeMap::new();
        let mut times = 0;
        for m in raw.message {
            let def = message(m)?;
            if def.crew.is_some() && crew.is_empty() {
                return err(format!(
                    "message 0x{:04X} names a crew_field but no [[crew]] mapping is configured",
                    def.mid
                ));
            }
            times += usize::from(def.kind == MessageKind::Time);
            let mid = def.mid;
            if messages.insert(mid, def).is_some() {
                return err(format!("message 0x{mid:04X} configured twice"));
            }
        }
        if times > 1 {
            return err("at most one time message may be configured");
        }
        Ok(Self {
            version: raw.vehicle.version,
            link: LinkConfig {
                listen: raw.link.listen,
                framing: raw.link.framing,
                header: raw.link.header,
                allowed_sources: raw.link.allowed_sources,
                sbn_peers: raw.link.sbn_peers,
            },
            time,
            crew,
            messages,
        })
    }

    /// Whether a time message is configured (without one, vehicle time is never used).
    pub fn has_time_message(&self) -> bool {
        self.messages.values().any(|m| m.kind == MessageKind::Time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE: &str = include_str!("../tests/fixtures/reference.toml");

    fn with(replace: &str, by: &str) -> Result<VehicleConfig, ConfigError> {
        assert!(REFERENCE.contains(replace), "fixture lacks {replace:?}");
        VehicleConfig::from_toml(&REFERENCE.replacen(replace, by, 1))
    }

    /// R-INT-04: the reference MID table (ICD-VEH §5) parses and validates.
    #[test]
    fn r_int_04_reference_table_is_valid() {
        let c = VehicleConfig::from_toml(REFERENCE).expect("reference table");
        assert_eq!(c.messages.len(), 5);
        assert!(c.has_time_message());
        assert_eq!(c.link.header.payload_offset(), 16);
        // 1980-01-06 (GPS-aligned cFE epoch) is 8 040 days after 1958-01-01.
        assert_eq!(
            c.time.epoch,
            VehicleEpoch::Tai(TaiNs(8_040 * 86_400 * 1_000_000_000))
        );
        let env = &c.messages[&0x0C40];
        assert_eq!(env.kind, MessageKind::Environment);
        assert_eq!(env.fields.len(), 5);
        assert_eq!(env.min_payload, 21);
    }

    /// SR-011: a command MID (type bit set) can never be configured.
    #[test]
    fn sr_011_command_mids_are_refused() {
        let e = with("mid = 0x0C40", "mid = 0x1C40").expect_err("command MID");
        assert!(e.0.contains("command MID"), "{e}");
        assert!(with("mid = 0x0C40", "mid = 0x0440").is_err()); // no secondary header
        assert!(with("mid = 0x0C40", "mid = 0x2C40").is_err()); // version bits
    }

    #[test]
    fn unknown_keys_units_and_parameters_are_refused() {
        assert!(with("[vehicle]", "[vehicle]\ncolour = \"red\"").is_err());
        assert!(with("unit = \"mm[Hg]\"", "unit = \"ppm\"").is_err());
        // No conversion from Celsius to the canonical mmHg of cabin_co2.
        assert!(with("unit = \"mm[Hg]\"", "unit = \"Cel\"").is_err());
        assert!(with("parameter = \"cabin_co2\"", "parameter = \"cabin_co3\"").is_err());
        // Heart rate never comes from the vehicle.
        assert!(with("parameter = \"cabin_co2\"", "parameter = \"heart_rate\"").is_err());
    }

    /// Rule 2: a numeric field without a unit is refused, never assumed.
    #[test]
    fn units_are_never_assumed() {
        let e = with("unit = \"mm[Hg]\"\n", "").expect_err("no unit");
        assert!(e.0.contains("unit is required"), "{e}");
    }

    /// CR-015 §4: SPE state names outside the CAELUM vocabulary are refused.
    #[test]
    fn f_env_02_unknown_spe_state_names_are_refused() {
        assert!(with("state = \"watch\"", "state = \"orange\"").is_err());
        assert!(with(
            "{ code = 1, state = \"watch\" }",
            "{ code = 0, state = \"watch\" }"
        )
        .is_err());
    }

    #[test]
    fn link_and_time_settings_are_validated() {
        assert!(with("allowed_sources = [\"127.0.0.1\"]", "allowed_sources = []").is_err());
        assert!(with(
            "epoch = \"1980-01-06T00:00:00\"",
            "epoch = \"1980-02-30T00:00:00\""
        )
        .is_err());
        assert!(with("epoch = \"1980-01-06T00:00:00\"", "epoch = \"1980-01-06\"").is_err());
        assert!(with("scale = \"tai\"", "scale = \"utc\"").is_ok());
        let utc_1970 = REFERENCE
            .replacen(
                "epoch = \"1980-01-06T00:00:00\"",
                "epoch = \"1970-01-01T00:00:00\"",
                1,
            )
            .replacen("scale = \"tai\"", "scale = \"utc\"", 1);
        assert!(VehicleConfig::from_toml(&utc_1970).is_err());
        assert!(with("clock_state_max_age_s = 30", "clock_state_max_age_s = 0").is_err());
        assert!(with(
            "framing = \"ccsds\"",
            "framing = \"ccsds\"\nsbn_peers = [1]"
        )
        .is_err());
    }

    #[test]
    fn duplicates_are_refused() {
        let dup_mid = format!(
            "{REFERENCE}\n[[message]]\nmid = 0x0C40\nname = \"X\"\nkind = \"environment\"\nbyte_order = \"little\"\n[[message.field]]\nparameter = \"cabin_temp\"\noffset = 0\ntype = \"f32\"\nunit = \"Cel\"\n"
        );
        assert!(VehicleConfig::from_toml(&dup_mid).is_err());
        let dup_crew = REFERENCE.replacen("vehicle_id = 2", "vehicle_id = 1", 1);
        assert!(VehicleConfig::from_toml(&dup_crew).is_err());
    }

    #[test]
    fn kind_rules_are_enforced() {
        // Exercise sessions belong to a crew member.
        assert!(with("crew_field = { offset = 0, type = \"u8\" }\n\n[[message.field]]\nparameter = \"exercise_load\"", "\n[[message.field]]\nparameter = \"exercise_load\"").is_err());
        // Flags must be integers with a non-zero mask that fits the field.
        assert!(with("mask = 0x8000", "mask = 0").is_err());
        assert!(with("mask = 0x8000", "mask = 0x18000").is_err());
    }
}
