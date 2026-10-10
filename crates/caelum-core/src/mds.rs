//! The medical data system service (F-GND-12, F-GND-13, F-GND-14, SR-016; CR-016,
//! docs/integration/M10.md §2): summary-first observation records to the onboard medical data
//! system over FHIR R4 (path A), read-back of its observations, and its health.
//!
//! It runs on its own connection to the encrypted database, outside the station lock, so a slow
//! or absent medical data system never delays alarm handling. Alarms never depend on it: they
//! go to the crew locally and to ground on path B (SR-015). Read-back observations are handed to
//! the station for normal storage and alarm evaluation.

use std::collections::BTreeMap;

use caelum_alarms::signing::{built_in_keys, verify_config, TrustPolicy};
use caelum_domain::ids::CrewMemberId;
use caelum_domain::parameters::Parameter;
use caelum_domain::time::{LeapSeconds, TaiNs};
use caelum_mds::connector::{ConnectorConfig, MdsConnector, TickReport};
use caelum_mds::fhir::{self, Effective, InboundObservation, ObservationRecord, ObservedValue};
use caelum_mds::health::{HealthConfig, MdsHealth, TechnicalAlarmSignal};
use caelum_mds::ids::{ClientId, ResourceKind};
use caelum_mds::inbox::InboxSnapshot;
use caelum_mds::outbox::{Backoff, OutboxConfig, OutboxItem, OutboxRecord, Priority};
use caelum_mds::routing::{FallbackPolicy, ItemKind};
use caelum_mds::store::{InboxStore, OutboxStore, StoreError as MdsStoreError};
use caelum_mds::transport::FhirTransport;
use caelum_store::Database;
use serde::Deserialize;

/// Configuration kind of the medical data system connection.
pub const MDS_KIND: &str = "mds";
/// State keys.
const HIGH_WATER: &str = "inbox.high_water";
const SUMMARY_END: &str = "summary.end_tai";
/// State keys of the client identity (set by an administrator; CR-016 §10).
pub const IDENTITY_CERT: &str = "identity.cert_pem";
/// Private key of the client identity (in the encrypted database, never logged).
pub const IDENTITY_KEY: &str = "identity.key_pem";

const NS: i64 = 1_000_000_000;

/// The signed `mds` bundle (CR-016 §10). Every value is required: no defaults.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MdsConfig {
    /// Bundle version.
    pub version: String,
    /// FHIR base URL of the medical data system (`https://`).
    pub base_url: String,
    /// Mission CA bundle (PEM) that signs the medical data system's certificate.
    pub ca_pem: String,
    /// Per-request timeout, ms.
    pub timeout_ms: u64,
    /// `hold_until_reconnect` or `direct_cfdp` (path A', CR-016).
    pub policy: String,
    /// Read-back poll interval, s.
    pub poll_interval_s: u32,
    /// Read-back page size.
    pub page_size: u32,
    /// Records sent per tick at most.
    pub max_sends_per_tick: u32,
    /// Summary window, s (F-GND-14: summaries by default, raw data only on event).
    pub summary_window_s: u32,
    /// Outage time before the technical alarm, s.
    pub outage_alarm_after_s: u32,
    /// Retry back-off for alarm-priority records, s.
    pub alarm_backoff_s: [u32; 2],
    /// Retry back-off for routine records, s.
    pub routine_backoff_s: [u32; 2],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MdsFile {
    mds: MdsConfig,
}

