//! cFS packet source (M9, docs/09 §2, ICD-VEH §11): encodes the cFS telemetry packets a vehicle
//! publishes, from values the test supplies, and emits them over UDP like a cFS telemetry output
//! or SBN peer. It never invents a value: a value the test leaves out is sent as NaN with its
//! validity bit cleared, as a vehicle reports a failed sensor.
//!
//! The encoder is written independently of `caelum-vehicle` (which it must not depend on), so the
//! decoder is tested against a second reading of ICD-VEH, not against itself.

use std::io;
use std::net::{SocketAddr, UdpSocket};

/// cFS telemetry header layouts (ICD-VEH §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// cFE 7: primary header, 6-byte time, 4 spare bytes (payload at 16).
    Cfe7,
    /// cFE 6: primary header, 6-byte time (payload at 12).
    Cfe6,
}

/// The packet header fields the test chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// cFS message ID (telemetry: type bit clear, secondary-header bit set).
    pub mid: u16,
    /// CCSDS sequence count (0..=0x3FFF).
    pub sequence: u16,
    /// Vehicle time, seconds since the vehicle epoch.
    pub seconds: u32,
    /// Vehicle time, 2⁻¹⁶ s.
    pub subseconds: u16,
}

/// A packet the source refuses to build.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CfsError {
    /// The MID is not a telemetry MID with a secondary header. The source cannot build commands.
    #[error("MID 0x{0:04X} is not a cFS telemetry MID")]
    NotTelemetry(u16),
    /// Sequence count out of range.
    #[error("sequence {0} exceeds 14 bits")]
    Sequence(u16),
    /// Packet too large for CCSDS.
    #[error("payload too large")]
    TooLarge,
}

/// Encodes a cFS telemetry packet (CCSDS 133.0-B-2 primary header, cFS secondary header with
/// big-endian time, unsegmented).
pub fn tlm_packet(layout: Layout, header: Header, payload: &[u8]) -> Result<Vec<u8>, CfsError> {
    if header.mid & 0xF800 != 0x0800 {
        return Err(CfsError::NotTelemetry(header.mid));
    }
    if header.sequence > 0x3FFF {
        return Err(CfsError::Sequence(header.sequence));
    }
    let spare = match layout {
        Layout::Cfe7 => 4,
        Layout::Cfe6 => 0,
    };
    let total = 12 + spare + payload.len();
    let data_len = u16::try_from(total - 7).map_err(|_| CfsError::TooLarge)?;
    let mut b = Vec::with_capacity(total);
    b.extend(header.mid.to_be_bytes());
    b.extend((0xC000 | header.sequence).to_be_bytes());
    b.extend(data_len.to_be_bytes());
    b.extend(header.seconds.to_be_bytes());
    b.extend(header.subseconds.to_be_bytes());
    b.extend(std::iter::repeat_n(0u8, spare));
    b.extend(payload);
    Ok(b)
}

/// SBN message type of an application message.
pub const SBN_APP_MSG: u8 = 3;

/// Wraps a packet in an SBN UDP application message (MsgSz, MsgType, ProcessorID, SpacecraftID).
pub fn sbn_app(processor_id: u32, spacecraft_id: u32, packet: &[u8]) -> Result<Vec<u8>, CfsError> {
    sbn(SBN_APP_MSG, processor_id, spacecraft_id, packet)
}

/// An SBN message of any type (e.g. a heartbeat or subscription the bridge must ignore).
pub fn sbn(
    msg_type: u8,
    processor_id: u32,
    spacecraft_id: u32,
    body: &[u8],
) -> Result<Vec<u8>, CfsError> {
    let size = u16::try_from(body.len()).map_err(|_| CfsError::TooLarge)?;
    let mut b = Vec::with_capacity(11 + body.len());
    b.extend(size.to_be_bytes());
    b.push(msg_type);
    b.extend(processor_id.to_be_bytes());
    b.extend(spacecraft_id.to_be_bytes());
    b.extend(body);
    Ok(b)
}

