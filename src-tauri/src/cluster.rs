//! Fail-over pair and crew-LAN service (F-SYS-04, F-SYS-05, R-OPS-02; CR-017,
//! docs/integration/M11.md).
//!
//! Without a signed `cluster` configuration the instance runs on its own (`standalone`). With
//! one, the node starts at application start on its own database connections and runtime tasks,
//! serves the crew LAN to provisioned stations, and reports its role for the status footer.
//! Anything that keeps a configured instance from running raises `technical.cluster`; nothing is
//! started in its place.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use caelum_cluster::lan::server::ClusterStatus;
use caelum_cluster::lan::tls::TlsMaterial;
use caelum_cluster::log::InstanceSigner;
use caelum_cluster::node::{Node, NodeConfig, NodeEvent, NodeParts};
use caelum_core::cluster::{
    config_in_force, identity, pending_rows, row_applier, trusted_signers, DbFenceStore, DbLogStore,
    LanAlarms, LanAuthority,
};
use caelum_core::security::clock::Clock as _;
use caelum_core::security::clock::SystemClock;
use caelum_core::security::keys::{database_key, instance_key, IfMissing};
use caelum_core::security::keystore::OsKeyStore;
use caelum_core::station::DATABASE_FILE;
use caelum_core::store::Database;
use tauri::{AppHandle, Manager};
use tauri_specta::Event as _;

use crate::events::AlarmBusEvent;
use crate::state::AppState;

/// What the footer shows when no node runs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NotRunning {
    /// No configuration: a single instance.
    Standalone,
    /// Configured, but it could not start (the technical alarm says why).
    Failed,
}

/// The running node, if any.
#[derive(Default)]
pub struct ClusterState {
    node: Mutex<Option<Node>>,
    not_running: Mutex<Option<NotRunning>>,
}

impl ClusterState {
    /// The instance role for the footer: `standalone`, `primary`, `promoting`, `standby`, or
    /// `unavailable` when a configured instance is not running.
    pub fn role(&self) -> String {
        if let Some(n) = self
            .node
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            return role_name(&n.status());
        }
        match *self
            .not_running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            Some(NotRunning::Failed) => "unavailable".to_owned(),
            Some(NotRunning::Standalone) | None => "standalone".to_owned(),
        }
    }
}

fn role_name(s: &ClusterStatus) -> String {
    serde_json::to_value(s.role)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unavailable".to_owned())
}

fn open(data_dir: &Path) -> Result<Database, String> {
    let keystore = OsKeyStore::open().map_err(|e| e.to_string())?;
    let (key, _) = database_key(&keystore, IfMissing::Fail).map_err(|e| e.to_string())?;
    Database::open(&data_dir.join(DATABASE_FILE), &key).map_err(|e| e.to_string())
}

fn clock() -> Result<Box<SystemClock>, String> {
    SystemClock::new().map(Box::new).map_err(|e| e.to_string())
}

/// Everything a node needs, from the configuration in force; `Ok(None)` when not configured.
fn prepare(data_dir: &Path) -> Result<Option<(NodeConfig, NodeParts, Arc<LanAlarms>)>, String> {
    let db = open(data_dir)?;
    let Some(plan) = config_in_force(&db)? else {
        return Ok(None);
    };
    let (chain, key_pem) = identity(&db)?
        .ok_or("the crew-LAN certificate and key are not set (cluster_identity_set)")?;
    let tls = TlsMaterial::from_pem(&plan.config.ca_pem, &chain, &key_pem)
        .map_err(|e| format!("crew-LAN certificate: {e}"))?;
    let keystore = OsKeyStore::open().map_err(|e| e.to_string())?;
    let (own_key, _) = instance_key(&keystore, IfMissing::Fail).map_err(|e| e.to_string())?;
    let (audit_key, _) = instance_key(&keystore, IfMissing::Fail).map_err(|e| e.to_string())?;
    let trust = trusted_signers(&plan, &own_key)?;
    let alarms = Arc::new(LanAlarms::new(open(data_dir)?, clock()?));
    let mut config = NodeConfig::new(plan.node.clone(), plan.listen);
    config.peer = plan.peer.as_ref().map(|(p, _)| p.clone());
    let parts = NodeParts {
        log: Box::new(DbLogStore::new(open(data_dir)?, clock()?, Some(row_applier()))),
        fences: Box::new(DbFenceStore::new(db)),
        signer: Box::new(InstanceSigner::new(plan.node.clone(), own_key)),
        trust,
        tls,
        registry: plan.registry,
        authority: Arc::new(LanAuthority::new(open(data_dir)?, audit_key, clock()?)),
        alarms: alarms.clone(),
    };
    Ok(Some((config, parts, alarms)))
}

