//! Store-and-forward downlink queue (F-GND-05, F-GND-06, F-GND-14, R-SEC-06).
//!
//! - Every item is checked against its channel's allow-list, signed with Ed25519 and persisted
//!   through [`QueueStore`] before it is accepted; nothing is held only in memory.
//! - Priorities CRITICAL → ROUTINE → RESEARCH, first-in first-out within a priority.
//! - Items leave only during a contact window ([`ContactSchedule`], or a link override from the
//!   link monitor). At loss of signal every CFDP transaction is suspended; at the next
//!   acquisition of signal they resume. Items are removed only when delivered (packets: handed to
//!   the link and confirmed with [`DownlinkQueue::delivered`]; files: CFDP transaction
//!   completed). A cancelled file transaction is restarted, never dropped.
//! - On restart the queue reloads every item and the CFDP state; items whose signature no longer
//!   verifies (store tampering) are quarantined, never sent, and reported.
//!
//! Allow-lists: OPS carries only [`OpsRecord`]s (docs/06 §3a); MEDICAL carries parameter
//! summaries whose parameter is in the MEDICAL allow-list (catalog channels, optionally narrowed
//! by mission configuration, never widened) and clinical files; RESEARCH carries only
//! de-identified research exports.

use std::collections::{BTreeMap, BTreeSet};

use caelum_domain::parameters::{Channel, Parameter, CATALOG};
use caelum_domain::time::TaiNs;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cfdp::{
    CfdpClass, CfdpConfig, CfdpEntity, CfdpError, CfdpEvent, EntitySnapshot, TransactionId,
};
use crate::config::{ApidPlan, ContactSchedule};
use crate::medical::{downlinked, MedicalSummary};
use crate::ops::OpsRecord;
use crate::packet::{SeqCounters, TmPacket};

/// Domain separation for downlink item signatures.
const ITEM_TAG: &[u8] = b"CAELUM-DOWNLINK-1";

/// Queue errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueueError {
    /// The payload is not allowed on the channel.
    #[error("not allowed on the {channel:?} channel: {reason}")]
    NotAllowed {
        /// Channel.
        channel: Channel,
        /// Why.
        reason: String,
    },
    /// Persistence failed; the item was not accepted.
    #[error("queue store: {0}")]
    Store(String),
    /// Building a packet failed.
    #[error("encoding: {0}")]
    Encode(String),
    /// A signature does not verify.
    #[error("item {0} signature does not verify")]
    BadSignature(u64),
    /// CFDP failure.
    #[error(transparent)]
    Cfdp(#[from] CfdpError),
    /// File content does not match the queued file.
    #[error("file `{0}` content does not match its digest")]
    FileMismatch(String),
}

/// Downlink priority (docs/04 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum QueuePriority {
    /// Alarms and escalations.
    Critical,
    /// Summaries, reports, event files.
    Routine,
    /// Research datasets.
    Research,
}

/// Kind of queued file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileKind {
    /// Event packet (alarm context, event-linked waveforms; F-GND-14).
    EventPacket,
    /// Daily report.
    DailyReport,
    /// DICOM object.
    Dicom,
    /// ECG file.
    Ecg,
    /// FHIR / CSV / EDF+ export.
    Export,
    /// De-identified research export.
    ResearchExport,
}

/// A file to send by CFDP. The content stays in the attachment store ([`FileSource`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileItem {
    /// Destination file name.
    pub name: String,
    /// Kind.
    pub kind: FileKind,
    /// CFDP class.
    pub class: CfdpClass,
    /// SHA-256 of the content (hex).
    pub sha256: String,
    /// Length in octets.
    pub len: u64,
}

impl FileItem {
    /// Describes content for queueing.
    pub fn describe(name: &str, kind: FileKind, class: CfdpClass, content: &[u8]) -> Self {
        Self {
            name: name.to_owned(),
            kind,
            class,
            sha256: crate::uplink::hex(&Sha256::digest(content)),
            len: content.len() as u64,
        }
    }
}

/// What an item carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Payload {
    /// An OPS record (path B content).
    Ops(OpsRecord),
    /// A MEDICAL parameter summary.
    Summary(MedicalSummary),
    /// A file.
    File(FileItem),
}

/// Per-channel allow-lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowLists {
    medical: BTreeSet<Parameter>,
}

impl AllowLists {
    /// The catalog allow-list: every parameter whose catalog entry allows MEDICAL downlink.
    pub fn from_catalog() -> Self {
        Self {
            medical: CATALOG
                .iter()
                .map(|s| s.parameter)
                .filter(|p| downlinked(*p))
                .collect(),
        }
    }

    /// Narrows the MEDICAL list to `allowed` (mission configuration); it can never widen it.
    pub fn restrict_medical(&mut self, allowed: &[Parameter]) {
        self.medical.retain(|p| allowed.contains(p));
    }

