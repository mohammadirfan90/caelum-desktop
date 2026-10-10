//! Sans-I/O parsing of inbound datagrams: optional SBN framing, the CCSDS 133.0-B-2 primary
//! header and the cFS telemetry secondary header (ICD-VEH §4, R-INT-04).
//!
//! Every length is checked before it is used; a malformed datagram is an error value, never a
//! panic. Telecommand packets are refused: CAELUM reads telemetry only (SR-011).

use crate::config::{Framing, HeaderLayout, LinkConfig, MID_SECHDR_BIT, MID_TYPE_BIT};

/// CCSDS primary header length.
pub const PRIMARY_HEADER_LEN: usize = 6;
/// SBN UDP message header: MsgSz u16, MsgType u8, ProcessorID u32, SpacecraftID u32 (big-endian).
pub const SBN_HEADER_LEN: usize = 11;
/// SBN message type of an application (Software Bus) message.
pub const SBN_APP_MSG: u8 = 3;

/// Vehicle time from the cFS secondary header: seconds and 1/65 536 s since the vehicle epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VehicleTime {
    /// Whole seconds since the vehicle epoch.
    pub seconds: u32,
    /// Subseconds in units of 2⁻¹⁶ s.
    pub subseconds: u16,
}

impl VehicleTime {
    /// Nanoseconds since the vehicle epoch (exact to the nearest lower nanosecond).
    pub fn nanos(self) -> i64 {
        i64::from(self.seconds) * 1_000_000_000
            + ((i64::from(self.subseconds) * 1_000_000_000) >> 16)
    }
}

/// A parsed cFS telemetry packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TlmPacket<'a> {
    /// cFS message ID (stream ID without the version bits).
    pub mid: u16,
    /// Application process ID.
    pub apid: u16,
    /// Packet sequence count (14 bits).
    pub sequence: u16,
    /// Vehicle time stamp.
    pub time: VehicleTime,
    /// Bytes after the cFS headers.
    pub payload: &'a [u8],
}

/// Why a datagram could not be read as a packet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PacketError {
    /// Shorter than the headers.
    #[error("datagram of {len} bytes is shorter than the headers")]
    TooShort {
        /// Bytes received.
        len: usize,
    },
    /// CCSDS version number other than 0 (version 1 packets).
    #[error("CCSDS version {0} is not supported")]
    Version(u8),
    /// Packet type bit set: a telecommand, which CAELUM never accepts (SR-011).
    #[error("telecommand packet refused")]
    Telecommand,
    /// No secondary header: not a cFS telemetry message.
    #[error("no secondary header")]
    NoSecondaryHeader,
    /// A segmented packet (sequence flags other than unsegmented).
    #[error("segmented packets are not supported")]
    Segmented,
    /// Declared length differs from the bytes received.
    #[error("packet length {declared} does not match the {actual} bytes received")]
    LengthMismatch {
        /// Length from the header.
        declared: usize,
        /// Bytes available.
        actual: usize,
    },
    /// An SBN control message (subscription, protocol, heartbeat): not data, ignored.
    #[error("SBN control message type {0} ignored")]
    SbnControl(u8),
    /// An SBN peer that is not allow-listed.
    #[error("SBN processor {0} is not allow-listed")]
    SbnPeer(u32),
}

impl PacketError {
    /// Stable code for counters and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::TooShort { .. } => "too_short",
            Self::Version(_) => "ccsds_version",
            Self::Telecommand => "telecommand",
            Self::NoSecondaryHeader => "no_secondary_header",
            Self::Segmented => "segmented",
            Self::LengthMismatch { .. } => "length_mismatch",
            Self::SbnControl(_) => "sbn_control",
            Self::SbnPeer(_) => "sbn_peer",
        }
    }
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(at)?,
        *b.get(at + 1)?,
        *b.get(at + 2)?,
        *b.get(at + 3)?,
    ]))
}

