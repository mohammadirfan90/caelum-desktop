//! The onboard ground downlink (F-GND-01, F-GND-05, F-GND-06, F-GND-14; CR-016, CR-024): MEDICAL
//! parameter summaries and alarm event packets queued, signed and persisted, sent only inside
//! contact windows, files by CFDP (class 2: acknowledged, NAK-driven retransmission) toward the
//! ground endpoint of the signed ground configuration (ICD-VEH OI-1: a UDP endpoint through the
//! vehicle until the interface is agreed).
//!
//! The MEDICAL channel carries clinical detail to the flight surgeon and BME; OPS stays on path B
//! with OPS fields only (SR-015, R-SEC-06). Without a `ground_link` section nothing is queued.

use std::net::UdpSocket;
use std::path::PathBuf;

use caelum_alarms::engine::Event;
use caelum_alarms::signing::{built_in_keys, verify_config, TrustPolicy};
use caelum_domain::ids::CrewMemberId;
use caelum_domain::observation::Quality;
use caelum_domain::parameters::{Channel, ValueType, CATALOG};
use caelum_domain::time::TaiNs;
use caelum_ground::cfdp::{CfdpClass, CfdpConfig};
use caelum_ground::config::{ContactSchedule, GroundConfig, CONTACT_KIND};
use caelum_ground::medical::{downlinked, MedicalSummary, SummaryValue};
use caelum_ground::publisher::{PacketSink, UdpSink};
use caelum_ground::queue::{
    AllowLists, DownlinkQueue, FileItem, FileKind, FileSource, Payload, QueueError, QueuePriority,
};
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::Command;
use serde_json::json;

use super::pathb::{FileQueueStore, GROUND_KIND};
use super::{Station, StationError};

const NS: i64 = 1_000_000_000;
/// Technical alarm of the ground downlink (CR-011 table).
pub const GROUND_RULE: &str = "technical.ground_link";
/// Packets sent per tick at most.
const BUDGET: usize = 64;
/// Readings around an alarm in its event packet, s.
pub const EVENT_CONTEXT_S: i64 = 300;

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

/// Files waiting for CFDP, kept next to the queue until delivered.
struct DirFiles(PathBuf);

impl FileSource for DirFiles {
    fn read(&self, item: &FileItem) -> Result<Vec<u8>, QueueError> {
        std::fs::read(self.0.join(&item.name)).map_err(|e| QueueError::Store(e.to_string()))
    }
}

/// The running ground downlink.
pub(super) struct GroundLink {
    queue: DownlinkQueue<FileQueueStore>,
    sink: UdpSink,
    rx: UdpSocket,
    files: PathBuf,
    window_ns: i64,
    last_summary_end: Option<i64>,
    pub(super) version: String,
}

/// Downlink counts for the Ground screen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DownlinkStatus {
    /// A ground link is configured.
    pub configured: bool,
    /// Ground configuration version.
    pub version: Option<String>,
    /// Inside a contact window now.
    pub in_contact: bool,
    /// Items waiting.
    pub waiting: usize,
    /// CFDP transfers suspended (outside contact).
    pub suspended: bool,
}