    /// Checks a payload against a channel.
    pub fn check(&self, channel: Channel, payload: &Payload) -> Result<(), QueueError> {
        let deny = |reason: &str| {
            Err(QueueError::NotAllowed {
                channel,
                reason: reason.to_owned(),
            })
        };
        match (channel, payload) {
            (Channel::Ops, Payload::Ops(_)) => Ok(()),
            (Channel::Ops, _) => deny("OPS carries only status and alarm records (R-SEC-06)"),
            (Channel::Medical, Payload::Summary(s)) => {
                if self.medical.contains(&s.parameter) {
                    Ok(())
                } else {
                    deny("parameter not on the MEDICAL allow-list")
                }
            }
            (Channel::Medical, Payload::File(f)) if f.kind != FileKind::ResearchExport => Ok(()),
            (Channel::Medical, _) => deny("not MEDICAL content"),
            (Channel::Research, Payload::File(f)) if f.kind == FileKind::ResearchExport => Ok(()),
            (Channel::Research, _) => deny("RESEARCH carries only de-identified exports"),
        }
    }
}

/// A signed queue item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignedItem {
    /// Queue-assigned ID, increasing.
    pub id: u64,
    /// Channel.
    pub channel: Channel,
    /// Priority.
    pub priority: QueuePriority,
    /// Enqueued at (TAI).
    pub created_at: TaiNs,
    /// Content.
    pub payload: Payload,
    /// Signing key ID (first 8 octets of SHA-256 of the public key, hex).
    pub key_id: String,
    /// Ed25519 signature, hex.
    pub signature: String,
}

fn item_message(
    id: u64,
    channel: Channel,
    priority: QueuePriority,
    created_at: TaiNs,
    payload: &Payload,
) -> Result<Vec<u8>, QueueError> {
    let body = serde_json::to_vec(payload).map_err(|e| QueueError::Encode(e.to_string()))?;
    let mut m = ITEM_TAG.to_vec();
    m.extend(id.to_be_bytes());
    m.push(match channel {
        Channel::Ops => 1,
        Channel::Medical => 2,
        Channel::Research => 3,
    });
    m.push(priority as u8);
    m.extend(created_at.0.to_be_bytes());
    m.extend(Sha256::digest(&body));
    Ok(m)
}

/// Signs downlink items (the instance key from `caelum-security`).
pub trait ItemSigner {
    /// Key ID.
    fn key_id(&self) -> String;
    /// Signs a message.
    fn sign_message(&self, msg: &[u8]) -> Signature;
    /// The public key.
    fn verifying_key(&self) -> VerifyingKey;
}

impl ItemSigner for SigningKey {
    fn key_id(&self) -> String {
        caelum_alarms::signing::key_id(&self.verifying_key())
    }
    fn sign_message(&self, msg: &[u8]) -> Signature {
        self.sign(msg)
    }
    fn verifying_key(&self) -> VerifyingKey {
        SigningKey::verifying_key(self)
    }
}

/// Verifies an item's signature (ground side, and on reload).
pub fn verify_item(item: &SignedItem, key: &VerifyingKey) -> Result<(), QueueError> {
    let bad = || QueueError::BadSignature(item.id);
    if caelum_alarms::signing::key_id(key) != item.key_id {
        return Err(bad());
    }
    let raw = crate::uplink::unhex(&item.signature).ok_or_else(bad)?;
    let sig: [u8; 64] = raw.try_into().map_err(|_| bad())?;
    let msg = item_message(
        item.id,
        item.channel,
        item.priority,
        item.created_at,
        &item.payload,
    )?;
    key.verify_strict(&msg, &Signature::from_bytes(&sig))
        .map_err(|_| bad())
}

/// Persistence of the queue (the lead implements it on SQLite; [`MemoryStore`] for tests).
pub trait QueueStore {
    /// Persists a new item. The queue accepts the item only if this succeeds.
    fn insert(&mut self, item: &SignedItem) -> Result<(), QueueError>;
    /// Removes a delivered item.
    fn remove(&mut self, id: u64) -> Result<(), QueueError>;
    /// Every stored item.
    fn load(&self) -> Result<Vec<SignedItem>, QueueError>;
    /// Persists the link state (sequence counts, CFDP state, file transfers), JSON.
    fn save_state(&mut self, json: &str) -> Result<(), QueueError>;
    /// The persisted link state.
    fn load_state(&self) -> Result<Option<String>, QueueError>;
}

/// In-memory [`QueueStore`].
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    items: BTreeMap<u64, SignedItem>,
    state: Option<String>,
    /// When set, every write fails (tests of the "not accepted unless persisted" rule).
    pub fail_writes: bool,
}

impl MemoryStore {
    /// Mutable access to a stored item (tamper tests).
    pub fn item_mut(&mut self, id: u64) -> Option<&mut SignedItem> {
        self.items.get_mut(&id)
    }
}