impl MdsConfig {
    /// Parses and checks the bundle content.
    pub fn parse(text: &str) -> Result<Self, String> {
        let f: MdsFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let c = f.mds;
        if !c.base_url.starts_with("https://") {
            return Err("base_url must be https://".into());
        }
        if c.policy != "hold_until_reconnect" && c.policy != "direct_cfdp" {
            return Err("policy is hold_until_reconnect or direct_cfdp".into());
        }
        if c.timeout_ms == 0
            || c.poll_interval_s == 0
            || c.page_size == 0
            || c.max_sends_per_tick == 0
            || c.summary_window_s == 0
        {
            return Err("timeouts, intervals and sizes must be greater than zero".into());
        }
        if c.alarm_backoff_s[0] == 0
            || c.alarm_backoff_s[0] > c.alarm_backoff_s[1]
            || c.routine_backoff_s[0] == 0
            || c.routine_backoff_s[0] > c.routine_backoff_s[1]
        {
            return Err("back-off is [base, max] with 0 < base ≤ max".into());
        }
        Ok(c)
    }

    /// The fallback policy.
    pub fn fallback(&self) -> FallbackPolicy {
        if self.policy == "direct_cfdp" {
            FallbackPolicy::DirectCfdp
        } else {
            FallbackPolicy::HoldUntilReconnect
        }
    }

    fn connector(&self) -> ConnectorConfig {
        let b = |s: [u32; 2]| Backoff {
            base_ns: i64::from(s[0]) * NS,
            max_ns: i64::from(s[1]) * NS,
        };
        ConnectorConfig {
            outbox: OutboxConfig {
                alarm: b(self.alarm_backoff_s),
                routine: b(self.routine_backoff_s),
            },
            health: HealthConfig {
                outage_alarm_after_ns: i64::from(self.outage_alarm_after_s) * NS,
            },
            poll_interval_ns: i64::from(self.poll_interval_s) * NS,
            page_size: self.page_size,
            max_sends_per_tick: self.max_sends_per_tick,
        }
    }
}

/// The connector's persistence on its own database connection.
pub struct DbMdsStore {
    db: Database,
}

impl DbMdsStore {
    /// A store on `db` (its own connection to the station database).
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// The connection.
    pub fn db(&self) -> &Database {
        &self.db
    }
}

fn store_err(e: impl std::fmt::Display) -> MdsStoreError {
    MdsStoreError(e.to_string())
}

impl OutboxStore for DbMdsStore {
    fn put_record(&mut self, record: &OutboxRecord) -> Result<(), MdsStoreError> {
        let json = serde_json::to_string(record).map_err(store_err)?;
        self.db
            .mds_put_outbox(&record.client_id.to_string(), &json)
            .map_err(store_err)
    }

    fn load_records(&mut self) -> Result<Vec<OutboxRecord>, MdsStoreError> {
        self.db
            .mds_outbox()
            .map_err(store_err)?
            .into_iter()
            .map(|j| serde_json::from_str(&j).map_err(store_err))
            .collect()
    }
}

impl InboxStore for DbMdsStore {
    fn load_inbox(&mut self) -> Result<InboxSnapshot, MdsStoreError> {
        Ok(InboxSnapshot {
            high_water: self.db.mds_state(HIGH_WATER).map_err(store_err)?,
            seen: self.db.mds_seen().map_err(store_err)?,
        })
    }

    fn put_seen(&mut self, source_id: &str, version: &str) -> Result<(), MdsStoreError> {
        self.db.mds_put_seen(source_id, version).map_err(store_err)
    }

    fn put_high_water(&mut self, last_updated: &str) -> Result<(), MdsStoreError> {
        self.db
            .mds_set_state(HIGH_WATER, last_updated)
            .map_err(store_err)
    }
}

/// What one sync did, for the shell to apply to the station and show.
#[derive(Debug, Default)]
pub struct SyncOutcome {
    /// Read-back observations to store and evaluate through the station (SR-016).
    pub inbound: Vec<InboundObservation>,
    /// Raise (`Some(true)`) or clear (`Some(false)`) `technical.mds_unreachable`.
    pub technical_alarm: Option<bool>,
    /// Summary records enqueued in this sync.
    pub enqueued: usize,
    /// The tick report, if the connector ran.
    pub report: Option<TickReport>,
}

