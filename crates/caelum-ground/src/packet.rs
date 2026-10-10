//! CCSDS Space Packets (CCSDS 133.0-B-2) with a CUC time secondary header (CCSDS 301.0-B-4,
//! epoch 1958-01-01 TAI) — F-GND-01, F-GND-11, SR-011.
//!
//! [`TmPacket`] is the only packet type CAELUM can build, and it is telemetry only: its sole
//! constructor writes the primary header through `SpacePacketHeader::new_for_unseg_tm`, and no
//! API in this crate takes a packet type, so a telecommand packet cannot be produced (ICD-VEH §7,
//! SR-011). Ground-side parsing ([`ParsedTm::parse`]) rejects telecommand packets.
//!
//! Layout (all big-endian):
//! - primary header, 6 octets: version 0, type 0 (TM), secondary header flag 1, APID (11 bits),
//!   sequence flags `11` (unsegmented), sequence count (14 bits), data length (octets − 1);
//! - secondary header, 7 octets: CUC P-field `0x1E` (CCSDS epoch, 4 coarse + 2 fine octets),
//!   TAI seconds since 1958-01-01, fraction in units of 2⁻¹⁶ s;
//! - user data.

use std::collections::BTreeMap;

use arbitrary_int::{u11, u14};
use caelum_domain::time::TaiNs;
use serde::{Deserialize, Serialize};
use spacepackets::time::cuc::CucTime;
use spacepackets::time::{TimeReader, TimeWriter};
use spacepackets::{CcsdsPacket, PacketType, SequenceFlags, SpacePacketHeader};

const NS_PER_S: i64 = 1_000_000_000;
/// Primary header length.
pub const PRIMARY_HEADER_LEN: usize = 6;
/// Secondary header length (CUC P-field + 4 coarse + 2 fine octets).
pub const SECONDARY_HEADER_LEN: usize = 7;
/// The CUC P-field: CCSDS epoch (level 1), 4 coarse octets, 2 fine octets.
pub const CUC_P_FIELD: u8 = 0x1E;
/// Largest user data field (packet data field ≤ 65 536 octets).
pub const MAX_USER_DATA: usize = 65_536 - SECONDARY_HEADER_LEN;
/// The idle-packet APID (all ones), never used for data.
pub const IDLE_APID: u16 = 0x7FF;

/// Packet errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PacketError {
    /// APID outside 0..=2046 (2047 is the idle APID).
    #[error("APID {0} is outside 0..=2046")]
    BadApid(u16),
    /// The time cannot be expressed as a 4-octet CUC from the 1958 epoch.
    #[error("time {0} ns is outside the CUC range (1958..2094)")]
    TimeOutOfRange(i64),
    /// User data too long for one packet.
    #[error("user data of {0} octets exceeds one packet")]
    TooLong(usize),
    /// Malformed packet bytes.
    #[error("malformed packet: {0}")]
    Malformed(String),
    /// A telecommand packet was offered where only telemetry is accepted.
    #[error("telecommand packets are refused")]
    Telecommand,
}

/// An application process identifier (0..=2046).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Apid(u16);

impl Apid {
    /// Validates an APID.
    pub fn new(value: u16) -> Result<Self, PacketError> {
        if value >= IDLE_APID {
            return Err(PacketError::BadApid(value));
        }
        Ok(Self(value))
    }

    /// The numeric value.
    pub const fn value(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for Apid {
    type Error = PacketError;
    fn try_from(v: u16) -> Result<Self, Self::Error> {
        Self::new(v)
    }
}

impl From<Apid> for u16 {
    fn from(a: Apid) -> u16 {
        a.0
    }
}

/// Per-APID source sequence counts (14 bits, wrapping), F-GND-01. The caller persists
/// [`Self::snapshot`] so counts continue after a restart.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeqCounters {
    next: BTreeMap<u16, u16>,
}

impl SeqCounters {
    /// Restores counters from a snapshot.
    pub fn restore(snapshot: BTreeMap<u16, u16>) -> Self {
        Self {
            next: snapshot.into_iter().map(|(a, c)| (a, c & 0x3FFF)).collect(),
        }
    }

    /// The counters to persist.
    pub fn snapshot(&self) -> BTreeMap<u16, u16> {
        self.next.clone()
    }