/// Rows sent per pump round at most.
const PUMP_BATCH: i64 = 256;
/// Pump interval while primary.
const PUMP_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// Acts again after a failure, so alarms never stay off because the pair could not run.
fn act_alone(app: &AppHandle, reason: &str) {
    tracing::error!(%reason, "the configured instance cannot run as a pair; acting alone");
    *app.state::<ClusterState>()
        .not_running
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(NotRunning::Failed);
    let _ = app.state::<AppState>().with(|s| Ok(s.promote()?));
    technical(app, Some(reason));
}

/// The primary's replication pump: queued row changes become signed log entries, which the node
/// ships to the standby. A change leaves the queue only once appended; a crash in between sends
/// it again, which the standby's upsert absorbs.
async fn pump(app: AppHandle, db: Database, clock: Box<SystemClock>) {
    loop {
        tokio::time::sleep(PUMP_EVERY).await;
        let primary = app
            .state::<ClusterState>()
            .node
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|n| n.status().role == caelum_cluster::failover::InstanceRole::Primary);
        if !primary {
            continue;
        }
        let Ok(now) = clock.now_tai() else { continue };
        let (last, bodies) = match pending_rows(&db, PUMP_BATCH, now.0) {
            Ok(r) => r,
            Err(e) => {
                technical(&app, Some(&format!("replication queue: {e}")));
                continue;
            }
        };
        let Some(last) = last else { continue };
        let mut sent = true;
        {
            let cluster = app.state::<ClusterState>();
            let guard = cluster
                .node
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(node) = guard.as_ref() else { continue };
            for body in bodies {
                if node.append(body).is_err() {
                    sent = false;
                    break;
                }
            }
        }
        if sent {
            if let Err(e) = db.cluster_outbox_done(last) {
                technical(&app, Some(&format!("replication queue: {e}")));
            }
        }
    }
}

fn technical(app: &AppHandle, reason: Option<&str>) {
    let _ = app
        .state::<AppState>()
        .with(|s| Ok(s.cluster_technical(reason)?));
}