/// Status for the Ground screen and BME (counts only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdsStatus {
    /// A signed `mds` configuration and a client identity are in force.
    pub configured: bool,
    /// Configuration version.
    pub version: Option<String>,
    /// `connected` / `degraded` / `unreachable` / `not_configured`.
    pub health: &'static str,
    /// Outbox records not yet acknowledged.
    pub pending: usize,
    /// Records held for review (4xx).
    pub held: usize,
    /// Why it is not running.
    pub problem: Option<String>,
}

enum Running {
    Idle(DbMdsStore),
    Connected {
        connector: Box<MdsConnector<DbMdsStore>>,
        config: MdsConfig,
    },
}

/// The service.
pub struct MdsService {
    state: Option<Running>,
    leap: LeapSeconds,
    problem: Option<String>,
}

fn health_name(h: MdsHealth) -> &'static str {
    match h {
        MdsHealth::Connected => "connected",
        MdsHealth::Degraded => "degraded",
        MdsHealth::Unreachable => "unreachable",
        MdsHealth::NotConfigured => "not_configured",
    }
}

impl MdsService {
    /// A service on its own database connection.
    pub fn new(db: Database, leap: LeapSeconds) -> Self {
        Self {
            state: Some(Running::Idle(DbMdsStore::new(db))),
            leap,
            problem: None,
        }
    }

    fn db(&self) -> Option<&Database> {
        match &self.state {
            Some(Running::Idle(s)) => Some(s.db()),
            Some(Running::Connected { connector, .. }) => Some(connector.store().db()),
            None => None,
        }
    }

    /// The signed configuration in force (re-verified), if any.
    pub fn config_in_force(&self) -> Result<Option<MdsConfig>, String> {
        let Some(db) = self.db() else { return Ok(None) };
        let Some((version, signature, content)) = db
            .latest_config_bundle(MDS_KIND)
            .map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        verify_config(
            MDS_KIND,
            content.as_bytes(),
            &signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| format!("the stored mds configuration no longer verifies: {e}"))?;
        let c = MdsConfig::parse(&content)?;
        Ok((c.version == version).then_some(c))
    }

    /// The client identity (certificate PEM, key PEM), if an administrator set one.
    pub fn identity(&self) -> Result<Option<(String, zeroize::Zeroizing<String>)>, String> {
        let Some(db) = self.db() else { return Ok(None) };
        let cert = db.mds_state(IDENTITY_CERT).map_err(|e| e.to_string())?;
        let key = db.mds_state(IDENTITY_KEY).map_err(|e| e.to_string())?;
        Ok(cert.zip(key).map(|(c, k)| (c, zeroize::Zeroizing::new(k))))
    }

    /// Brings the connector in line with the configuration in force (start, restart on a new
    /// version, stop when removed). Returns the configuration to build the transport from.
    pub fn refresh(&mut self, now: TaiNs) -> Option<MdsConfig> {
        let config = match self.config_in_force() {
            Ok(c) => c,
            Err(e) => {
                self.problem = Some(e);
                None
            }
        };
        let identity_ok = matches!(self.identity(), Ok(Some(_)));
        let state = self.state.take()?;
        let store = match state {
            Running::Connected {
                connector,
                config: old,
            } => {
                if config.as_ref() == Some(&old) && identity_ok {
                    self.state = Some(Running::Connected {
                        connector,
                        config: old,
                    });
                    return config;
                }
                (*connector).into_store()
            }
            Running::Idle(s) => s,
        };
        match config {
            Some(c) if identity_ok => {
                match MdsConnector::load(store, self.leap.clone(), c.connector(), now) {
                    Ok(connector) => {
                        self.problem = None;
                        self.state = Some(Running::Connected {
                            connector: Box::new(connector),
                            config: c.clone(),
                        });
                        Some(c)
                    }
                    Err(e) => {
                        // The store went into the connector; the error leaves no state to return to.
                        self.problem = Some(e.to_string());
                        None
                    }
                }
            }
            Some(_) => {
                self.problem = Some("no client identity set".to_owned());
                self.state = Some(Running::Idle(store));
                None
            }
            None => {
                self.state = Some(Running::Idle(store));
                None
            }
        }
    }

