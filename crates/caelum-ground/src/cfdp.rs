//! CCSDS File Delivery Protocol (CCSDS 727.0-B-5) — F-GND-06, F-GND-07 (uplink reception).
//!
//! A minimal compliant subset (CR-016 §3): class 1 (unacknowledged, with transaction closure so
//! the sender learns the outcome and the queue can start a new transaction) and class 2
//! (acknowledged) transactions between two entities, with the Suspend / Resume procedures
//! (727.0-B-5 §4.6.6–4.6.7) driven by contact windows. PDUs are encoded and decoded with the
//! stack crate `spacepackets` (normal file size, PDU CRC on, modular checksum, no segment
//! metadata, no filestore requests, no Keep Alive / Prompt / inactivity procedures).
//!
//! Sans-I/O: [`CfdpEntity::poll`] returns the PDUs to send within a budget, and
//! [`CfdpEntity::receive`] takes PDUs from the link. Timers (positive ACK, NAK) run on *active*
//! time only: while a transaction is suspended its timers are frozen, so a 30-day loss of signal
//! never exhausts a limit (F-GND-06).
//!
//! Incoming PDUs are parsed inside `catch_unwind`: a malformed PDU that trips an indexing bug in
//! the decoder is rejected as malformed instead of taking the process down (CR-016 §3).

use std::collections::{BTreeMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};

use caelum_domain::time::TaiNs;
use serde::{Deserialize, Serialize};
use spacepackets::cfdp::lv::Lv;
use spacepackets::cfdp::pdu::ack::AckPdu;
use spacepackets::cfdp::pdu::eof::EofPdu;
use spacepackets::cfdp::pdu::file_data::FileDataPdu;
use spacepackets::cfdp::pdu::finished::{
    DeliveryCode, FileStatus, FinishedPduCreator, FinishedPduReader,
};
use spacepackets::cfdp::pdu::metadata::{
    MetadataGenericParams, MetadataPduCreator, MetadataPduReader,
};
use spacepackets::cfdp::pdu::nak::{NakPduCreator, NakPduReader};
use spacepackets::cfdp::pdu::{CommonPduConfig, FileDirectiveType, PduHeader, WritablePduPacket};
use spacepackets::cfdp::{
    ChecksumType, ConditionCode, CrcFlag, Direction, LargeFileFlag, PduType, TransactionStatus,
    TransmissionMode,
};
use spacepackets::util::{UnsignedByteFieldU16, UnsignedByteFieldU32};

/// Most segment requests in one NAK PDU.
const MAX_NAK_SEGMENTS: usize = 64;

/// CFDP errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CfdpError {
    /// PDU encoding failed.
    #[error("PDU encoding failed: {0}")]
    Encode(String),
    /// A received PDU is malformed or not for this entity.
    #[error("rejected PDU: {0}")]
    Rejected(String),
    /// Configuration is invalid.
    #[error("invalid CFDP configuration: {0}")]
    Config(String),
    /// No such transaction.
    #[error("unknown transaction")]
    UnknownTransaction,
}

/// Transmission class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CfdpClass {
    /// Unacknowledged.
    Class1,
    /// Acknowledged (retransmission of lost segments, closure).
    Class2,
}

/// Local MIB parameters. Every value is explicit (no defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CfdpConfig {
    /// This entity.
    pub local_entity: u16,
    /// The peer entity.
    pub remote_entity: u16,
    /// File data octets per File Data PDU.
    pub segment_len: usize,
    /// Positive ACK timer (active time, ns).
    pub ack_timer_ns: i64,
    /// Positive ACK limit (expirations before the transaction is cancelled).
    pub ack_limit: u32,
    /// NAK timer (active time, ns).
    pub nak_timer_ns: i64,
    /// NAK limit.
    pub nak_limit: u32,
}

impl CfdpConfig {
    /// Validates the parameters.
    pub fn validate(&self) -> Result<(), CfdpError> {
        if self.local_entity == self.remote_entity {
            return Err(CfdpError::Config("entity IDs must differ".into()));
        }
        if self.segment_len == 0 || self.segment_len > 4096 {
            return Err(CfdpError::Config("segment length must be 1..=4096".into()));
        }
        if self.ack_timer_ns <= 0
            || self.nak_timer_ns <= 0
            || self.ack_limit == 0
            || self.nak_limit == 0
        {
            return Err(CfdpError::Config(
                "timers and limits must be positive".into(),
            ));
        }
        Ok(())
    }
}

/// Transaction identifier (source entity, sequence number).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TransactionId {
    /// Source entity.
    pub source: u16,
    /// Transaction sequence number.
    pub seq: u32,
}

/// Transaction state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxState {
    /// Running.
    Active,
    /// Suspended (loss of signal).
    Suspended,
    /// Completed successfully.
    Completed,
    /// Cancelled with a CFDP condition code (raw value).
    Cancelled(u8),
}

/// Events for the user (727.0-B-5 §3.5 indications, reduced).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfdpEvent {
    /// A sent file completed.
    Sent(TransactionId),
    /// A transaction was cancelled (condition code).
    Cancelled(TransactionId, u8),
    /// A file was received completely and its checksum verified.
    Received {
        /// Transaction.
        id: TransactionId,
        /// Destination file name.
        name: String,
        /// Content.
        data: Vec<u8>,
    },
}

