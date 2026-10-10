//! Path B (M10 integration; F-GND-11, SR-011, SR-015, CR-016, docs/integration/M10.md §2): the
//! ground stage of every escalation goes straight to the vehicle as telemetry-only OPS packets
//! (crew, priority, category, state, stage, times; never physiology, docs/06 §3a). It never
//! depends on the medical data system. Items are signed with the instance key and persisted
//! before they count as accepted; a publish failure raises a technical alarm.

use std::path::{Path, PathBuf};

use caelum_alarms::escalation::Notice;
use caelum_alarms::signing::{built_in_keys, TrustPolicy};
use caelum_domain::time::TaiNs;
use caelum_ground::cfdp::CfdpConfig;
use caelum_ground::config::{ContactSchedule, GroundConfig};
use caelum_ground::publisher::{PathBPublisher, UdpSink};
use caelum_ground::queue::ItemSigner;
use caelum_ground::queue::{
    AllowLists, DownlinkQueue, FileItem, FileSource, QueueError, QueueStore, SignedItem,
};
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::keys::InstanceKey;
use caelum_security::role::Command;
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::json;

use super::{Station, StationError};

/// Configuration kind of the ground bundle (APID plan, path B endpoint, CFDP entities).
pub const GROUND_KIND: &str = "ground";
/// Technical alarm while OPS packets cannot be published.
pub const PATH_B_RULE: &str = "technical.path_b";

/// The publisher with its store and the vehicle endpoint.
pub type PathB = PathBPublisher<FileQueueStore, UdpSink>;

/// Path B status for the Ground screen (counts only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathBStatus {
    /// A signed ground configuration is in force and the publisher is open.
    pub configured: bool,
    /// Ground configuration version.
    pub version: Option<String>,
    /// OPS records waiting to be sent.
    pub waiting: usize,
    /// Why path B is not running.
    pub problem: Option<String>,
}

/// Durable queue store: one file per signed item and one state file, each written atomically.
/// OPS items carry no physiology by type (docs/06 §3a); integrity is the item signature, checked
/// when the queue opens.
pub struct FileQueueStore {
    dir: PathBuf,
}

impl FileQueueStore {
    /// A store in `dir` (created if missing).
    pub fn open(dir: &Path) -> Result<Self, QueueError> {
        std::fs::create_dir_all(dir).map_err(|e| QueueError::Store(e.to_string()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    fn write(&self, name: &str, text: &str) -> Result<(), QueueError> {
        let path = self.dir.join(name);
        let tmp = path.with_extension("tmp");
        let io = |e: std::io::Error| QueueError::Store(e.to_string());
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::File::open(&tmp)
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)
    }
}

impl QueueStore for FileQueueStore {
    fn insert(&mut self, item: &SignedItem) -> Result<(), QueueError> {
        let text = serde_json::to_string(item).map_err(|e| QueueError::Store(e.to_string()))?;
        self.write(&format!("item-{:020}.json", item.id), &text)
    }

    fn remove(&mut self, id: u64) -> Result<(), QueueError> {
        match std::fs::remove_file(self.dir.join(format!("item-{id:020}.json"))) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(QueueError::Store(e.to_string())),
        }
    }

    fn load(&self) -> Result<Vec<SignedItem>, QueueError> {
        let io = |e: std::io::Error| QueueError::Store(e.to_string());
        let mut names: Vec<String> = std::fs::read_dir(&self.dir)
            .map_err(io)?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("item-") && n.ends_with(".json"))
            .collect();
        names.sort();
        names
            .into_iter()
            .map(|n| {
                let text = std::fs::read_to_string(self.dir.join(&n)).map_err(io)?;
                serde_json::from_str(&text).map_err(|e| QueueError::Store(format!("{n}: {e}")))
            })
            .collect()
    }

    fn save_state(&mut self, json: &str) -> Result<(), QueueError> {
        self.write("state.json", json)
    }

    fn load_state(&self) -> Result<Option<String>, QueueError> {
        match std::fs::read_to_string(self.dir.join("state.json")) {
            Ok(t) => Ok(Some(t)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(QueueError::Store(e.to_string())),
        }
    }
}

/// Path B sends no files.
struct NoFiles;

impl FileSource for NoFiles {
    fn read(&self, _item: &FileItem) -> Result<Vec<u8>, QueueError> {
        Err(QueueError::Store("path B carries no files".to_owned()))
    }
}

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

/// Signs path B items with the instance key without exposing it.
struct InstanceSigner<'a>(&'a InstanceKey);