impl QueueStore for MemoryStore {
    fn insert(&mut self, item: &SignedItem) -> Result<(), QueueError> {
        if self.fail_writes {
            return Err(QueueError::Store("write failed".into()));
        }
        self.items.insert(item.id, item.clone());
        Ok(())
    }
    fn remove(&mut self, id: u64) -> Result<(), QueueError> {
        if self.fail_writes {
            return Err(QueueError::Store("write failed".into()));
        }
        self.items.remove(&id);
        Ok(())
    }
    fn load(&self) -> Result<Vec<SignedItem>, QueueError> {
        Ok(self.items.values().cloned().collect())
    }
    fn save_state(&mut self, json: &str) -> Result<(), QueueError> {
        if self.fail_writes {
            return Err(QueueError::Store("write failed".into()));
        }
        self.state = Some(json.to_owned());
        Ok(())
    }
    fn load_state(&self) -> Result<Option<String>, QueueError> {
        Ok(self.state.clone())
    }
}

/// Reads queued file content from the attachment store.
pub trait FileSource {
    /// The content of a queued file.
    fn read(&self, item: &FileItem) -> Result<Vec<u8>, QueueError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LinkState {
    seq: BTreeMap<u16, u16>,
    cfdp: EntitySnapshot,
    transfers: BTreeMap<u64, TransactionId>,
    in_contact: bool,
}

/// One packet to transmit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outbound {
    /// The queue item it carries (`None` for CFDP PDUs; files complete through CFDP).
    pub item: Option<u64>,
    /// Encoded telemetry packet.
    pub packet: Vec<u8>,
}

/// Counters for the Ground screen and the 30-day test.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueStats {
    /// Items waiting.
    pub pending: usize,
    /// Items quarantined (signature failure on reload).
    pub quarantined: usize,
    /// File transfers in progress.
    pub transfers: usize,
    /// Restarted file transfers after a cancellation.
    pub restarts: u64,
}

/// The downlink queue.
pub struct DownlinkQueue<S: QueueStore> {
    store: S,
    plan: ApidPlan,
    allow: AllowLists,
    schedule: ContactSchedule,
    link_override: Option<bool>,
    verifying_key: VerifyingKey,
    items: BTreeMap<u64, SignedItem>,
    quarantined: BTreeSet<u64>,
    transfers: BTreeMap<u64, TransactionId>,
    cfdp: CfdpEntity,
    seq: SeqCounters,
    next_id: u64,
    in_contact: bool,
    restarts: u64,
}

impl<S: QueueStore> DownlinkQueue<S> {
    /// Opens the queue from its store (first start or restart). Items whose signature does not
    /// verify against `verifying_key` are quarantined. Outgoing file transfers get their content
    /// back from `files`.
    pub fn open(
        store: S,
        plan: ApidPlan,
        allow: AllowLists,
        schedule: ContactSchedule,
        cfdp_cfg: CfdpConfig,
        verifying_key: VerifyingKey,
        files: &dyn FileSource,
    ) -> Result<Self, QueueError> {
        let mut items = BTreeMap::new();
        let mut quarantined = BTreeSet::new();
        for item in store.load()? {
            if verify_item(&item, &verifying_key).is_err() {
                quarantined.insert(item.id);
            }
            items.insert(item.id, item);
        }
        let next_id = items.keys().next_back().map_or(1, |id| id + 1);
        let (cfdp, seq, transfers, in_contact) = match store.load_state()? {
            Some(json) => {
                let s: LinkState =
                    serde_json::from_str(&json).map_err(|e| QueueError::Store(e.to_string()))?;
                let mut cfdp = CfdpEntity::restore(cfdp_cfg, s.cfdp)?;
                for (item_id, tid) in &s.transfers {
                    if let Some(SignedItem {
                        payload: Payload::File(f),
                        ..
                    }) = items.get(item_id)
                    {
                        let data = read_checked(files, f)?;
                        // A transaction that already finished has nothing to restore.
                        let _ = cfdp.restore_data(*tid, data);
                    }
                }
                (cfdp, SeqCounters::restore(s.seq), s.transfers, s.in_contact)
            }
            None => (
                CfdpEntity::new(cfdp_cfg, 0)?,
                SeqCounters::default(),
                BTreeMap::new(),
                false,
            ),
        };
        Ok(Self {
            store,
            plan,
            allow,
            schedule,
            link_override: None,
            verifying_key,
            items,
            quarantined,
            transfers,
            cfdp,
            seq,
            next_id,
            in_contact,
            restarts: 0,
        })
    }

    /// Releases the store (restart simulation, shutdown).
    pub fn into_store(self) -> S {
        self.store
    }

    /// Replaces the contact schedule (signed uplink of a new schedule).
    pub fn set_schedule(&mut self, schedule: ContactSchedule) {
        self.schedule = schedule;
    }