/// The CFDP modular checksum (727.0-B-5 §4.2.2): sum of the file's 4-octet big-endian words,
/// aligned to the file start and zero-padded, modulo 2³².
pub fn modular_checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |acc, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        acc.wrapping_add(u32::from_be_bytes(w))
    })
}

/// A countdown that advances only while the transaction is active.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct ActiveClock {
    active_ns: i64,
    last: Option<i64>,
}

impl ActiveClock {
    fn advance(&mut self, now: TaiNs, suspended: bool) {
        if let Some(last) = self.last {
            if !suspended && now.0 > last {
                self.active_ns += now.0 - last;
            }
        }
        self.last = Some(now.0);
    }
}

fn conf(
    id: TransactionId,
    source: u16,
    dest: u16,
    class: CfdpClass,
) -> Result<CommonPduConfig, CfdpError> {
    CommonPduConfig::new(
        UnsignedByteFieldU16::new(source),
        UnsignedByteFieldU16::new(dest),
        UnsignedByteFieldU32::new(id.seq),
        match class {
            CfdpClass::Class1 => TransmissionMode::Unacknowledged,
            CfdpClass::Class2 => TransmissionMode::Acknowledged,
        },
        LargeFileFlag::Normal,
        CrcFlag::WithCrc,
        Direction::TowardsReceiver,
    )
    .map_err(|e| CfdpError::Encode(e.to_string()))
}

fn enc<P: WritablePduPacket>(p: &P) -> Result<Vec<u8>, CfdpError> {
    p.to_vec().map_err(|e| CfdpError::Encode(e.to_string()))
}

/// The sending side of a transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTransaction {
    id: TransactionId,
    class: CfdpClass,
    cfg: CfdpConfig,
    name: String,
    #[serde(skip)]
    data: Vec<u8>,
    file_size: u64,
    checksum: u32,
    metadata_sent: bool,
    resend_metadata: bool,
    next_offset: u64,
    retransmit: VecDeque<(u64, u64)>,
    eof_sent: bool,
    eof_acked: bool,
    eof_due_at: Option<i64>,
    ack_count: u32,
    ack_finished: bool,
    finished_condition: Option<u8>,
    clock: ActiveClock,
    state: TxState,
}

impl SourceTransaction {
    /// Transaction ID.
    pub fn id(&self) -> TransactionId {
        self.id
    }

    /// State.
    pub fn state(&self) -> TxState {
        self.state
    }

    /// Re-attaches the file content after a restore (content is not part of the snapshot).
    pub fn restore_data(&mut self, data: Vec<u8>) -> Result<(), CfdpError> {
        if data.len() as u64 != self.file_size || modular_checksum(&data) != self.checksum {
            return Err(CfdpError::Rejected(
                "restored content differs from the file sent".into(),
            ));
        }
        self.data = data;
        Ok(())
    }

    fn suspended(&self) -> bool {
        self.state == TxState::Suspended
    }

    fn header(&self, directive: bool, len: u16) -> Result<PduHeader, CfdpError> {
        let c = conf(
            self.id,
            self.cfg.local_entity,
            self.cfg.remote_entity,
            self.class,
        )?;
        Ok(if directive {
            PduHeader::new_for_file_directive(c, len)
        } else {
            PduHeader::new_for_file_data_default(c, len)
        })
    }

    fn file_data(&self, start: u64, end: u64) -> Result<Vec<u8>, CfdpError> {
        let s = usize::try_from(start).map_err(|_| CfdpError::Encode("offset".into()))?;
        let e = usize::try_from(end).map_err(|_| CfdpError::Encode("offset".into()))?;
        let chunk = self
            .data
            .get(s..e)
            .ok_or_else(|| CfdpError::Encode("file content missing (not restored?)".into()))?;
        enc(&FileDataPdu::new_no_seg_metadata(
            self.header(false, 0)?,
            start,
            chunk,
        ))
    }