    /// Takes the next sequence count for an APID.
    pub fn next(&mut self, apid: Apid) -> u16 {
        let slot = self.next.entry(apid.value()).or_insert(0);
        let current = *slot;
        *slot = (current + 1) & 0x3FFF;
        current
    }
}

/// TAI time as CUC (coarse seconds since 1958, 16-bit fraction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cuc {
    /// TAI seconds since 1958-01-01.
    pub coarse: u32,
    /// Fraction of a second in units of 2⁻¹⁶ s.
    pub fine: u16,
}

impl Cuc {
    /// Converts TAI nanoseconds (truncating to the 2⁻¹⁶ s resolution).
    pub fn from_tai(t: TaiNs) -> Result<Self, PacketError> {
        let (s, ns) = t.split();
        let coarse = u32::try_from(s).map_err(|_| PacketError::TimeOutOfRange(t.0))?;
        // ns < 1e9, so ns << 16 fits i64 and the quotient is < 65 536.
        let fine =
            u16::try_from((ns << 16) / NS_PER_S).map_err(|_| PacketError::TimeOutOfRange(t.0))?;
        Ok(Self { coarse, fine })
    }

    /// Back to TAI nanoseconds. The fraction is rounded up to whole nanoseconds, so that
    /// `from_tai(to_tai(c)) == c` for every CUC value.
    pub fn to_tai(self) -> TaiNs {
        let scaled = i64::from(self.fine) * NS_PER_S;
        TaiNs(i64::from(self.coarse) * NS_PER_S + (scaled + 0xFFFF) / 0x1_0000)
    }

    /// The 6-octet T-field (no P-field), used inside user data.
    pub fn t_field(self) -> [u8; 6] {
        let mut out = [0u8; 6];
        out[..4].copy_from_slice(&self.coarse.to_be_bytes());
        out[4..].copy_from_slice(&self.fine.to_be_bytes());
        out
    }

    /// Reads a 6-octet T-field.
    pub fn from_t_field(b: &[u8]) -> Result<Self, PacketError> {
        let short = || PacketError::Malformed("CUC T-field needs 6 octets".into());
        let coarse = b.get(..4).ok_or_else(short)?;
        let fine = b.get(4..6).ok_or_else(short)?;
        Ok(Self {
            coarse: u32::from_be_bytes([coarse[0], coarse[1], coarse[2], coarse[3]]),
            fine: u16::from_be_bytes([fine[0], fine[1]]),
        })
    }
}

/// A telemetry Space Packet. There is no way to build a telecommand with this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmPacket {
    bytes: Vec<u8>,
}

impl TmPacket {
    /// Builds a telemetry packet: TM type bit fixed to 0, secondary header present, unsegmented.
    pub fn build(
        apid: Apid,
        seq_count: u16,
        time: TaiNs,
        user_data: &[u8],
    ) -> Result<Self, PacketError> {
        if user_data.len() > MAX_USER_DATA {
            return Err(PacketError::TooLong(user_data.len()));
        }
        let cuc = Cuc::from_tai(time)?;
        let data_field_len = SECONDARY_HEADER_LEN + user_data.len();
        let data_len =
            u16::try_from(data_field_len - 1).map_err(|_| PacketError::TooLong(user_data.len()))?;
        let apid11 = u11::try_new(apid.value()).map_err(|_| PacketError::BadApid(apid.value()))?;
        let seq = u14::try_new(seq_count & 0x3FFF)
            .map_err(|_| PacketError::Malformed("sequence count".into()))?;
        let mut header = SpacePacketHeader::new_for_unseg_tm(apid11, seq, data_len);
        header.set_sec_header_flag();
        let mut bytes = vec![0u8; PRIMARY_HEADER_LEN + data_field_len];
        header
            .write_to_be_bytes(&mut bytes[..PRIMARY_HEADER_LEN])
            .map_err(|e| PacketError::Malformed(e.to_string()))?;
        let time = CucTime::new_with_medium_fractions(cuc.coarse, cuc.fine);
        time.write_to_bytes(
            &mut bytes[PRIMARY_HEADER_LEN..PRIMARY_HEADER_LEN + SECONDARY_HEADER_LEN],
        )
        .map_err(|e| PacketError::Malformed(e.to_string()))?;
        bytes[PRIMARY_HEADER_LEN + SECONDARY_HEADER_LEN..].copy_from_slice(user_data);
        Ok(Self { bytes })
    }

    /// The encoded packet.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consumes the packet into its bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// A decoded telemetry packet (ground side, F-GND-08).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTm {
    /// APID.
    pub apid: Apid,
    /// Sequence count.
    pub seq_count: u16,
    /// Packet time (secondary header).
    pub time: TaiNs,
    /// User data.
    pub user_data: Vec<u8>,
}