/// The ICD-VEH v0.2 reference MIDs (docs/icd/ICD-VEH.md §5).
pub mod mid {
    /// cFE TIME housekeeping.
    pub const TIME_HK: u16 = 0x0805;
    /// Cabin atmosphere.
    pub const CABIN_ATMOSPHERE: u16 = 0x0C40;
    /// Area dosimeter and SPE state.
    pub const RADIATION_AREA: u16 = 0x0C41;
    /// Personal dosimeter.
    pub const RADIATION_PERSONAL: u16 = 0x0C42;
    /// Exercise session summary.
    pub const EXERCISE_SESSION: u16 = 0x0C43;
}

/// A value the test supplies, or none (sent as NaN with the validity bit cleared).
fn value(v: Option<f32>, bit: u8, bits: &mut u8) -> [u8; 4] {
    match v {
        Some(x) => {
            *bits |= bit;
            x.to_le_bytes()
        }
        None => f32::NAN.to_le_bytes(),
    }
}

/// Cabin atmosphere (reference MID 0x0C40), in the units ICD-VEH §5 states.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CabinAtmosphere {
    /// ppCO₂, mmHg.
    pub co2_mmhg: Option<f32>,
    /// ppO₂, kPa.
    pub o2_kpa: Option<f32>,
    /// Total pressure, kPa.
    pub pressure_kpa: Option<f32>,
    /// Temperature, °C.
    pub temp_c: Option<f32>,
    /// Relative humidity, %.
    pub humidity_pct: Option<f32>,
}

/// Payload of the cabin-atmosphere message (24 bytes, little-endian).
pub fn cabin_atmosphere(v: &CabinAtmosphere) -> Vec<u8> {
    let mut bits = 0u8;
    let mut b = Vec::with_capacity(24);
    b.extend(value(v.co2_mmhg, 0x01, &mut bits));
    b.extend(value(v.o2_kpa, 0x02, &mut bits));
    b.extend(value(v.pressure_kpa, 0x04, &mut bits));
    b.extend(value(v.temp_c, 0x08, &mut bits));
    b.extend(value(v.humidity_pct, 0x10, &mut bits));
    b.push(bits);
    b.extend([0; 3]);
    b
}

/// Area dosimeter and SPE state (reference MID 0x0C41).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AreaRadiation {
    /// Dose-equivalent rate, µSv/h.
    pub dose_rate_usv_h: Option<f32>,
    /// Cumulative dose equivalent, mSv.
    pub cumulative_msv: Option<f32>,
    /// SPE state code from the vehicle database (reference: 0 none … 4 all_clear).
    pub spe_code: Option<u8>,
}

/// Payload of the area-radiation message (12 bytes, little-endian).
pub fn radiation_area(v: &AreaRadiation) -> Vec<u8> {
    let mut bits = 0u8;
    let mut b = Vec::with_capacity(12);
    b.extend(value(v.dose_rate_usv_h, 0x01, &mut bits));
    b.extend(value(v.cumulative_msv, 0x02, &mut bits));
    match v.spe_code {
        Some(code) => {
            bits |= 0x04;
            b.push(code);
        }
        None => b.push(0xFF),
    }
    b.push(bits);
    b.extend([0; 2]);
    b
}

/// Personal dosimeter (reference MID 0x0C42).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PersonalDose {
    /// Vehicle crew ID.
    pub crew: u8,
    /// Dose-equivalent rate, µSv/h.
    pub dose_rate_usv_h: Option<f32>,
    /// Cumulative dose equivalent, mSv.
    pub cumulative_msv: Option<f32>,
}

/// Payload of the personal-dosimeter message (12 bytes, little-endian).
pub fn radiation_personal(v: &PersonalDose) -> Vec<u8> {
    let mut bits = 0u8;
    let rate = value(v.dose_rate_usv_h, 0x01, &mut bits);
    let cumulative = value(v.cumulative_msv, 0x02, &mut bits);
    let mut b = vec![v.crew, bits, 0, 0];
    b.extend(rate);
    b.extend(cumulative);
    b
}

/// Exercise session summary (reference MID 0x0C43).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExerciseSession {
    /// Vehicle crew ID.
    pub crew: u8,
    /// Mean load, W.
    pub mean_load_w: f32,
    /// Duration, s.
    pub duration_s: u32,
}