    fn poll(
        &mut self,
        now: TaiNs,
        budget: &mut usize,
        events: &mut Vec<CfdpEvent>,
    ) -> Result<Vec<Vec<u8>>, CfdpError> {
        self.clock.advance(now, self.suspended());
        let mut out = Vec::new();
        if self.state != TxState::Active {
            return Ok(out);
        }
        let seg = self.cfg.segment_len as u64;
        while *budget > 0 {
            if !self.metadata_sent || self.resend_metadata {
                // Class 1 always requests transaction closure, so the sender learns the outcome.
                let params =
                    MetadataGenericParams::new(true, ChecksumType::Modular, self.file_size);
                let name =
                    Lv::new_from_str(&self.name).map_err(|e| CfdpError::Encode(e.to_string()))?;
                out.push(enc(&MetadataPduCreator::new_no_opts(
                    self.header(true, 0)?,
                    params,
                    name,
                    name,
                ))?);
                self.metadata_sent = true;
                self.resend_metadata = false;
            } else if let Some((start, end)) = self.retransmit.pop_front() {
                let stop = end.min(start + seg);
                out.push(self.file_data(start, stop)?);
                if stop < end {
                    self.retransmit.push_front((stop, end));
                }
            } else if self.next_offset < self.file_size {
                let stop = self.file_size.min(self.next_offset + seg);
                out.push(self.file_data(self.next_offset, stop)?);
                self.next_offset = stop;
            } else if !self.eof_sent || self.eof_due_at.is_some_and(|d| self.clock.active_ns >= d) {
                if self.eof_sent {
                    self.ack_count += 1;
                    if self.ack_count > self.cfg.ack_limit {
                        let code = match self.class {
                            CfdpClass::Class1 => ConditionCode::CheckLimitReached as u8,
                            CfdpClass::Class2 => ConditionCode::PositiveAckLimitReached as u8,
                        };
                        self.state = TxState::Cancelled(code);
                        events.push(CfdpEvent::Cancelled(self.id, code));
                        break;
                    }
                }
                out.push(enc(&EofPdu::new_no_error(
                    self.header(true, 0)?,
                    self.checksum,
                    self.file_size,
                ))?);
                self.eof_sent = true;
                match self.class {
                    // Class 1 with closure: wait for the Finished PDU (check timer).
                    CfdpClass::Class1 if self.finished_condition.is_none() => {
                        self.eof_due_at = Some(self.clock.active_ns + self.cfg.ack_timer_ns);
                    }
                    CfdpClass::Class1 => {}
                    CfdpClass::Class2 if !self.eof_acked => {
                        self.eof_due_at = Some(self.clock.active_ns + self.cfg.ack_timer_ns);
                    }
                    CfdpClass::Class2 => {}
                }
            } else if self.ack_finished {
                if self.class == CfdpClass::Class2 {
                    let ack = AckPdu::new(
                        self.header(true, 0)?,
                        FileDirectiveType::Finished,
                        ConditionCode::NoError,
                        TransactionStatus::Terminated,
                    )
                    .map_err(|e| CfdpError::Encode(e.to_string()))?;
                    out.push(enc(&ack)?);
                }
                self.ack_finished = false;
                match self.finished_condition {
                    Some(0) => {
                        self.state = TxState::Completed;
                        events.push(CfdpEvent::Sent(self.id));
                    }
                    Some(code) => {
                        self.state = TxState::Cancelled(code);
                        events.push(CfdpEvent::Cancelled(self.id, code));
                    }
                    None => {}
                }
                break;
            } else {
                break;
            }
            *budget -= 1;
        }
        Ok(out)
    }

    fn on_pdu(&mut self, pdu: &Parsed) {
        if matches!(self.state, TxState::Completed | TxState::Cancelled(_)) {
            // A repeated Finished after completion still needs an ACK (lost ACK).
            if let Parsed::Finished { .. } = pdu {
                if self.state == TxState::Completed && self.class == CfdpClass::Class2 {
                    self.ack_finished = true;
                    self.state = TxState::Active;
                }
            }
            return;
        }
        match pdu {
            Parsed::AckEof => {
                self.eof_acked = true;
                self.eof_due_at = None;
            }
            Parsed::Nak { segments } => {
                for &(start, end) in segments {
                    if start == 0 && end == 0 {
                        self.resend_metadata = true;
                    } else if start < end && end <= self.file_size {
                        self.retransmit.push_back((start, end));
                    }
                }
            }
            Parsed::Finished { condition } => {
                self.eof_acked = true;
                self.eof_due_at = None;
                self.finished_condition = Some(*condition);
                self.ack_finished = true;
            }
            _ => {}
        }
    }
}

/// The receiving side of a transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestTransaction {
    id: TransactionId,
    class: CfdpClass,
    cfg: CfdpConfig,
    name: Option<String>,
    data: Vec<u8>,
    ranges: Vec<(u64, u64)>,
    eof: Option<(u32, u64)>,
    ack_eof_due: bool,
    nak_due_at: Option<i64>,
    nak_count: u32,
    finished_sent: bool,
    finished_due_at: Option<i64>,
    finished_count: u32,
    finished_condition: u8,
    clock: ActiveClock,
    state: TxState,
}

impl DestTransaction {
    fn new(id: TransactionId, class: CfdpClass, cfg: CfdpConfig) -> Self {
        Self {
            id,
            class,
            cfg,
            name: None,
            data: Vec::new(),
            ranges: Vec::new(),
            eof: None,
            ack_eof_due: false,
            nak_due_at: None,
            nak_count: 0,
            finished_sent: false,
            finished_due_at: None,
            finished_count: 0,
            finished_condition: 0,
            clock: ActiveClock::default(),
            state: TxState::Active,
        }
    }

    /// State.
    pub fn state(&self) -> TxState {
        self.state
    }