    /// Link monitor override: `Some(false)` = loss of signal detected, `Some(true)` = link up
    /// outside the schedule, `None` = follow the schedule.
    pub fn set_link_override(&mut self, state: Option<bool>) {
        self.link_override = state;
    }

    /// Counters.
    pub fn stats(&self) -> QueueStats {
        QueueStats {
            pending: self.items.len(),
            quarantined: self.quarantined.len(),
            transfers: self.transfers.len(),
            restarts: self.restarts,
        }
    }

    /// Whether any CFDP transaction is suspended.
    pub fn cfdp_suspended(&self) -> bool {
        self.cfdp.any_suspended()
    }

    /// Accepts an item: allow-list check, signature, persistence. Returns its ID.
    pub fn enqueue(
        &mut self,
        channel: Channel,
        priority: QueuePriority,
        payload: Payload,
        now: TaiNs,
        signer: &dyn ItemSigner,
    ) -> Result<u64, QueueError> {
        self.allow.check(channel, &payload)?;
        if let Payload::Summary(s) = &payload {
            s.validate()
                .map_err(|e| QueueError::Encode(e.to_string()))?;
        }
        if signer.verifying_key() != self.verifying_key {
            return Err(QueueError::BadSignature(self.next_id));
        }
        let id = self.next_id;
        let msg = item_message(id, channel, priority, now, &payload)?;
        let item = SignedItem {
            id,
            channel,
            priority,
            created_at: now,
            payload,
            key_id: signer.key_id(),
            signature: crate::uplink::hex(&signer.sign_message(&msg).to_bytes()),
        };
        self.store.insert(&item)?;
        self.next_id += 1;
        self.items.insert(id, item);
        Ok(id)
    }

    fn save(&mut self) -> Result<(), QueueError> {
        let state = LinkState {
            seq: self.seq.snapshot(),
            cfdp: self.cfdp.snapshot(),
            transfers: self.transfers.clone(),
            in_contact: self.in_contact,
        };
        let json = serde_json::to_string(&state).map_err(|e| QueueError::Store(e.to_string()))?;
        self.store.save_state(&json)
    }

    /// Advances the queue: handles contact transitions (suspend at LOS, resume at AOS), starts
    /// file transfers and returns at most `budget` packets to transmit. Nothing leaves outside a
    /// contact window.
    pub fn tick(
        &mut self,
        now: TaiNs,
        budget: usize,
        files: &dyn FileSource,
    ) -> Result<Vec<Outbound>, QueueError> {
        let contact = self
            .link_override
            .unwrap_or_else(|| self.schedule.in_contact(now));
        if contact != self.in_contact {
            if contact {
                self.cfdp.resume_all(now);
            } else {
                self.cfdp.suspend_all(now);
            }
            self.in_contact = contact;
        }
        let mut out = Vec::new();
        if !contact {
            self.save()?;
            return Ok(out);
        }
        let mut order: Vec<(QueuePriority, u64)> = self
            .items
            .values()
            .filter(|i| !self.quarantined.contains(&i.id))
            .map(|i| (i.priority, i.id))
            .collect();
        order.sort_unstable();
        for (_, id) in order {
            let Some(item) = self.items.get(&id) else {
                continue;
            };
            match &item.payload {
                Payload::File(f) => {
                    if !self.transfers.contains_key(&id) {
                        let data = read_checked(files, f)?;
                        let tid = self.cfdp.put(f.class, &f.name, data)?;
                        self.transfers.insert(id, tid);
                    }
                }
                Payload::Ops(r) => {
                    if out.len() < budget {
                        let apid = r.apid(&self.plan);
                        let p = r
                            .to_packet(&self.plan, self.seq.next(apid))
                            .map_err(|e| QueueError::Encode(e.to_string()))?;
                        out.push(Outbound {
                            item: Some(id),
                            packet: p.into_bytes(),
                        });
                    }
                }
                Payload::Summary(s) => {
                    if out.len() < budget {
                        let p = s
                            .to_packet(&self.plan, self.seq.next(self.plan.medical_summary))
                            .map_err(|e| QueueError::Encode(e.to_string()))?;
                        out.push(Outbound {
                            item: Some(id),
                            packet: p.into_bytes(),
                        });
                    }
                }
            }
        }
        let room = budget.saturating_sub(out.len());
        for pdu in self.cfdp.poll(now, room)? {
            let p = TmPacket::build(self.plan.cfdp, self.seq.next(self.plan.cfdp), now, &pdu)
                .map_err(|e| QueueError::Encode(e.to_string()))?;
            out.push(Outbound {
                item: None,
                packet: p.into_bytes(),
            });
        }
        self.handle_events()?;
        self.save()?;
        Ok(out)
    }