/// Payload of the exercise-session message (12 bytes, little-endian).
pub fn exercise_session(v: &ExerciseSession) -> Vec<u8> {
    let mut b = vec![v.crew, 0, 0, 0];
    b.extend(v.mean_load_w.to_le_bytes());
    b.extend(v.duration_s.to_le_bytes());
    b
}

/// cFE TIME housekeeping, the fields the bridge reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeHk {
    /// CLKSET (ClockStateFlags bit 15).
    pub clock_set: bool,
    /// TAI−UTC, s.
    pub leap_seconds: i16,
}

/// Payload of the TIME housekeeping message: command counters (bytes 0–1) and ClockStateAPI
/// (bytes 4–5) are not modelled and sent as zero; ClockStateFlags at 2, LeapSeconds at 6.
pub fn time_hk(v: &TimeHk) -> Vec<u8> {
    let flags: u16 = if v.clock_set { 0x8000 } else { 0 };
    let mut b = vec![0, 0];
    b.extend(flags.to_le_bytes());
    b.extend([0, 0]);
    b.extend(v.leap_seconds.to_le_bytes());
    b
}

/// Emits datagrams to the bridge over UDP from a loopback socket.
#[derive(Debug)]
pub struct CfsUdpSource {
    socket: UdpSocket,
    target: SocketAddr,
}

impl CfsUdpSource {
    /// A source on an ephemeral loopback port sending to `target`.
    pub fn loopback(target: SocketAddr) -> io::Result<Self> {
        Ok(Self {
            socket: UdpSocket::bind("127.0.0.1:0")?,
            target,
        })
    }

    /// The source address (what the bridge's allow-list must contain).
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Sends one datagram.
    pub fn emit(&self, datagram: &[u8]) -> io::Result<()> {
        let n = self.socket.send_to(datagram, self.target)?;
        if n == datagram.len() {
            Ok(())
        } else {
            Err(io::Error::other("datagram truncated"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_have_the_cfs_telemetry_header() {
        let h = Header {
            mid: mid::CABIN_ATMOSPHERE,
            sequence: 5,
            seconds: 0x0102_0304,
            subseconds: 0x8000,
        };
        let p = tlm_packet(Layout::Cfe7, h, &[9, 9]).expect("valid");
        assert_eq!(
            p,
            [0x0C, 0x40, 0xC0, 0x05, 0x00, 0x0B, 1, 2, 3, 4, 0x80, 0, 0, 0, 0, 0, 9, 9]
        );
        assert_eq!(tlm_packet(Layout::Cfe6, h, &[]).expect("valid").len(), 12);
    }

    /// SR-011: the source cannot build a telecommand either.
    #[test]
    fn sr_011_command_mids_are_refused() {
        let h = Header {
            mid: 0x1C40,
            sequence: 0,
            seconds: 0,
            subseconds: 0,
        };
        assert_eq!(
            tlm_packet(Layout::Cfe7, h, &[]),
            Err(CfsError::NotTelemetry(0x1C40))
        );
        let h = Header {
            mid: 0x0C40,
            sequence: 0x4000,
            ..h
        };
        assert_eq!(
            tlm_packet(Layout::Cfe7, h, &[]),
            Err(CfsError::Sequence(0x4000))
        );
    }

    /// Rule 2: an omitted value goes out as NaN with its validity bit cleared.
    #[test]
    fn omitted_values_are_flagged_invalid_not_invented() {
        let p = cabin_atmosphere(&CabinAtmosphere {
            co2_mmhg: Some(5.0),
            ..CabinAtmosphere::default()
        });
        assert_eq!(p.len(), 24);
        assert_eq!(p[20], 0x01);
        assert!(f32::from_le_bytes([p[4], p[5], p[6], p[7]]).is_nan());
        let r = radiation_area(&AreaRadiation::default());
        assert_eq!(r[9], 0);
    }

    #[test]
    fn sbn_header_is_eleven_bytes() {
        let b = sbn_app(7, 66, &[1, 2, 3]).expect("small");
        assert_eq!(b[..3], [0, 3, SBN_APP_MSG]);
        assert_eq!(b.len(), 14);
    }
}