    fn insert(&mut self, offset: u64, chunk: &[u8]) -> Result<(), CfdpError> {
        let end = offset + chunk.len() as u64;
        let (s, e) = (
            usize::try_from(offset).map_err(|_| CfdpError::Rejected("offset".into()))?,
            usize::try_from(end).map_err(|_| CfdpError::Rejected("offset".into()))?,
        );
        if self.data.len() < e {
            self.data.resize(e, 0);
        }
        self.data[s..e].copy_from_slice(chunk);
        self.ranges.push((offset, end));
        self.ranges.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for (a, b) in self.ranges.drain(..) {
            match merged.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        self.ranges = merged;
        Ok(())
    }

    fn missing(&self, size: u64) -> Vec<(u64, u64)> {
        let mut gaps = Vec::new();
        let mut at = 0;
        for &(a, b) in &self.ranges {
            if a > at {
                gaps.push((at, a.min(size)));
            }
            at = at.max(b);
        }
        if at < size {
            gaps.push((at, size));
        }
        if self.name.is_none() {
            gaps.insert(0, (0, 0));
        }
        gaps.truncate(MAX_NAK_SEGMENTS);
        gaps
    }

    fn header(&self) -> Result<PduHeader, CfdpError> {
        let c = conf(self.id, self.id.source, self.cfg.local_entity, self.class)?;
        Ok(PduHeader::new_for_file_directive(c, 0))
    }

    fn finish(&mut self, condition: u8, events: &mut Vec<CfdpEvent>) {
        self.finished_condition = condition;
        if condition == 0 {
            events.push(CfdpEvent::Received {
                id: self.id,
                name: self.name.clone().unwrap_or_default(),
                data: std::mem::take(&mut self.data),
            });
        } else {
            events.push(CfdpEvent::Cancelled(self.id, condition));
        }
        match self.class {
            CfdpClass::Class1 => {
                self.state = if condition == 0 {
                    TxState::Completed
                } else {
                    TxState::Cancelled(condition)
                };
            }
            CfdpClass::Class2 => self.finished_sent = false,
        }
    }

    fn on_pdu(&mut self, pdu: Parsed, events: &mut Vec<CfdpEvent>) -> Result<(), CfdpError> {
        match pdu {
            Parsed::Metadata { name } => self.name = Some(name),
            Parsed::FileData { offset, data } => {
                if self.finished_due_at.is_none() && !self.finished_sent {
                    self.insert(offset, &data)?;
                }
            }
            Parsed::Eof { checksum, size } => {
                self.eof = Some((checksum, size));
                if self.class == CfdpClass::Class2 {
                    self.ack_eof_due = true;
                }
                if self.nak_due_at.is_none() {
                    self.nak_due_at = Some(self.clock.active_ns);
                }
            }
            Parsed::AckFinished
                if self.class == CfdpClass::Class2
                    && (self.finished_sent || self.finished_due_at.is_some()) =>
            {
                self.state = if self.finished_condition == 0 {
                    TxState::Completed
                } else {
                    TxState::Cancelled(self.finished_condition)
                };
                self.finished_due_at = None;
            }
            _ => {}
        }
        let _ = events;
        Ok(())
    }

    fn poll(
        &mut self,
        now: TaiNs,
        budget: &mut usize,
        events: &mut Vec<CfdpEvent>,
    ) -> Result<Vec<Vec<u8>>, CfdpError> {
        self.clock.advance(now, self.state == TxState::Suspended);
        let mut out = Vec::new();
        if self.state != TxState::Active || *budget == 0 {
            return Ok(out);
        }
        if self.ack_eof_due {
            let ack = AckPdu::new(
                self.header()?,
                FileDirectiveType::Eof,
                ConditionCode::NoError,
                TransactionStatus::Active,
            )
            .map_err(|e| CfdpError::Encode(e.to_string()))?;
            out.push(enc(&ack)?);
            self.ack_eof_due = false;
            *budget -= 1;
        }
        let Some((checksum, size)) = self.eof else {
            return Ok(out);
        };
        // Finished already decided: (re)send it on the ACK timer (class 2).
        if self.finished_sent || self.finished_due_at.is_some() {
            if self
                .finished_due_at
                .is_some_and(|d| self.clock.active_ns >= d)
                && *budget > 0
            {
                self.finished_count += 1;
                if self.finished_count > self.cfg.ack_limit {
                    // The file was delivered; the source will time out on its own.
                    self.state = TxState::Completed;
                    return Ok(out);
                }
                out.push(self.finished_pdu()?);
                self.finished_due_at = Some(self.clock.active_ns + self.cfg.ack_timer_ns);
                *budget -= 1;
            }
            return Ok(out);
        }
        let gaps = self.missing(size);
        if gaps.is_empty() {
            self.data
                .truncate(usize::try_from(size).map_err(|_| CfdpError::Rejected("size".into()))?);
            let condition = if modular_checksum(&self.data) == checksum {
                0
            } else {
                ConditionCode::FileChecksumFailure as u8
            };
            self.finish(condition, events);
            if self.class == CfdpClass::Class1 {
                out.push(self.finished_pdu()?);
                *budget = budget.saturating_sub(1);
            } else if *budget > 0 {
                out.push(self.finished_pdu()?);
                self.finished_sent = true;
                self.finished_due_at = Some(self.clock.active_ns + self.cfg.ack_timer_ns);
                *budget -= 1;
            }
            return Ok(out);
        }
        match self.class {
            CfdpClass::Class1 => {
                // Unacknowledged: missing data cannot be recovered; closure reports it so the
                // sender can start a new transaction.
                self.finish(ConditionCode::FileSizeError as u8, events);
                out.push(self.finished_pdu()?);
                *budget = budget.saturating_sub(1);
            }
            CfdpClass::Class2 => {
                if self.nak_due_at.is_some_and(|d| self.clock.active_ns >= d) && *budget > 0 {
                    self.nak_count += 1;
                    if self.nak_count > self.cfg.nak_limit {
                        let code = ConditionCode::NakLimitReached as u8;
                        self.state = TxState::Cancelled(code);
                        events.push(CfdpEvent::Cancelled(self.id, code));
                        return Ok(out);
                    }
                    let pairs: Vec<(u32, u32)> = gaps
                        .iter()
                        .map(|&(a, b)| {
                            (
                                u32::try_from(a).unwrap_or(u32::MAX),
                                u32::try_from(b).unwrap_or(u32::MAX),
                            )
                        })
                        .collect();
                    let end = u32::try_from(size).map_err(|_| CfdpError::Encode("size".into()))?;
                    let nak = NakPduCreator::new_normal_file_size(self.header()?, 0, end, &pairs)
                        .map_err(|e| CfdpError::Encode(format!("{e:?}")))?;
                    out.push(enc(&nak)?);
                    self.nak_due_at = Some(self.clock.active_ns + self.cfg.nak_timer_ns);
                    *budget -= 1;
                }
            }
        }
        Ok(out)
    }

    fn finished_pdu(&self) -> Result<Vec<u8>, CfdpError> {
        let condition = ConditionCode::try_from(self.finished_condition)
            .map_err(|_| CfdpError::Encode("condition".into()))?;
        let (delivery, status) = if self.finished_condition == 0 {
            (DeliveryCode::Complete, FileStatus::Retained)
        } else {
            (DeliveryCode::Incomplete, FileStatus::DiscardDeliberately)
        };
        enc(&FinishedPduCreator::new(
            self.header()?,
            condition,
            delivery,
            status,
            &[],
            None,
        ))
    }
}

/// A decoded PDU, reduced to what the procedures need.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Parsed {
    Metadata { name: String },
    FileData { offset: u64, data: Vec<u8> },
    Eof { checksum: u32, size: u64 },
    AckEof,
    AckFinished,
    Nak { segments: Vec<(u64, u64)> },
    Finished { condition: u8 },
}