/// Starts the node if a cluster configuration is in force (call once in `setup`).
pub fn start_cluster(app: AppHandle, data_dir: &Path) {
    app.manage(ClusterState::default());
    let prepared = match prepare(data_dir) {
        Ok(p) => p,
        Err(reason) => {
            act_alone(&app, &reason);
            return;
        }
    };
    let Some((config, parts, alarms)) = prepared else {
        *app.state::<ClusterState>()
            .not_running
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(NotRunning::Standalone);
        return;
    };
    // A configured instance starts as the standby: it acts only once the node promotes it, so a
    // pair never evaluates alarms twice (F-SYS-04).
    let _ = app.state::<AppState>().with(|s| {
        s.stand_by();
        Ok(())
    });
    let pump_db = match open(data_dir).and_then(|db| Ok((db, clock()?))) {
        Ok(p) => Some(p),
        Err(reason) => {
            act_alone(&app, &format!("replication queue: {reason}"));
            return;
        }
    };
    let capture_db = match open(data_dir) {
        Ok(db) => db,
        Err(reason) => {
            act_alone(&app, &format!("replication queue: {reason}"));
            return;
        }
    };
    // Every alarm change on this station goes to the crew-LAN stream (OPS fields; detail only
    // to viewers allowed it, decided by caelum-cluster).
    let forward = alarms.clone();
    AlarmBusEvent::listen_any(&app, move |e| {
        let id = match &e.payload {
            AlarmBusEvent::Alarm { alarm, .. }
            | AlarmBusEvent::Ended { alarm, .. }
            | AlarmBusEvent::Stage { alarm, .. } => alarm.clone(),
            AlarmBusEvent::Sound { .. } | AlarmBusEvent::SoundOff { .. } => return,
        };
        forward.publish(&id);
    });
    tauri::async_runtime::spawn(async move {
        let node = match Node::start(config, parts).await {
            Ok(n) => n,
            Err(e) => {
                act_alone(&app, &format!("the instance could not start: {e}"));
                return;
            }
        };
        if let Some((db, clock)) = pump_db {
            tauri::async_runtime::spawn(pump(app.clone(), db, clock));
        }
        let mut events = node.events();
        *app.state::<ClusterState>()
            .node
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(node);
        technical(&app, None);
        loop {
            let event = match events.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    technical(&app, Some(&format!("{n} fail-over events were missed")));
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            match event {
                NodeEvent::BecamePrimary { fence, silence } => {
                    tracing::warn!(%fence, ?silence, "this instance is now primary");
                    // The first primary of a pair queues what it held before the pair existed;
                    // a promoted standby already holds the replicated record.
                    let fresh = app
                        .state::<ClusterState>()
                        .node
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .as_ref()
                        .is_some_and(|n| n.status().head_seq == 0);
                    let capture = capture_db.cluster_set_capture(true).and_then(|()| {
                        if fresh {
                            capture_db.cluster_seed_outbox().map(|_| ())
                        } else {
                            Ok(())
                        }
                    });
                    if let Err(e) = capture {
                        technical(&app, Some(&format!("replication capture: {e}")));
                    }
                    if let Err(e) = app.state::<AppState>().with(|s| Ok(s.promote()?)) {
                        technical(&app, Some(&format!("promotion: {}", e.message)));
                    }
                }
                NodeEvent::ReconnectDevices { fence, .. } => {
                    tracing::warn!(%fence, "hand-off: devices reconnect to this instance");
                }
                NodeEvent::Demoted { from, to } => {
                    tracing::warn!(%from, %to, "this instance was fenced and is now standby");
                    let _ = capture_db.cluster_set_capture(false);
                    let _ = app.state::<AppState>().with(|s| {
                        s.stand_by();
                        Ok(())
                    });
                }
                NodeEvent::Diverted { entries } => technical(
                    &app,
                    Some(&format!(
                        "{} unacknowledged entries were moved to quarantine for review",
                        entries.len()
                    )),
                ),
                NodeEvent::ReplicationRefused { reason } => {
                    technical(&app, Some(&format!("replication refused: {reason}")));
                }
                NodeEvent::StoreFailed { reason } => {
                    technical(&app, Some(&format!("store failed: {reason}")));
                }
            }
        }
    });
}

/// Applies the signed cluster configuration (administrator, re-authentication). Takes effect at
/// the next start.
#[tauri::command]
#[specta::specta]
pub async fn config_apply_cluster(
    state: tauri::State<'_, AppState>,
    bundle: String,
    signature: String,
) -> Result<String, crate::commands::CommandError> {
    state.with(|s| Ok(s.config_apply_cluster(bundle.as_bytes(), &signature)?))
}

/// Sets this station's crew-LAN certificate chain and key (administrator, re-authentication).
/// Takes effect at the next start.
#[tauri::command]
#[specta::specta]
pub async fn cluster_identity_set(
    state: tauri::State<'_, AppState>,
    chain_pem: String,
    key_pem: String,
) -> Result<(), crate::commands::CommandError> {
    let key = zeroize::Zeroizing::new(key_pem);
    state.with(|s| Ok(s.cluster_identity_set(&chain_pem, &key)?))
}