    /// Enqueues the summaries of closed windows (F-GND-14), then runs one connector tick.
    pub fn sync(&mut self, now: TaiNs, transport: &mut dyn FhirTransport) -> SyncOutcome {
        let mut out = SyncOutcome::default();
        let leap = self.leap.clone();
        let Some(Running::Connected { connector, config }) = self.state.as_mut() else {
            return out;
        };
        let window = i64::from(config.summary_window_s) * NS;
        let policy = config.fallback();
        let db = connector.store().db();
        let start = db
            .mds_state(SUMMARY_END)
            .ok()
            .flatten()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(now.0 - window);
        let mut items = Vec::new();
        let mut end = start;
        while end + window <= now.0 {
            let (a, b) = (end, end + window);
            items.extend(summaries(db, a, b, &leap));
            end = b;
        }
        let routes = connector.route(ItemKind::ObservationSummary, policy, now);
        if routes.uses_outbox() {
            for item in items {
                if connector.enqueue(item, now).is_ok() {
                    out.enqueued += 1;
                }
            }
        }
        if end != start {
            let _ = connector
                .store()
                .db()
                .mds_set_state(SUMMARY_END, &end.to_string());
        }
        let report = connector.tick(now, transport);
        out.inbound = report.inbound().cloned().collect();
        out.technical_alarm = report
            .technical_alarm
            .map(|s| matches!(s, TechnicalAlarmSignal::Raise { .. }));
        out.report = Some(report);
        out
    }

    /// Status (counts only).
    pub fn status(&self, now: TaiNs) -> MdsStatus {
        match &self.state {
            Some(Running::Connected { connector, config }) => MdsStatus {
                configured: true,
                version: Some(config.version.clone()),
                health: health_name(connector.health(now)),
                pending: connector.outbox().unsent_count(),
                held: connector.outbox().held().count(),
                problem: None,
            },
            _ => MdsStatus {
                configured: false,
                version: None,
                health: health_name(MdsHealth::NotConfigured),
                pending: 0,
                held: 0,
                problem: self.problem.clone(),
            },
        }
    }
}

/// Median per crew member and parameter over `[a, b)`, for numeric, non-simulated readings.
fn summaries(db: &Database, a: i64, b: i64, leap: &LeapSeconds) -> Vec<OutboxItem> {
    let Ok(crews) = db.crew_members() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for c in crews.into_iter().filter(|c| c.active) {
        let Ok(crew) = c.id.parse::<CrewMemberId>() else {
            continue;
        };
        let Ok(rows) = db.observations_for_crew_between(&c.id, a, b - 1) else {
            continue;
        };
        let mut by_param: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for r in rows {
            if r.classification == "simulated" || r.quality == "unusable" {
                continue;
            }
            if let Some(v) = r.value {
                by_param.entry(r.parameter).or_default().push(v);
            }
        }
        for (param, mut values) in by_param {
            let Some(parameter) = Parameter::from_key(&param) else {
                continue;
            };
            values.sort_by(f64::total_cmp);
            let mid = values.len() / 2;
            let median = if values.len().is_multiple_of(2) {
                (values[mid - 1] + values[mid]) / 2.0
            } else {
                values[mid]
            };
            let rec = ObservationRecord {
                parameter,
                value: ObservedValue::Quantity(median),
                effective: Effective::Period {
                    start: TaiNs(a),
                    end: TaiNs(b),
                },
                crew,
                device: None,
            };
            let id = ClientId::derive(
                ResourceKind::Observation,
                &format!("summary/{crew}/{param}/{a}/{b}"),
            );
            if let Ok(resource) = fhir::observation(&rec, id, leap) {
                out.push(OutboxItem {
                    client_id: id,
                    kind: ResourceKind::Observation,
                    priority: Priority::Routine,
                    resource,
                });
            }
        }
    }
    out
}