/// Parses a PDU: (transaction, class, direction towards receiver, content).
fn parse(buf: &[u8]) -> Result<(TransactionId, u16, CfdpClass, Parsed), CfdpError> {
    let rej = |e: String| CfdpError::Rejected(e);
    let (header, header_len) = PduHeader::from_bytes(buf).map_err(|e| rej(e.to_string()))?;
    let c = header.common_pdu_conf();
    let source = u16::try_from(c.source_id().value()).map_err(|_| rej("source id".into()))?;
    let dest = u16::try_from(c.dest_id().value()).map_err(|_| rej("dest id".into()))?;
    let id = TransactionId {
        source,
        seq: u32::try_from(c.transaction_seq_num.value()).map_err(|_| rej("seq".into()))?,
    };
    let class = match c.trans_mode {
        TransmissionMode::Acknowledged => CfdpClass::Class2,
        TransmissionMode::Unacknowledged => CfdpClass::Class1,
    };
    let content = if header.pdu_type() == PduType::FileData {
        let fd = FileDataPdu::from_bytes(buf).map_err(|e| rej(e.to_string()))?;
        Parsed::FileData {
            offset: fd.offset(),
            data: fd.file_data().to_vec(),
        }
    } else {
        let code = *buf
            .get(header_len)
            .ok_or_else(|| rej("no directive code".into()))?;
        let directive =
            FileDirectiveType::try_from(code).map_err(|_| rej(format!("directive {code:#x}")))?;
        match directive {
            FileDirectiveType::Metadata => {
                let m = MetadataPduReader::from_bytes(buf).map_err(|e| rej(e.to_string()))?;
                let name = m
                    .dest_file_name()
                    .value_as_str()
                    .and_then(Result::ok)
                    .ok_or_else(|| rej("file name".into()))?
                    .to_owned();
                Parsed::Metadata { name }
            }
            FileDirectiveType::Eof => {
                let e = EofPdu::from_bytes(buf).map_err(|e| rej(e.to_string()))?;
                Parsed::Eof {
                    checksum: e.file_checksum(),
                    size: e.file_size(),
                }
            }
            FileDirectiveType::Ack => {
                let a = AckPdu::from_bytes(buf).map_err(|e| rej(e.to_string()))?;
                match a.directive_code_of_acked_pdu() {
                    FileDirectiveType::Eof => Parsed::AckEof,
                    FileDirectiveType::Finished => Parsed::AckFinished,
                    other => return Err(rej(format!("ACK of {other:?}"))),
                }
            }
            FileDirectiveType::Nak => {
                let n = NakPduReader::from_bytes(buf).map_err(|e| rej(e.to_string()))?;
                Parsed::Nak {
                    segments: n
                        .get_segment_requests_iterator()
                        .map(|i| i.collect())
                        .unwrap_or_default(),
                }
            }
            FileDirectiveType::Finished => {
                let f = FinishedPduReader::from_bytes(buf).map_err(|e| rej(e.to_string()))?;
                Parsed::Finished {
                    condition: f.condition_code() as u8,
                }
            }
            other => return Err(rej(format!("{other:?} PDUs are not supported"))),
        }
    };
    Ok((id, dest, class, content))
}

fn parse_guarded(buf: &[u8]) -> Result<(TransactionId, u16, CfdpClass, Parsed), CfdpError> {
    catch_unwind(AssertUnwindSafe(|| parse(buf))).unwrap_or_else(|_| {
        Err(CfdpError::Rejected(
            "PDU decoder fault on malformed input".into(),
        ))
    })
}

/// Persistable state of an entity (file content of outgoing files is re-attached on restore).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntitySnapshot {
    next_seq: u32,
    sources: Vec<SourceTransaction>,
    dests: Vec<DestTransaction>,
}

/// A CFDP entity with its transactions.
#[derive(Debug, Clone)]
pub struct CfdpEntity {
    cfg: CfdpConfig,
    next_seq: u32,
    sources: BTreeMap<TransactionId, SourceTransaction>,
    dests: BTreeMap<TransactionId, DestTransaction>,
    events: Vec<CfdpEvent>,
}