impl ItemSigner for InstanceSigner<'_> {
    fn key_id(&self) -> String {
        self.0.verifying_key().key_id()
    }
    fn sign_message(&self, msg: &[u8]) -> Signature {
        Signature::from_bytes(&self.0.sign(msg))
    }
    fn verifying_key(&self) -> VerifyingKey {
        self.0.public_key()
    }
}

impl Station {
    /// Opens the path B publisher from the ground configuration in force (none: path B is
    /// not configured and the status says so).
    pub(super) fn open_path_b(&mut self) -> Result<(), StationError> {
        let Some((_, signature, content)) = self.db.latest_config_bundle(GROUND_KIND)? else {
            self.path_b = None;
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
        let store = FileQueueStore::open(&self.data_dir.join("path-b"))
            .map_err(|e| rejected(e.to_string()))?;
        // The publisher sends no files: the CFDP settings are required by the queue type but
        // unused on path B (CR-016 §3).
        let cfdp = CfdpConfig {
            local_entity: config.cfdp.local_entity_id,
            remote_entity: config.cfdp.ground_entity_id,
            segment_len: 1024,
            ack_timer_ns: 10_000_000_000,
            ack_limit: 3,
            nak_timer_ns: 10_000_000_000,
            nak_limit: 3,
        };
        let queue = DownlinkQueue::open(
            store,
            config.apids.clone(),
            AllowLists::from_catalog(),
            ContactSchedule::default(),
            cfdp,
            self.key.public_key(),
            &NoFiles,
        )
        .map_err(|e| rejected(e.to_string()))?;
        let sink = UdpSink::new(config.path_b.endpoint).map_err(|e| rejected(e.to_string()))?;
        self.path_b = Some((config.version, PathBPublisher::new(queue, sink)));
        Ok(())
    }

    /// `config_apply_ground`: the signed ground configuration (administrator, re-authentication).
    /// Path B reopens on it. Audited.
    pub fn config_apply_ground(
        &mut self,
        bundle: &[u8],
        signature: &str,
    ) -> Result<String, StationError> {
        let user = self.admin_for(Command::ConfigApplyGround)?;
        let config = GroundConfig::load_signed(
            bundle,
            signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| rejected(format!("the ground configuration is not accepted: {e}")))?;
        let text = std::str::from_utf8(bundle).map_err(|_| rejected("not UTF-8 text"))?;
        let now = self.now()?;
        let digest = caelum_alarms::signing::digest(bundle);
        self.db.put_config_bundle(
            &format!("{GROUND_KIND}-{digest}"),
            GROUND_KIND,
            &config.version,
            signature,
            text,
            now.0,
        )?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some(GROUND_KIND),
            &json!({ "kind": GROUND_KIND, "version": config.version, "sha256": digest }),
        )?;
        self.open_path_b()?;
        Ok(config.version)
    }

    /// Publishes the ground stage of escalations in `notices` and flushes the backlog. A failure
    /// raises `technical.path_b`; success clears it. Never blocks onboard escalation.
    pub(super) fn publish_ops(
        &mut self,
        notices: &[Notice],
        now: TaiNs,
    ) -> Result<(), StationError> {
        let key = InstanceSigner(&self.key);
        let Some((_, publisher)) = self.path_b.as_mut() else {
            return Ok(());
        };
        let mut failed = false;
        for n in notices {
            if let Notice::Ops(e) = n {
                failed |= publisher.publish_escalation(e, &key).is_err();
            }
        }
        failed |= publisher.flush(now).map(|r| r.waiting > 0).unwrap_or(true);
        if failed != self.alarms.technical_open(PATH_B_RULE, None) {
            let why = failed.then(|| json!({ "path_b": "OPS packets are waiting: the vehicle endpoint did not take them" }));
            self.alarms
                .technical(&self.db, PATH_B_RULE, None, why, now)?;
        }
        Ok(())
    }

    /// Path B status (counts only; part of `ground_status`).
    pub fn path_b_status(&self) -> PathBStatus {
        match &self.path_b {
            Some((version, p)) => PathBStatus {
                configured: true,
                version: Some(version.clone()),
                waiting: p.waiting(),
                problem: None,
            },
            None => PathBStatus {
                configured: false,
                version: None,
                waiting: 0,
                problem: Some("no signed ground configuration".to_owned()),
            },
        }
    }

    /// `ground_status`: who may see the ground link status (all signed-in roles).
    pub fn ground_status_allowed(&mut self) -> Result<(), StationError> {
        self.user_for(Command::GroundStatus).map(|_| ())
    }

    /// `model_list` (F-ML-01): installed models and their validation status.
    pub fn model_list(
        &mut self,
    ) -> Result<Vec<caelum_ml::registry::ModelStatusView>, StationError> {
        self.user_for(Command::ModelList)?;
        Ok(self.models.statuses())
    }
}