    /// Confirms that the link accepted a packet item; it is removed from the queue. Packets not
    /// confirmed are sent again at the next tick.
    pub fn delivered(&mut self, id: u64) -> Result<(), QueueError> {
        if matches!(
            self.items.get(&id).map(|i| &i.payload),
            Some(Payload::Ops(_) | Payload::Summary(_))
        ) {
            self.store.remove(id)?;
            self.items.remove(&id);
        }
        Ok(())
    }

    /// A CFDP PDU from the ground entity (ACK, NAK, Finished).
    pub fn receive_pdu(&mut self, now: TaiNs, pdu: &[u8]) -> Result<(), QueueError> {
        self.cfdp.receive(now, pdu)?;
        self.handle_events()
    }

    fn handle_events(&mut self) -> Result<(), QueueError> {
        for event in self.cfdp.take_events() {
            let (tid, completed) = match event {
                CfdpEvent::Sent(tid) => (tid, true),
                CfdpEvent::Cancelled(tid, _) => (tid, false),
                CfdpEvent::Received { .. } => continue,
            };
            let Some(item_id) = self
                .transfers
                .iter()
                .find(|(_, t)| **t == tid)
                .map(|(i, _)| *i)
            else {
                continue;
            };
            self.transfers.remove(&item_id);
            if completed {
                self.store.remove(item_id)?;
                self.items.remove(&item_id);
            } else {
                // Never dropped: the next tick starts a new transaction.
                self.restarts += 1;
            }
        }
        self.cfdp.prune();
        Ok(())
    }
}