impl CfdpEntity {
    /// A new entity; `first_seq` continues the sequence numbers persisted by the caller.
    pub fn new(cfg: CfdpConfig, first_seq: u32) -> Result<Self, CfdpError> {
        cfg.validate()?;
        Ok(Self {
            cfg,
            next_seq: first_seq,
            sources: BTreeMap::new(),
            dests: BTreeMap::new(),
            events: Vec::new(),
        })
    }

    /// Restores an entity; outgoing files need [`Self::restore_data`] before they can continue.
    pub fn restore(cfg: CfdpConfig, snapshot: EntitySnapshot) -> Result<Self, CfdpError> {
        let mut e = Self::new(cfg, snapshot.next_seq)?;
        e.sources = snapshot.sources.into_iter().map(|s| (s.id, s)).collect();
        e.dests = snapshot.dests.into_iter().map(|d| (d.id, d)).collect();
        Ok(e)
    }

    /// State to persist.
    pub fn snapshot(&self) -> EntitySnapshot {
        EntitySnapshot {
            next_seq: self.next_seq,
            sources: self.sources.values().cloned().collect(),
            dests: self.dests.values().cloned().collect(),
        }
    }

    /// Re-attaches an outgoing file's content after a restore.
    pub fn restore_data(&mut self, id: TransactionId, data: Vec<u8>) -> Result<(), CfdpError> {
        self.sources
            .get_mut(&id)
            .ok_or(CfdpError::UnknownTransaction)?
            .restore_data(data)
    }

    /// Put.request: starts sending a file to the peer.
    pub fn put(
        &mut self,
        class: CfdpClass,
        name: &str,
        data: Vec<u8>,
    ) -> Result<TransactionId, CfdpError> {
        if name.is_empty() || name.len() > 255 {
            return Err(CfdpError::Config("file name must be 1..=255 octets".into()));
        }
        let file_size = data.len() as u64;
        if file_size > u64::from(u32::MAX) {
            return Err(CfdpError::Config("large files are not supported".into()));
        }
        let id = TransactionId {
            source: self.cfg.local_entity,
            seq: self.next_seq,
        };
        self.next_seq = self.next_seq.wrapping_add(1);
        self.sources.insert(
            id,
            SourceTransaction {
                id,
                class,
                cfg: self.cfg,
                name: name.to_owned(),
                checksum: modular_checksum(&data),
                data,
                file_size,
                metadata_sent: false,
                resend_metadata: false,
                next_offset: 0,
                retransmit: VecDeque::new(),
                eof_sent: false,
                eof_acked: false,
                eof_due_at: None,
                ack_count: 0,
                ack_finished: false,
                finished_condition: None,
                clock: ActiveClock::default(),
                state: TxState::Active,
            },
        );
        Ok(id)
    }

    /// State of an outgoing transaction.
    pub fn source_state(&self, id: TransactionId) -> Option<TxState> {
        self.sources.get(&id).map(|s| s.state)
    }

    /// Suspend.request for every active transaction (loss of signal).
    pub fn suspend_all(&mut self, now: TaiNs) {
        for s in self.sources.values_mut() {
            s.clock.advance(now, s.suspended());
            if s.state == TxState::Active {
                s.state = TxState::Suspended;
            }
        }
        for d in self.dests.values_mut() {
            d.clock.advance(now, d.state == TxState::Suspended);
            if d.state == TxState::Active {
                d.state = TxState::Suspended;
            }
        }
    }

    /// Resume.request for every suspended transaction (acquisition of signal).
    pub fn resume_all(&mut self, now: TaiNs) {
        for s in self.sources.values_mut() {
            s.clock.advance(now, s.suspended());
            if s.state == TxState::Suspended {
                s.state = TxState::Active;
            }
        }
        for d in self.dests.values_mut() {
            d.clock.advance(now, d.state == TxState::Suspended);
            if d.state == TxState::Suspended {
                d.state = TxState::Active;
            }
        }
    }

    /// Whether any transaction is suspended.
    pub fn any_suspended(&self) -> bool {
        self.sources.values().any(|s| s.state == TxState::Suspended)
            || self.dests.values().any(|d| d.state == TxState::Suspended)
    }

    /// PDUs to send now, at most `budget`.
    pub fn poll(&mut self, now: TaiNs, mut budget: usize) -> Result<Vec<Vec<u8>>, CfdpError> {
        let mut out = Vec::new();
        for s in self.sources.values_mut() {
            out.extend(s.poll(now, &mut budget, &mut self.events)?);
        }
        for d in self.dests.values_mut() {
            out.extend(d.poll(now, &mut budget, &mut self.events)?);
        }
        Ok(out)
    }

    /// Handles a PDU from the peer. PDUs for another entity or a suspended transaction are
    /// refused (a suspended transaction ignores PDUs, 727.0-B-5 §4.6.6).
    pub fn receive(&mut self, now: TaiNs, buf: &[u8]) -> Result<(), CfdpError> {
        let (id, dest, class, content) = parse_guarded(buf)?;
        if id.source == self.cfg.local_entity {
            // A response to one of our transactions.
            let s = self
                .sources
                .get_mut(&id)
                .ok_or(CfdpError::UnknownTransaction)?;
            s.clock.advance(now, s.suspended());
            if s.state == TxState::Suspended {
                return Err(CfdpError::Rejected("transaction suspended".into()));
            }
            s.on_pdu(&content);
            return Ok(());
        }
        if dest != self.cfg.local_entity || id.source != self.cfg.remote_entity {
            return Err(CfdpError::Rejected("PDU for another entity".into()));
        }
        let d = self
            .dests
            .entry(id)
            .or_insert_with(|| DestTransaction::new(id, class, self.cfg));
        d.clock.advance(now, d.state == TxState::Suspended);
        if d.state == TxState::Suspended {
            return Err(CfdpError::Rejected("transaction suspended".into()));
        }
        if d.class != class {
            return Err(CfdpError::Rejected("transmission mode changed".into()));
        }
        d.on_pdu(content, &mut self.events)
    }