impl Station {
    /// `config_apply_contact`: the signed contact-window schedule (F-GND-05). Audited.
    pub fn config_apply_contact(
        &mut self,
        bundle: &[u8],
        signature: &str,
    ) -> Result<String, StationError> {
        let user = self.admin_for(Command::ConfigApplyContact)?;
        ContactSchedule::load_signed(
            bundle,
            signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| rejected(format!("the contact schedule is not accepted: {e}")))?;
        let verified = verify_config(
            CONTACT_KIND,
            bundle,
            signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| rejected(e.to_string()))?;
        let text = std::str::from_utf8(bundle).map_err(|_| rejected("not UTF-8 text"))?;
        let now = self.now()?;
        self.db.put_config_bundle(
            &format!("{CONTACT_KIND}-{}", verified.sha256),
            CONTACT_KIND,
            &verified.version,
            signature,
            text,
            now.0,
        )?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some(CONTACT_KIND),
            &json!({ "kind": CONTACT_KIND, "version": verified.version, "sha256": verified.sha256 }),
        )?;
        self.open_ground_link()?;
        Ok(verified.version)
    }

    /// Opens (or closes) the ground downlink from the signed ground configuration and contact
    /// schedule in force.
    pub(super) fn open_ground_link(&mut self) -> Result<(), StationError> {
        self.ground = None;
        let Some((_, signature, content)) = self.db.latest_config_bundle(GROUND_KIND)? else {
            return Ok(());
        };
        let config = GroundConfig::load_signed(
            content.as_bytes(),
            &signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| {
            rejected(format!(
                "the stored ground configuration no longer verifies: {e}"
            ))
        })?;
        let Some(link) = config.ground_link else {
            return Ok(());
        };
        let schedule = match self.db.latest_config_bundle(CONTACT_KIND)? {
            Some((_, sig, text)) => ContactSchedule::load_signed(
                text.as_bytes(),
                &sig,
                &built_in_keys(),
                TrustPolicy::for_this_build(),
            )
            .map_err(|e| {
                rejected(format!(
                    "the stored contact schedule no longer verifies: {e}"
                ))
            })?,
            // No schedule: no contact, so everything waits (never sent blind).
            None => ContactSchedule::default(),
        };
        let dir = self.data_dir.join("ground");
        let files = dir.join("files");
        std::fs::create_dir_all(&files).map_err(|e| rejected(e.to_string()))?;
        let store = FileQueueStore::open(&dir).map_err(|e| rejected(e.to_string()))?;
        // CFDP class 2 timers (CR-016 §3): 10 s ACK and NAK timers, three tries, 1 KiB segments.
        let cfdp = CfdpConfig {
            local_entity: config.cfdp.local_entity_id,
            remote_entity: config.cfdp.ground_entity_id,
            segment_len: 1024,
            ack_timer_ns: 10 * NS,
            ack_limit: 3,
            nak_timer_ns: 10 * NS,
            nak_limit: 3,
        };
        let queue = DownlinkQueue::open(
            store,
            config.apids,
            AllowLists::from_catalog(),
            schedule,
            cfdp,
            self.key.public_key(),
            &DirFiles(files.clone()),
        )
        .map_err(|e| rejected(e.to_string()))?;
        let sink = UdpSink::new(link.endpoint).map_err(|e| rejected(e.to_string()))?;
        let rx = UdpSocket::bind(link.receive).map_err(|e| rejected(e.to_string()))?;
        rx.set_nonblocking(true)
            .map_err(|e| rejected(e.to_string()))?;
        self.ground = Some(GroundLink {
            queue,
            sink,
            rx,
            files,
            window_ns: i64::from(link.summary_window_s) * NS,
            last_summary_end: None,
            version: config.version,
        });
        Ok(())
    }

    /// The ground downlink status (counts only).
    pub fn downlink_status(&self) -> DownlinkStatus {
        match &self.ground {
            None => DownlinkStatus::default(),
            Some(g) => {
                let s = g.queue.stats();
                DownlinkStatus {
                    configured: true,
                    version: Some(g.version.clone()),
                    in_contact: !g.queue.cfdp_suspended(),
                    waiting: s.pending,
                    suspended: g.queue.cfdp_suspended(),
                }
            }
        }
    }

    /// One downlink round (on the tick): acknowledgements in, summaries of finished windows
    /// queued, packets out inside contact windows.
    pub(super) fn ground_tick(&mut self, now: TaiNs) -> Result<(), StationError> {
        if self.ground.is_none() {
            return Ok(());
        }
        let signer = super::pathb::signer(&self.key);
        // CFDP acknowledgements and NAKs from ground.
        if let Some(g) = self.ground.as_mut() {
            let mut buf = [0_u8; 4096];
            for _ in 0..BUDGET {
                match g.rx.recv(&mut buf) {
                    Ok(n) => {
                        let _ = g.queue.receive_pdu(now, &buf[..n]);
                    }
                    Err(_) => break,
                }
            }
        }
        // MEDICAL summaries of every finished window.
        let (window, last) = match &self.ground {
            Some(g) => (g.window_ns, g.last_summary_end),
            None => return Ok(()),
        };
        let end = now.0 - now.0.rem_euclid(window);
        if last.is_none_or(|l| end > l) {
            let start = last.map_or(end - window, |l| l.max(end - window));
            let summaries = self.summaries(start, end)?;
            if let Some(g) = self.ground.as_mut() {
                for s in summaries {
                    g.queue
                        .enqueue(
                            Channel::Medical,
                            QueuePriority::Routine,
                            Payload::Summary(s),
                            now,
                            &signer,
                        )
                        .map_err(|e| rejected(e.to_string()))?;
                }
                g.last_summary_end = Some(end);
            }
        }
        if let Some(g) = self.ground.as_mut() {
            let out = g
                .queue
                .tick(now, BUDGET, &DirFiles(g.files.clone()))
                .map_err(|e| rejected(e.to_string()))?;
            for o in out {
                if g.sink.send(&o.packet).is_err() {
                    break;
                }
                if let Some(id) = o.item {
                    g.queue.delivered(id).map_err(|e| rejected(e.to_string()))?;
                }
            }
        }
        Ok(())
    }

    /// Summaries of `[start, end)` for every active crew member and downlinked parameter with
    /// readings. A parameter with no reading in the window sends nothing (never a made-up value).
    fn summaries(&self, start: i64, end: i64) -> Result<Vec<MedicalSummary>, StationError> {
        let mut out = Vec::new();
        let crews: Vec<String> = self
            .db
            .crew_members()?
            .into_iter()
            .filter(|c| c.active)
            .map(|c| c.id)
            .collect();
        for crew in crews {
            let Ok(crew_id) = crew.parse::<CrewMemberId>() else {
                continue;
            };
            for spec in CATALOG.iter().filter(|s| downlinked(s.parameter)) {
                let rows: Vec<_> = self
                    .db
                    .observations_between(&crew, spec.key, start, end - 1)?
                    .into_iter()
                    .filter(|r| r.classification != "simulated")
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                let quality = rows
                    .iter()
                    .map(|r| match r.quality.as_str() {
                        "good" => Quality::Good,
                        "degraded" => Quality::Degraded,
                        "poor" => Quality::Poor,
                        _ => Quality::Unusable,
                    })
                    .max()
                    .unwrap_or(Quality::Unusable);
                let count = u16::try_from(rows.len()).unwrap_or(u16::MAX);
                let value = match spec.value {
                    ValueType::Numeric { .. } => {
                        let v: Vec<f64> = rows.iter().filter_map(|r| r.value).collect();
                        if v.is_empty() {
                            continue;
                        }
                        let min = v.iter().copied().fold(f64::INFINITY, f64::min);
                        let max = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                        let mean = v.iter().sum::<f64>() / v.len() as f64;
                        SummaryValue::Numeric {
                            min: min as f32,
                            mean: mean as f32,
                            max: max as f32,
                            count: u16::try_from(v.len()).unwrap_or(u16::MAX),
                        }
                    }
                    ValueType::Enum(states) => {
                        let Some(last) = rows
                            .last()
                            .and_then(|r| r.value_text.as_deref())
                            .and_then(|t| states.iter().position(|s| *s == t))
                        else {
                            continue;
                        };
                        SummaryValue::State {
                            last: u8::try_from(last).unwrap_or(u8::MAX),
                            count,
                        }
                    }
                    ValueType::EnumPending(_) => continue,
                };
                let s = MedicalSummary {
                    crew_id,
                    parameter: spec.parameter,
                    quality,
                    window_start: TaiNs(start),
                    window_end: TaiNs(end),
                    value,
                };
                if s.validate().is_ok() {
                    out.push(s);
                }
            }
        }
        Ok(out)
    }

    /// Event packets (F-GND-14): for every critical or warning physiological alarm raised, its
    /// detail and the readings of the 5 minutes before and after, sent by CFDP class 2.
    pub(super) fn ground_events(
        &mut self,
        events: &[Event],
        now: TaiNs,
    ) -> Result<(), StationError> {
        if self.ground.is_none() {
            return Ok(());
        }
        let signer = super::pathb::signer(&self.key);
        for e in events {
            let Event::Raised(a) = e else { continue };
            let Some(view) = self.db.alarm_view(&a.id.to_string())? else {
                continue;
            };
            if view.kind != "physiological"
                || !matches!(view.priority.as_str(), "critical" | "warning")
            {
                continue;
            }
            let Some(crew) = view.crew_member_id.clone() else {
                continue;
            };
            let from = view.created_at_tai - EVENT_CONTEXT_S * NS;
            let to = view.created_at_tai + EVENT_CONTEXT_S * NS;
            let mut readings = Vec::new();
            for spec in CATALOG
                .iter()
                .filter(|s| s.channels.contains(&Channel::Medical))
            {
                for r in self.db.observations_between(&crew, spec.key, from, to)? {
                    if r.classification == "simulated" {
                        continue;
                    }
                    readings.push(json!({
                        "parameter": r.parameter, "value": r.value, "text": r.value_text, "unit": r.unit,
                        "measured_at_tai": r.measured_at_tai, "quality": r.quality,
                        "classification": r.classification, "algorithm": r.algorithm_version,
                    }));
                }
            }
            let why: serde_json::Value =
                serde_json::from_str(&view.why).unwrap_or(serde_json::Value::Null);
            let packet = json!({
                "format": "caelum-event-packet/1",
                "alarm": {
                    "id": view.id, "crew_member_id": crew, "rule_id": view.rule_id,
                    "ruleset_version": view.ruleset_version, "priority": view.priority,
                    "category": view.category, "title": view.title, "trigger_value": view.trigger_value,
                    "why": why, "created_at_tai": view.created_at_tai,
                },
                "readings": readings,
            });
            let content = serde_json::to_vec(&packet).map_err(|e| rejected(e.to_string()))?;
            let name = format!("event-{}.json", view.id);
            let Some(g) = self.ground.as_mut() else {
                continue;
            };
            std::fs::write(g.files.join(&name), &content).map_err(|e| rejected(e.to_string()))?;
            let item =
                FileItem::describe(&name, FileKind::EventPacket, CfdpClass::Class2, &content);
            g.queue
                .enqueue(
                    Channel::Medical,
                    QueuePriority::Routine,
                    Payload::File(item),
                    now,
                    &signer,
                )
                .map_err(|e| rejected(e.to_string()))?;
        }
        Ok(())
    }
}