fn read_checked(files: &dyn FileSource, f: &FileItem) -> Result<Vec<u8>, QueueError> {
    let data = files.read(f)?;
    if data.len() as u64 != f.len || crate::uplink::hex(&Sha256::digest(&data)) != f.sha256 {
        return Err(QueueError::FileMismatch(f.name.clone()));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfdp::tests::{cfg, S};
    use crate::config::ContactWindow;
    use crate::medical::SummaryValue;
    use crate::ops::tests::{alarm, crew, plan};
    use crate::ops::OpsAlarmRecord;
    use crate::packet::ParsedTm;
    use caelum_domain::observation::Quality;
    use std::collections::HashMap;

    const DAY: i64 = 86_400 * S;
    const T0: i64 = 2_100_000_000 * S;

    #[derive(Default)]
    struct Files(HashMap<String, Vec<u8>>);

    impl FileSource for Files {
        fn read(&self, item: &FileItem) -> Result<Vec<u8>, QueueError> {
            self.0
                .get(&item.name)
                .cloned()
                .ok_or_else(|| QueueError::Store("missing".into()))
        }
    }

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[42; 32])
    }

    fn window(a: i64, b: i64) -> ContactWindow {
        ContactWindow {
            start_tai_ns: a,
            end_tai_ns: b,
        }
    }

    fn open(
        store: MemoryStore,
        schedule: ContactSchedule,
        files: &Files,
    ) -> DownlinkQueue<MemoryStore> {
        DownlinkQueue::open(
            store,
            plan(),
            AllowLists::from_catalog(),
            schedule,
            cfg(10, 1),
            key().verifying_key(),
            files,
        )
        .expect("opens")
    }

    fn summary(at: i64) -> MedicalSummary {
        MedicalSummary {
            crew_id: crew(),
            parameter: Parameter::HeartRate,
            quality: Quality::Good,
            window_start: TaiNs(at - 60 * S),
            window_end: TaiNs(at),
            value: SummaryValue::Numeric {
                min: 60.0,
                mean: 62.0,
                max: 65.0,
                count: 60,
            },
        }
    }

    /// The ground station: decodes packets, runs the receiving CFDP entity.
    struct Ground {
        cfdp: CfdpEntity,
        ops: Vec<OpsRecord>,
        summaries: Vec<MedicalSummary>,
        files: BTreeMap<String, Vec<u8>>,
    }

    impl Ground {
        fn new() -> Self {
            Self {
                cfdp: CfdpEntity::new(cfg(1, 10), 0).expect("ground"),
                ops: Vec::new(),
                summaries: Vec::new(),
                files: BTreeMap::new(),
            }
        }

        fn receive(&mut self, now: TaiNs, packet: &[u8]) {
            let p = plan();
            let (tm, rest) = ParsedTm::parse(packet).expect("valid TM");
            assert!(rest.is_empty());
            if tm.apid == p.cfdp {
                let _ = self.cfdp.receive(now, &tm.user_data);
            } else if tm.apid == p.medical_summary {
                self.summaries
                    .push(MedicalSummary::decode(tm.time, &tm.user_data).expect("summary"));
            } else {
                self.ops
                    .push(OpsRecord::decode(&p, tm.apid, tm.time, &tm.user_data).expect("ops"));
            }
            self.collect();
        }

        fn collect(&mut self) {
            for e in self.cfdp.take_events() {
                if let CfdpEvent::Received { name, data, .. } = e {
                    // Class 1 is at-least-once: a repeat must be byte-identical.
                    if let Some(prev) = self.files.insert(name, data.clone()) {
                        assert_eq!(prev, data, "a repeated delivery differs");
                    }
                }
            }
        }
    }

    /// One minute of link activity: queue → ground, ground responses → queue. `drop` loses PDUs.
    fn exchange(
        q: &mut DownlinkQueue<MemoryStore>,
        g: &mut Ground,
        files: &Files,
        now: TaiNs,
        budget: usize,
        drop: &mut dyn FnMut() -> bool,
    ) -> usize {
        let out = q.tick(now, budget, files).expect("tick");
        let n = out.len();
        for o in out {
            if o.item.is_none() && drop() {
                continue;
            }
            g.receive(now, &o.packet);
            if let Some(id) = o.item {
                q.delivered(id).expect("delivered");
            }
        }
        let responses = g.cfdp.poll(now, budget).expect("ground poll");
        g.collect();
        for pdu in responses {
            if !drop() {
                let _ = q.receive_pdu(now, &pdu);
            }
        }
        n
    }

    /// F-GND-05, F-GND-06 gate: over a simulated 30-day loss of signal the queue holds every
    /// item, CFDP transactions are suspended at LOS and resumed at the next contact window,
    /// a restart in the middle of the LOS loses nothing, and every item and file is delivered
    /// exactly once, with no loss, once contact returns.
    #[test]
    fn f_gnd_05_f_gnd_06_thirty_day_los_holds_everything_and_completes() {
        let los_end = T0 + 30 * DAY;
        let schedule = ContactSchedule::new(vec![
            window(T0, T0 + 3 * 60 * S),
            window(los_end, los_end + 2 * 3600 * S),
        ])
        .expect("schedule");
        let mut files = Files::default();
        let big: Vec<u8> = (0..20_000u32).map(|i| (i % 253) as u8).collect();
        let dicom: Vec<u8> = (0..15_000u32).map(|i| (i % 241) as u8).collect();
        let small: Vec<u8> = (0..900u32).map(|i| (i % 7) as u8).collect();
        files.0.insert("ecg/evt-1.edf".into(), big.clone());
        files.0.insert("dicom/us-1.dcm".into(), dicom.clone());
        files.0.insert("reports/day-1.pdf".into(), small.clone());

        let mut q = open(MemoryStore::default(), schedule.clone(), &files);
        let k = key();
        let mut expected_ops = Vec::new();
        for i in 0..3 {
            let r = OpsRecord::Alarm(alarm(T0 + i * S));
            q.enqueue(
                Channel::Ops,
                QueuePriority::Critical,
                Payload::Ops(r),
                TaiNs(T0),
                &k,
            )
            .expect("ops");
            expected_ops.push(r);
        }
        for i in 0..5 {
            q.enqueue(
                Channel::Medical,
                QueuePriority::Routine,
                Payload::Summary(summary(T0 + i * 60 * S)),
                TaiNs(T0),
                &k,
            )
            .expect("summary");
        }
        for (name, kind, class, data) in [
            ("ecg/evt-1.edf", FileKind::Ecg, CfdpClass::Class2, &big),
            ("dicom/us-1.dcm", FileKind::Dicom, CfdpClass::Class2, &dicom),
            (
                "reports/day-1.pdf",
                FileKind::DailyReport,
                CfdpClass::Class1,
                &small,
            ),
        ] {
            q.enqueue(
                Channel::Medical,
                QueuePriority::Routine,
                Payload::File(FileItem::describe(name, kind, class, data)),
                TaiNs(T0),
                &k,
            )
            .expect("file");
        }
        assert_eq!(q.stats().pending, 11);

        // First contact: 3 minutes at 30 packets per minute — not enough for the files.
        let mut g = Ground::new();
        let mut never = || false;
        for m in 0..3 {
            exchange(
                &mut q,
                &mut g,
                &files,
                TaiNs(T0 + m * 60 * S),
                30,
                &mut never,
            );
        }
        assert_eq!(g.ops.len(), 3, "critical items go first");
        assert!(g.files.is_empty(), "files are still in transfer at LOS");
        let pending_at_los = q.stats().pending;
        assert_eq!(pending_at_los, 3, "only the three files remain");

        // 30 days of loss of signal: an alarm every day, a restart on day 15.
        for day in 0..30 {
            let now = TaiNs(T0 + 3 * 60 * S + day * DAY);
            let r = OpsRecord::Alarm(alarm(now.0));
            q.enqueue(
                Channel::Ops,
                QueuePriority::Critical,
                Payload::Ops(r),
                now,
                &k,
            )
            .expect("held");
            expected_ops.push(r);
            for h in 0..24 {
                let t = TaiNs(now.0 + h * 3600 * S);
                assert!(
                    q.tick(t, 30, &files).expect("tick").is_empty(),
                    "nothing leaves during LOS"
                );
                assert!(q.cfdp_suspended(), "transactions suspended at LOS");
            }
            if day == 15 {
                let store = q.into_store();
                q = open(store, schedule.clone(), &files);
                assert!(q.cfdp_suspended(), "suspension survives the restart");
            }
        }
        assert_eq!(q.stats().pending, 3 + 30, "every item held through the LOS");
        assert_eq!(q.stats().quarantined, 0);

        // Contact returns: transfers resume (with some loss on the link) and complete.
        let mut n = 0u32;
        let mut lossy = || {
            n += 1;
            n.is_multiple_of(17)
        };
        for m in 0..120 {
            exchange(
                &mut q,
                &mut g,
                &files,
                TaiNs(los_end + m * 60 * S),
                30,
                &mut lossy,
            );
            if q.stats().pending == 0 {
                break;
            }
        }
        assert_eq!(q.stats().pending, 0, "{:?}", q.stats());
        assert!(q.into_store().load().expect("load").is_empty());
        assert_eq!(g.ops, expected_ops, "every OPS record, once, in order");
        assert_eq!(g.summaries.len(), 5);
        assert_eq!(g.files.get("ecg/evt-1.edf"), Some(&big));
        assert_eq!(g.files.get("dicom/us-1.dcm"), Some(&dicom));
        assert_eq!(g.files.get("reports/day-1.pdf"), Some(&small));
    }

    /// R-SEC-06: the OPS channel refuses everything except OPS records; physiology cannot be
    /// queued on it, and research carries only de-identified exports.
    #[test]
    fn r_sec_06_ops_channel_refuses_physiology() {
        let files = Files::default();
        let mut q = open(MemoryStore::default(), ContactSchedule::default(), &files);
        let k = key();
        let err = q.enqueue(
            Channel::Ops,
            QueuePriority::Critical,
            Payload::Summary(summary(T0)),
            TaiNs(T0),
            &k,
        );
        assert!(matches!(
            err,
            Err(QueueError::NotAllowed {
                channel: Channel::Ops,
                ..
            })
        ));
        let file = FileItem::describe("x", FileKind::Ecg, CfdpClass::Class2, b"x");
        assert!(q
            .enqueue(
                Channel::Ops,
                QueuePriority::Critical,
                Payload::File(file.clone()),
                TaiNs(T0),
                &k
            )
            .is_err());
        assert!(q
            .enqueue(
                Channel::Research,
                QueuePriority::Research,
                Payload::File(file),
                TaiNs(T0),
                &k
            )
            .is_err());
        assert!(q
            .enqueue(
                Channel::Medical,
                QueuePriority::Critical,
                Payload::Ops(OpsRecord::Alarm(alarm(T0))),
                TaiNs(T0),
                &k
            )
            .is_err());
    }

    /// F-GND-05: the MEDICAL allow-list can be narrowed by configuration, never widened.
    #[test]
    fn f_gnd_05_medical_allow_list_narrows_only() {
        let mut a = AllowLists::from_catalog();
        a.restrict_medical(&[Parameter::Spo2, Parameter::SpeState]);
        let hr = Payload::Summary(summary(T0));
        assert!(a.check(Channel::Medical, &hr).is_err());
        let spo2 = Payload::Summary(MedicalSummary {
            parameter: Parameter::Spo2,
            ..summary(T0)
        });
        assert!(a.check(Channel::Medical, &spo2).is_ok());
        // spe_state has no defined states: it cannot be added by configuration.
        assert!(!a.medical.contains(&Parameter::SpeState));
    }

    /// F-GND-05: an item is accepted only once persisted; a store failure refuses it.
    #[test]
    fn f_gnd_05_item_not_accepted_unless_persisted() {
        let files = Files::default();
        let store = MemoryStore {
            fail_writes: true,
            ..MemoryStore::default()
        };
        let mut q = open(store, ContactSchedule::default(), &files);
        let r = Payload::Ops(OpsRecord::Alarm(alarm(T0)));
        assert!(matches!(
            q.enqueue(Channel::Ops, QueuePriority::Critical, r, TaiNs(T0), &key()),
            Err(QueueError::Store(_))
        ));
        assert_eq!(q.stats().pending, 0);
    }

    /// F-GND-05: items are signed; a tampered stored item is quarantined at reload and never sent.
    #[test]
    fn f_gnd_05_tampered_items_are_quarantined() {
        let files = Files::default();
        let schedule = ContactSchedule::new(vec![window(T0, T0 + DAY)]).expect("schedule");
        let mut q = open(MemoryStore::default(), schedule.clone(), &files);
        let k = key();
        let a = q
            .enqueue(
                Channel::Ops,
                QueuePriority::Critical,
                Payload::Ops(OpsRecord::Alarm(alarm(T0))),
                TaiNs(T0 - DAY),
                &k,
            )
            .expect("a");
        q.enqueue(
            Channel::Ops,
            QueuePriority::Critical,
            Payload::Ops(OpsRecord::Alarm(alarm(T0 + S))),
            TaiNs(T0 - DAY),
            &k,
        )
        .expect("b");
        let mut store = q.into_store();
        let item = store.item_mut(a).expect("stored");
        verify_item(item, &k.verifying_key()).expect("valid before tampering");
        if let Payload::Ops(OpsRecord::Alarm(OpsAlarmRecord {
            escalation_stage, ..
        })) = &mut item.payload
        {
            *escalation_stage = 0;
        }
        let mut q = open(store, schedule, &files);
        assert_eq!(q.stats().quarantined, 1);
        let out = q.tick(TaiNs(T0), 10, &files).expect("tick");
        assert_eq!(out.len(), 1);
        assert_ne!(out[0].item, Some(a));
        // A signer other than the instance key is refused.
        let other = SigningKey::from_bytes(&[1; 32]);
        assert!(q
            .enqueue(
                Channel::Ops,
                QueuePriority::Critical,
                Payload::Ops(OpsRecord::Alarm(alarm(T0))),
                TaiNs(T0),
                &other
            )
            .is_err());
    }

    /// F-GND-05: priorities order transmission; unconfirmed packets are re-sent.
    #[test]
    fn f_gnd_05_priority_order_and_resend_until_confirmed() {
        let files = Files::default();
        let schedule = ContactSchedule::new(vec![window(T0, T0 + DAY)]).expect("schedule");
        let mut q = open(MemoryStore::default(), schedule, &files);
        let k = key();
        let s = q
            .enqueue(
                Channel::Medical,
                QueuePriority::Routine,
                Payload::Summary(summary(T0)),
                TaiNs(T0),
                &k,
            )
            .expect("summary");
        let c = q
            .enqueue(
                Channel::Ops,
                QueuePriority::Critical,
                Payload::Ops(OpsRecord::Alarm(alarm(T0))),
                TaiNs(T0),
                &k,
            )
            .expect("critical");
        let out = q.tick(TaiNs(T0), 1, &files).expect("tick");
        assert_eq!(out.iter().map(|o| o.item).collect::<Vec<_>>(), [Some(c)]);
        // Not confirmed: sent again, with a new sequence count.
        let again = q.tick(TaiNs(T0 + S), 1, &files).expect("tick");
        assert_eq!(again[0].item, Some(c));
        assert_ne!(again[0].packet[2..4], out[0].packet[2..4]);
        q.delivered(c).expect("ok");
        let next = q.tick(TaiNs(T0 + 2 * S), 5, &files).expect("tick");
        assert_eq!(next.iter().map(|o| o.item).collect::<Vec<_>>(), [Some(s)]);
    }

    /// The link monitor can declare LOS inside a scheduled window (and AOS outside one).
    #[test]
    fn link_override_suspends_and_resumes() {
        let mut files = Files::default();
        files.0.insert("f".into(), vec![1; 5_000]);
        let schedule = ContactSchedule::new(vec![window(T0, T0 + DAY)]).expect("schedule");
        let mut q = open(MemoryStore::default(), schedule, &files);
        q.enqueue(
            Channel::Medical,
            QueuePriority::Routine,
            Payload::File(FileItem::describe(
                "f",
                FileKind::Export,
                CfdpClass::Class2,
                &[1; 5_000],
            )),
            TaiNs(T0),
            &key(),
        )
        .expect("file");
        assert!(!q.tick(TaiNs(T0), 5, &files).expect("tick").is_empty());
        q.set_link_override(Some(false));
        assert!(q.tick(TaiNs(T0 + S), 5, &files).expect("tick").is_empty());
        assert!(q.cfdp_suspended());
        q.set_link_override(None);
        assert!(!q
            .tick(TaiNs(T0 + 2 * S), 5, &files)
            .expect("tick")
            .is_empty());
        assert!(!q.cfdp_suspended());
    }

    #[test]
    fn changed_file_content_is_refused() {
        let mut files = Files::default();
        files.0.insert("f".into(), vec![2; 10]);
        let schedule = ContactSchedule::new(vec![window(T0, T0 + DAY)]).expect("schedule");
        let mut q = open(MemoryStore::default(), schedule, &files);
        q.enqueue(
            Channel::Medical,
            QueuePriority::Routine,
            Payload::File(FileItem::describe(
                "f",
                FileKind::Export,
                CfdpClass::Class1,
                &[1; 10],
            )),
            TaiNs(T0),
            &key(),
        )
        .expect("file");
        assert!(matches!(
            q.tick(TaiNs(T0), 5, &files),
            Err(QueueError::FileMismatch(_))
        ));
    }
}