    /// Indications since the last call.
    pub fn take_events(&mut self) -> Vec<CfdpEvent> {
        std::mem::take(&mut self.events)
    }

    /// Forgets finished transactions (after the caller recorded their outcome).
    pub fn prune(&mut self) {
        let done = |s: TxState| matches!(s, TxState::Completed | TxState::Cancelled(_));
        self.sources.retain(|_, s| !done(s.state));
        self.dests.retain(|_, d| !done(d.state));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const S: i64 = 1_000_000_000;

    pub(crate) fn cfg(local: u16, remote: u16) -> CfdpConfig {
        CfdpConfig {
            local_entity: local,
            remote_entity: remote,
            segment_len: 100,
            ack_timer_ns: 10 * S,
            ack_limit: 3,
            nak_timer_ns: 10 * S,
            nak_limit: 5,
        }
    }

    fn file(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 % 251) as u8).collect()
    }

    /// Runs both entities over a link that drops PDUs for which `drop` returns true.
    fn run(
        tx: &mut CfdpEntity,
        rx: &mut CfdpEntity,
        start: i64,
        steps: usize,
        mut drop: impl FnMut(usize) -> bool,
    ) -> Vec<CfdpEvent> {
        let mut events = Vec::new();
        let mut n = 0;
        for step in 0..steps {
            let now = TaiNs(start + step as i64 * S);
            for pdu in tx.poll(now, 50).expect("poll") {
                n += 1;
                if !drop(n) {
                    let _ = rx.receive(now, &pdu);
                }
            }
            for pdu in rx.poll(now, 50).expect("poll") {
                n += 1;
                if !drop(n) {
                    let _ = tx.receive(now, &pdu);
                }
            }
            events.extend(tx.take_events());
            events.extend(rx.take_events());
        }
        events
    }

    #[test]
    fn modular_checksum_matches_the_standard_definition() {
        assert_eq!(modular_checksum(&[]), 0);
        assert_eq!(modular_checksum(&[0, 0, 0, 1, 0, 0, 0, 2]), 3);
        // Trailing partial word is zero-padded on the right.
        assert_eq!(modular_checksum(&[1]), 0x0100_0000);
        assert_eq!(modular_checksum(&[0xFF; 8]), 0xFFFF_FFFE);
    }

    /// F-GND-06: a class 1 transfer delivers the file over a clean link.
    #[test]
    fn f_gnd_06_class1_transfer_completes() {
        let mut tx = CfdpEntity::new(cfg(10, 1), 0).expect("tx");
        let mut rx = CfdpEntity::new(cfg(1, 10), 0).expect("rx");
        let data = file(1_234);
        let id = tx
            .put(CfdpClass::Class1, "ecg/evt-1.edf", data.clone())
            .expect("put");
        let ev = run(&mut tx, &mut rx, 0, 5, |_| false);
        assert!(ev.contains(&CfdpEvent::Sent(id)));
        assert!(ev.contains(&CfdpEvent::Received {
            id,
            name: "ecg/evt-1.edf".into(),
            data
        }));
    }

    /// F-GND-06: class 2 recovers lost File Data, EOF, ACK and Finished PDUs.
    #[test]
    fn f_gnd_06_class2_recovers_losses() {
        let mut tx = CfdpEntity::new(cfg(10, 1), 7).expect("tx");
        let mut rx = CfdpEntity::new(cfg(1, 10), 0).expect("rx");
        let data = file(2_000);
        let id = tx
            .put(CfdpClass::Class2, "dicom/series.dcm", data.clone())
            .expect("put");
        assert_eq!(id.seq, 7);
        // Drop the metadata, every third PDU early on, and some later responses.
        let ev = run(&mut tx, &mut rx, 0, 200, |n| {
            n == 1 || (n < 40 && n % 3 == 0) || n == 45
        });
        assert!(ev.contains(&CfdpEvent::Sent(id)), "{ev:?}");
        let received: Vec<&CfdpEvent> = ev
            .iter()
            .filter(|e| matches!(e, CfdpEvent::Received { .. }))
            .collect();
        assert_eq!(received.len(), 1, "delivered exactly once");
        assert!(matches!(received[0], CfdpEvent::Received { data: d, .. } if *d == data));
        assert_eq!(tx.source_state(id), Some(TxState::Completed));
    }

    /// F-GND-06: class 1 with loss reports an incomplete file; nothing is fabricated.
    #[test]
    fn class1_loss_is_reported_not_hidden() {
        let mut tx = CfdpEntity::new(cfg(10, 1), 0).expect("tx");
        let mut rx = CfdpEntity::new(cfg(1, 10), 0).expect("rx");
        let id = tx.put(CfdpClass::Class1, "f", file(500)).expect("put");
        let ev = run(&mut tx, &mut rx, 0, 5, |n| n == 3);
        assert!(ev.contains(&CfdpEvent::Cancelled(
            id,
            ConditionCode::FileSizeError as u8
        )));
        assert!(!ev.iter().any(|e| matches!(e, CfdpEvent::Received { .. })));
    }

    /// The positive ACK limit cancels a class 2 transaction whose peer never answers.
    #[test]
    fn class2_ack_limit_cancels_without_a_peer() {
        let mut tx = CfdpEntity::new(cfg(10, 1), 0).expect("tx");
        let mut rx = CfdpEntity::new(cfg(1, 10), 0).expect("rx");
        let id = tx.put(CfdpClass::Class2, "f", file(50)).expect("put");
        let ev = run(&mut tx, &mut rx, 0, 100, |_| true);
        assert!(ev.contains(&CfdpEvent::Cancelled(
            id,
            ConditionCode::PositiveAckLimitReached as u8
        )));
    }

    /// F-GND-06: suspended transactions send nothing and their timers freeze, so a 30-day
    /// suspension does not exhaust the ACK limit; after resume the transfer completes.
    #[test]
    fn f_gnd_06_suspend_freezes_timers_and_resume_completes() {
        let mut tx = CfdpEntity::new(cfg(10, 1), 0).expect("tx");
        let mut rx = CfdpEntity::new(cfg(1, 10), 0).expect("rx");
        let data = file(3_000);
        let id = tx
            .put(CfdpClass::Class2, "report.pdf", data.clone())
            .expect("put");
        // First pass partly delivered, then LOS.
        let first = tx.poll(TaiNs(0), 10).expect("poll");
        for pdu in &first {
            rx.receive(TaiNs(0), pdu).expect("accepted");
        }
        tx.suspend_all(TaiNs(S));
        rx.suspend_all(TaiNs(S));
        assert!(tx.any_suspended());
        let thirty_days = 30 * 86_400 * S;
        for day in 0..30 {
            assert!(tx
                .poll(TaiNs(S + day * 86_400 * S), 100)
                .expect("poll")
                .is_empty());
        }
        assert_eq!(tx.source_state(id), Some(TxState::Suspended));
        tx.resume_all(TaiNs(thirty_days));
        rx.resume_all(TaiNs(thirty_days));
        let ev = run(&mut tx, &mut rx, thirty_days, 50, |_| false);
        assert!(ev.contains(&CfdpEvent::Sent(id)), "{ev:?}");
        assert!(ev
            .iter()
            .any(|e| matches!(e, CfdpEvent::Received { data: d, .. } if *d == data)));
    }

    #[test]
    fn foreign_and_malformed_pdus_are_rejected() {
        let mut rx = CfdpEntity::new(cfg(1, 10), 0).expect("rx");
        assert!(rx.receive(TaiNs(0), &[0xFF; 3]).is_err());
        assert!(rx.receive(TaiNs(0), &[]).is_err());
        let mut other = CfdpEntity::new(cfg(20, 1), 0).expect("other");
        other.put(CfdpClass::Class1, "f", file(10)).expect("put");
        let pdus = other.poll(TaiNs(0), 10).expect("poll");
        assert!(matches!(
            rx.receive(TaiNs(0), &pdus[0]),
            Err(CfdpError::Rejected(_))
        ));
        // Corrupting a byte breaks the PDU CRC.
        let mut tx = CfdpEntity::new(cfg(10, 1), 0).expect("tx");
        tx.put(CfdpClass::Class1, "f", file(10)).expect("put");
        let mut pdu = tx.poll(TaiNs(0), 1).expect("poll").remove(0);
        let last = pdu.len() - 3;
        pdu[last] ^= 0xFF;
        assert!(rx.receive(TaiNs(0), &pdu).is_err());
    }

    #[test]
    fn snapshot_restore_continues_a_transfer() {
        let mut tx = CfdpEntity::new(cfg(10, 1), 0).expect("tx");
        let mut rx = CfdpEntity::new(cfg(1, 10), 0).expect("rx");
        let data = file(1_000);
        let id = tx.put(CfdpClass::Class2, "f", data.clone()).expect("put");
        for pdu in tx.poll(TaiNs(0), 4).expect("poll") {
            rx.receive(TaiNs(0), &pdu).expect("ok");
        }
        let json = serde_json::to_string(&tx.snapshot()).expect("serialises");
        let snap: EntitySnapshot = serde_json::from_str(&json).expect("parses");
        let mut tx = CfdpEntity::restore(cfg(10, 1), snap).expect("restores");
        assert!(
            tx.poll(TaiNs(S), 10).is_err(),
            "content must be re-attached first"
        );
        assert!(tx.restore_data(id, file(999)).is_err());
        tx.restore_data(id, data.clone()).expect("restores");
        let ev = run(&mut tx, &mut rx, 2 * S, 50, |_| false);
        assert!(ev.contains(&CfdpEvent::Sent(id)));
        assert!(ev
            .iter()
            .any(|e| matches!(e, CfdpEvent::Received { data: d, .. } if *d == data)));
    }

    #[test]
    fn invalid_configuration_is_refused() {
        assert!(CfdpEntity::new(cfg(1, 1), 0).is_err());
        let mut c = cfg(1, 2);
        c.segment_len = 0;
        assert!(CfdpEntity::new(c, 0).is_err());
        let mut c = cfg(1, 2);
        c.ack_limit = 0;
        assert!(CfdpEntity::new(c, 0).is_err());
        let mut tx = CfdpEntity::new(cfg(1, 2), 0).expect("ok");
        assert!(tx.put(CfdpClass::Class1, "", vec![]).is_err());
    }
}