impl ParsedTm {
    /// Parses one telemetry packet; returns it and the remaining bytes (a TCP stream carries
    /// packets back to back). Telecommands and packets without the CUC header are refused.
    pub fn parse(buf: &[u8]) -> Result<(Self, &[u8]), PacketError> {
        let (header, _) = SpacePacketHeader::from_be_bytes(buf)
            .map_err(|e| PacketError::Malformed(e.to_string()))?;
        if header.packet_type() != PacketType::Tm {
            return Err(PacketError::Telecommand);
        }
        if header.ccsds_version().value() != 0 {
            return Err(PacketError::Malformed("CCSDS version is not 0".into()));
        }
        if !header.sec_header_flag() {
            return Err(PacketError::Malformed("no secondary header".into()));
        }
        if header.sequence_flags() != SequenceFlags::Unsegmented {
            return Err(PacketError::Malformed(
                "segmented packets are not used".into(),
            ));
        }
        let total = header.packet_len();
        let packet = buf
            .get(..total)
            .ok_or_else(|| PacketError::Malformed(format!("needs {total} octets")))?;
        let sec = packet
            .get(PRIMARY_HEADER_LEN..PRIMARY_HEADER_LEN + SECONDARY_HEADER_LEN)
            .ok_or_else(|| PacketError::Malformed("short secondary header".into()))?;
        if sec[0] != CUC_P_FIELD {
            return Err(PacketError::Malformed(format!(
                "CUC P-field {:#04x}",
                sec[0]
            )));
        }
        let cuc = CucTime::from_bytes(sec).map_err(|e| PacketError::Malformed(e.to_string()))?;
        let fine = u16::try_from(cuc.fractions().counter())
            .map_err(|_| PacketError::Malformed("CUC fraction".into()))?;
        let time = Cuc {
            coarse: cuc.counter(),
            fine,
        }
        .to_tai();
        let apid = Apid::new(header.apid().value())?;
        Ok((
            Self {
                apid,
                seq_count: header.seq_count().value(),
                time,
                user_data: packet[PRIMARY_HEADER_LEN + SECONDARY_HEADER_LEN..].to_vec(),
            },
            &buf[total..],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn apid(v: u16) -> Apid {
        Apid::new(v).expect("valid apid")
    }

    /// F-GND-01, R-INT-01: primary header bit layout per CCSDS 133.0-B-2 §4.1.3, checked octet by
    /// octet.
    #[test]
    fn f_gnd_01_primary_header_layout_matches_the_standard() {
        let t = TaiNs(2_000_000_000 * NS_PER_S + NS_PER_S / 2);
        let p = TmPacket::build(apid(0x123), 0x1ABC, t, &[0xAA, 0xBB]).expect("builds");
        let b = p.bytes();
        // Version 000, type 0, sec hdr 1, APID 0x123 → 0b000_0_1_001 0010_0011.
        assert_eq!(b[0], 0b0000_1001);
        assert_eq!(b[1], 0x23);
        // Sequence flags 11, count 0x1ABC.
        assert_eq!(b[2], 0b1100_0000 | 0x1A);
        assert_eq!(b[3], 0xBC);
        // Data length = 7 + 2 − 1.
        assert_eq!(u16::from_be_bytes([b[4], b[5]]), 8);
        // CUC P-field, coarse seconds and half a second as 0x8000.
        assert_eq!(b[6], CUC_P_FIELD);
        assert_eq!(u32::from_be_bytes([b[7], b[8], b[9], b[10]]), 2_000_000_000);
        assert_eq!(u16::from_be_bytes([b[11], b[12]]), 0x8000);
        assert_eq!(&b[13..], &[0xAA, 0xBB]);
    }

    /// SR-011, F-GND-11: the publisher's packet type can never set the telecommand bit.
    #[test]
    fn sr_011_tm_packet_cannot_be_a_telecommand() {
        for a in [0u16, 1, 0x100, 2046] {
            let p = TmPacket::build(apid(a), 16_383, TaiNs(0), &[]).expect("builds");
            assert_eq!(p.bytes()[0] & 0b0001_0000, 0, "type bit must be TM");
        }
    }

    /// Ground side refuses telecommand packets.
    #[test]
    fn parser_refuses_telecommands() {
        let mut b = TmPacket::build(apid(5), 1, TaiNs(0), &[1])
            .expect("builds")
            .into_bytes();
        b[0] |= 0b0001_0000;
        assert_eq!(ParsedTm::parse(&b), Err(PacketError::Telecommand));
    }

    #[test]
    fn idle_and_out_of_range_apids_are_refused() {
        assert_eq!(Apid::new(2047), Err(PacketError::BadApid(2047)));
        assert_eq!(Apid::new(4000), Err(PacketError::BadApid(4000)));
        assert!(Apid::new(2046).is_ok());
    }

    #[test]
    fn times_before_1958_or_after_2094_are_refused() {
        assert!(matches!(
            TmPacket::build(apid(1), 0, TaiNs(-1), &[]),
            Err(PacketError::TimeOutOfRange(_))
        ));
        let late = TaiNs((i64::from(u32::MAX) + 1) * NS_PER_S);
        assert!(matches!(
            TmPacket::build(apid(1), 0, late, &[]),
            Err(PacketError::TimeOutOfRange(_))
        ));
    }

    /// F-GND-01: sequence counts are per APID and wrap at 2¹⁴.
    #[test]
    fn f_gnd_01_sequence_counts_are_per_apid_and_wrap() {
        let mut c = SeqCounters::default();
        assert_eq!(c.next(apid(1)), 0);
        assert_eq!(c.next(apid(1)), 1);
        assert_eq!(c.next(apid(2)), 0);
        let mut snap = c.snapshot();
        snap.insert(1, 16_383);
        let mut c = SeqCounters::restore(snap);
        assert_eq!(c.next(apid(1)), 16_383);
        assert_eq!(c.next(apid(1)), 0);
        assert_eq!(c.next(apid(2)), 1);
    }

    #[test]
    fn truncated_and_malformed_packets_are_rejected() {
        let b = TmPacket::build(apid(9), 3, TaiNs(5 * NS_PER_S), &[1, 2, 3])
            .expect("builds")
            .into_bytes();
        assert!(ParsedTm::parse(&b[..b.len() - 1]).is_err());
        let mut bad_p = b.clone();
        bad_p[6] = 0x2F;
        assert!(ParsedTm::parse(&bad_p).is_err());
        let mut no_sec = b.clone();
        no_sec[0] &= !0b0000_1000;
        assert!(ParsedTm::parse(&no_sec).is_err());
    }

    #[test]
    fn back_to_back_packets_parse_in_order() {
        let a = TmPacket::build(apid(1), 0, TaiNs(NS_PER_S), &[1]).expect("a");
        let b = TmPacket::build(apid(2), 7, TaiNs(2 * NS_PER_S), &[2, 2]).expect("b");
        let stream = [a.bytes(), b.bytes()].concat();
        let (pa, rest) = ParsedTm::parse(&stream).expect("first");
        let (pb, rest) = ParsedTm::parse(rest).expect("second");
        assert!(rest.is_empty());
        assert_eq!((pa.apid.value(), pa.user_data), (1, vec![1]));
        assert_eq!(
            (pb.apid.value(), pb.seq_count, pb.user_data),
            (2, 7, vec![2, 2])
        );
    }

    proptest! {
        /// F-GND-01: build → parse is the identity (time to CUC resolution), and the type bit is
        /// always TM (SR-011).
        #[test]
        fn f_gnd_01_round_trip(a in 0u16..2047, seq in 0u16..16_384, s in 0i64..4_000_000_000, frac in 0u16..=u16::MAX, data in proptest::collection::vec(any::<u8>(), 0..64)) {
            let t = Cuc { coarse: u32::try_from(s).unwrap_or(0), fine: frac }.to_tai();
            let p = TmPacket::build(apid(a), seq, t, &data).expect("builds");
            prop_assert_eq!(p.bytes()[0] & 0x10, 0);
            let (parsed, rest) = ParsedTm::parse(p.bytes()).expect("parses");
            prop_assert!(rest.is_empty());
            prop_assert_eq!(parsed.apid.value(), a);
            prop_assert_eq!(parsed.seq_count, seq);
            prop_assert_eq!(parsed.time, t);
            prop_assert_eq!(parsed.user_data, data);
        }

        /// CUC conversion error is below one fraction unit (≈ 15.3 µs).
        #[test]
        fn cuc_resolution(ns in 0i64..4_000_000_000 * NS_PER_S) {
            let back = Cuc::from_tai(TaiNs(ns)).expect("in range").to_tai();
            prop_assert!(back.0 <= ns && ns - back.0 < 15_259);
        }
    }
}