/// Removes the link framing and returns the space packet.
pub fn unframe<'a>(datagram: &'a [u8], link: &LinkConfig) -> Result<&'a [u8], PacketError> {
    match link.framing {
        Framing::Ccsds => Ok(datagram),
        Framing::Sbn => {
            let short = PacketError::TooShort {
                len: datagram.len(),
            };
            let size = usize::from(be16(datagram, 0).ok_or(short.clone())?);
            let kind = *datagram.get(2).ok_or(short.clone())?;
            let processor = be32(datagram, 3).ok_or(short.clone())?;
            let body = datagram.get(SBN_HEADER_LEN..).ok_or(short)?;
            if let Some(peers) = &link.sbn_peers {
                if !peers.contains(&processor) {
                    return Err(PacketError::SbnPeer(processor));
                }
            }
            if kind != SBN_APP_MSG {
                return Err(PacketError::SbnControl(kind));
            }
            if body.len() != size {
                return Err(PacketError::LengthMismatch {
                    declared: size,
                    actual: body.len(),
                });
            }
            Ok(body)
        }
    }
}

/// Parses one cFS telemetry packet (exactly one per datagram).
pub fn parse(packet: &[u8], layout: HeaderLayout) -> Result<TlmPacket<'_>, PacketError> {
    let header_len = layout.payload_offset();
    if packet.len() < header_len {
        return Err(PacketError::TooShort { len: packet.len() });
    }
    let short = PacketError::TooShort { len: packet.len() };
    let stream = be16(packet, 0).ok_or(short.clone())?;
    let seq = be16(packet, 2).ok_or(short.clone())?;
    let data_len = be16(packet, 4).ok_or(short.clone())?;
    // The top three bits are the version; the check above guarantees the shift fits a u8.
    let version = u8::try_from(stream >> 13).map_err(|_| short.clone())?;
    if version != 0 {
        return Err(PacketError::Version(version));
    }
    if stream & MID_TYPE_BIT != 0 {
        return Err(PacketError::Telecommand);
    }
    if stream & MID_SECHDR_BIT == 0 {
        return Err(PacketError::NoSecondaryHeader);
    }
    if seq >> 14 != 0b11 {
        return Err(PacketError::Segmented);
    }
    let declared = usize::from(data_len) + PRIMARY_HEADER_LEN + 1;
    if declared != packet.len() {
        return Err(PacketError::LengthMismatch {
            declared,
            actual: packet.len(),
        });
    }
    let seconds = be32(packet, PRIMARY_HEADER_LEN).ok_or(short.clone())?;
    let subseconds = be16(packet, PRIMARY_HEADER_LEN + 4).ok_or(short.clone())?;
    let payload = packet.get(header_len..).ok_or(short)?;
    Ok(TlmPacket {
        mid: stream & 0x1FFF,
        apid: stream & 0x07FF,
        sequence: seq & 0x3FFF,
        time: VehicleTime {
            seconds,
            subseconds,
        },
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Framing;

    fn packet(stream: u16, seq: u16, payload: &[u8], layout: HeaderLayout) -> Vec<u8> {
        let pad = layout.payload_offset() - 12;
        let total = 12 + pad + payload.len();
        let mut b = Vec::new();
        b.extend(stream.to_be_bytes());
        b.extend((0xC000 | seq).to_be_bytes());
        b.extend(u16::try_from(total - 7).expect("small").to_be_bytes());
        b.extend(1_000u32.to_be_bytes());
        b.extend(0x8000u16.to_be_bytes());
        b.extend(vec![0; pad]);
        b.extend(payload);
        b
    }

    /// R-INT-04: primary and cFE 7 secondary headers decode.
    #[test]
    fn r_int_04_parses_cfe7_and_cfe6_headers() {
        for layout in [HeaderLayout::Cfe7, HeaderLayout::Cfe6] {
            let b = packet(0x0C40, 42, &[1, 2, 3], layout);
            let p = parse(&b, layout).expect("valid");
            assert_eq!(p.mid, 0x0C40);
            assert_eq!(p.apid, 0x440);
            assert_eq!(p.sequence, 42);
            assert_eq!(p.time.seconds, 1_000);
            assert_eq!(p.time.nanos(), 1_000_500_000_000);
            assert_eq!(p.payload, &[1, 2, 3]);
        }
    }

    /// SR-011: telecommand packets are refused even if they arrive.
    #[test]
    fn sr_011_telecommands_are_refused() {
        let b = packet(0x1C40, 0, &[], HeaderLayout::Cfe7);
        assert_eq!(parse(&b, HeaderLayout::Cfe7), Err(PacketError::Telecommand));
    }

    #[test]
    fn malformed_headers_are_errors() {
        let l = HeaderLayout::Cfe7;
        assert!(matches!(parse(&[], l), Err(PacketError::TooShort { .. })));
        assert!(matches!(
            parse(&[0; 15], l),
            Err(PacketError::TooShort { .. })
        ));
        assert_eq!(
            parse(&packet(0x2C40, 0, &[], l), l),
            Err(PacketError::Version(1))
        );
        assert_eq!(
            parse(&packet(0x0440, 0, &[], l), l),
            Err(PacketError::NoSecondaryHeader)
        );
        let mut seg = packet(0x0C40, 0, &[], l);
        seg[2] &= 0x7F;
        assert_eq!(parse(&seg, l), Err(PacketError::Segmented));
        let mut long = packet(0x0C40, 0, &[1, 2], l);
        long.push(9);
        assert!(matches!(
            parse(&long, l),
            Err(PacketError::LengthMismatch { .. })
        ));
        let mut short = packet(0x0C40, 0, &[1, 2], l);
        short.pop();
        assert!(matches!(
            parse(&short, l),
            Err(PacketError::LengthMismatch { .. })
        ));
    }

    fn link(framing: Framing, peers: Option<Vec<u32>>) -> LinkConfig {
        LinkConfig {
            listen: "127.0.0.1:0".parse().expect("addr"),
            framing,
            header: HeaderLayout::Cfe7,
            allowed_sources: vec!["127.0.0.1".parse().expect("ip")],
            sbn_peers: peers,
        }
    }

    fn sbn(kind: u8, processor: u32, body: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(u16::try_from(body.len()).expect("small").to_be_bytes());
        b.push(kind);
        b.extend(processor.to_be_bytes());
        b.extend(66u32.to_be_bytes());
        b.extend(body);
        b
    }

    #[test]
    fn sbn_framing_keeps_application_messages_only() {
        let body = packet(0x0C40, 1, &[7], HeaderLayout::Cfe7);
        let l = link(Framing::Sbn, Some(vec![5]));
        assert_eq!(
            unframe(&sbn(SBN_APP_MSG, 5, &body), &l),
            Ok(body.as_slice())
        );
        assert_eq!(
            unframe(&sbn(1, 5, &[]), &l),
            Err(PacketError::SbnControl(1))
        );
        assert_eq!(
            unframe(&sbn(SBN_APP_MSG, 6, &body), &l),
            Err(PacketError::SbnPeer(6))
        );
        let mut bad = sbn(SBN_APP_MSG, 5, &body);
        bad.pop();
        assert!(matches!(
            unframe(&bad, &l),
            Err(PacketError::LengthMismatch { .. })
        ));
        assert!(matches!(
            unframe(&[0, 1], &l),
            Err(PacketError::TooShort { .. })
        ));
        let open = link(Framing::Sbn, None);
        assert!(unframe(&sbn(SBN_APP_MSG, 99, &body), &open).is_ok());
        let raw = link(Framing::Ccsds, None);
        assert_eq!(unframe(&body, &raw), Ok(body.as_slice()));
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// Arbitrary bytes never panic the parser.
            #[test]
            fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
                let _ = parse(&bytes, HeaderLayout::Cfe7);
                let _ = parse(&bytes, HeaderLayout::Cfe6);
                let _ = unframe(&bytes, &link(Framing::Sbn, Some(vec![1])));
            }
        }
    }
}
