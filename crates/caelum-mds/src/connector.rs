//! The medical data system connector: outbox + inbox + health over a [`FhirTransport`]
//! (F-GND-12, F-GND-13, SR-015, SR-016, R-INT-05, R-INT-06, R-INT-07; ICD-MDS §4, §6, §9).
//!
//! Sans-I/O: the caller supplies the transport and the time on every [`MdsConnector::tick`]; the
//! connector owns its persistent store. One tick:
//! 1. returns records left in flight by an earlier store failure to `Pending`;
//! 2. sends the due outbox records, alarm-related first, honouring each record's back-off; a link
//!    failure (unreachable, timeout, 5xx, protocol error) stops sending for this tick;
//! 3. polls read-back when due (and the link did not just fail) and de-duplicates (SR-016);
//! 4. updates health and returns technical-alarm changes (ICD-MDS §9).
//!
//! Patient alarms never wait for this connector: alarm items are routed to path B by
//! [`crate::routing::route`] before and independently of the outbox (SR-015, R-INT-06). Inbound
//! observations returned in [`TickReport`] must go through the normal ingest validation and alarm
//! evaluation (SR-016).

use caelum_domain::time::{LeapSeconds, TaiNs};

use crate::fhir::InboundObservation;
use crate::health::{HealthConfig, HealthMonitor, MdsHealth, TechnicalAlarmSignal};
use crate::ids::ClientId;
use crate::inbox::{Inbox, InboxError, PollResult};
use crate::outbox::{Disposition, EnqueueOutcome, Outbox, OutboxConfig, OutboxError, OutboxItem};
use crate::routing::{route, FallbackPolicy, ItemKind, Routes};
use crate::store::{InboxStore, OutboxStore};
use crate::transport::{FhirTransport, TransportError};

/// A store for both the outbox and the read-back state.
pub trait MdsStore: OutboxStore + InboxStore {}

impl<T: OutboxStore + InboxStore + ?Sized> MdsStore for T {}

/// Connector configuration (signed mission configuration; no built-in defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectorConfig {
    /// Outbox back-off.
    pub outbox: OutboxConfig,
    /// Health / technical alarm limit.
    pub health: HealthConfig,
    /// Read-back polling interval in nanoseconds (ICD-MDS §6: 10 s), must be > 0.
    pub poll_interval_ns: i64,
    /// Search page size (`_count`), must be > 0.
    pub page_size: u32,
    /// Most creates sent in one tick (bounds the tick after a long outage), must be > 0.
    pub max_sends_per_tick: u32,
}

/// Connector errors at start-up.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectorError {
    /// The outbox could not be loaded.
    #[error(transparent)]
    Outbox(#[from] OutboxError),
    /// The read-back state could not be loaded.
    #[error(transparent)]
    Inbox(#[from] InboxError),
    /// A configuration value is out of range.
    #[error("invalid connector configuration: {0}")]
    InvalidConfig(&'static str),
}

/// What one [`MdsConnector::tick`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct TickReport {
    /// Records the medical data system acknowledged, in send order.
    pub acknowledged: Vec<ClientId>,
    /// Records rejected with 4xx and held for review.
    pub held_for_review: Vec<ClientId>,
    /// Records scheduled for a retry, with the time of the next attempt.
    pub retry_scheduled: Vec<(ClientId, TaiNs)>,
    /// The read-back result, when a poll succeeded in this tick.
    pub poll: Option<PollResult>,
    /// The poll failure, when a poll was attempted and failed.
    pub poll_error: Option<TransportError>,
    /// A persistence failure; sending stopped for this tick (nothing is assumed written).
    pub store_error: Option<OutboxError>,
    /// A change of the technical alarm "medical data system unreachable".
    pub technical_alarm: Option<TechnicalAlarmSignal>,
    /// Health after the tick.
    pub health: MdsHealth,
}

impl TickReport {
    /// New and amended inbound observations (send through ingest and alarm evaluation, SR-016).
    pub fn inbound(&self) -> impl Iterator<Item = &InboundObservation> {
        self.poll
            .iter()
            .flat_map(|p| p.new.iter().chain(p.amended.iter()))
    }
}

/// The connector. Owns its store; the transport is lent per tick.
#[derive(Debug)]
pub struct MdsConnector<S> {
    store: S,
    leap: LeapSeconds,
    config: ConnectorConfig,
    outbox: Outbox,
    inbox: Inbox,
    health: HealthMonitor,
    next_poll_at: TaiNs,
}

impl<S: MdsStore> MdsConnector<S> {
    /// Loads the persisted outbox and read-back state (records in flight at a crash are retried).
    /// Health starts `Unreachable` with the outage counted from `now`.
    pub fn load(
        mut store: S,
        leap: LeapSeconds,
        config: ConnectorConfig,
        now: TaiNs,
    ) -> Result<Self, ConnectorError> {
        if config.poll_interval_ns <= 0 {
            return Err(ConnectorError::InvalidConfig(
                "poll_interval_ns must be > 0",
            ));
        }
        if config.page_size == 0 {
            return Err(ConnectorError::InvalidConfig("page_size must be > 0"));
        }
        if config.max_sends_per_tick == 0 {
            return Err(ConnectorError::InvalidConfig(
                "max_sends_per_tick must be > 0",
            ));
        }
        let outbox = Outbox::load(&mut store, config.outbox)?;
        let inbox = Inbox::load(&mut store, &leap, config.page_size)?;
        let mut health = HealthMonitor::new(config.health, now);
        health.set_held_for_review(outbox.held().next().is_some());
        Ok(Self {
            store,
            leap,
            config,
            outbox,
            inbox,
            health,
            next_poll_at: now,
        })
    }

    /// The routes of an item given the current health (F-GND-13, SR-015); see [`route`].
    pub fn route(&self, kind: ItemKind, policy: FallbackPolicy, now: TaiNs) -> Routes {
        route(kind, policy, self.health.status(now))
    }

    /// Adds a record to the outbox (persisted first; re-enqueueing the same client id is a no-op).
    pub fn enqueue(&mut self, item: OutboxItem, now: TaiNs) -> Result<EnqueueOutcome, OutboxError> {
        self.outbox.enqueue(&mut self.store, item, now)
    }

    /// Returns a record held for review to `Pending`.
    pub fn release_held(&mut self, id: ClientId, now: TaiNs) -> Result<(), OutboxError> {
        self.outbox.release_held(&mut self.store, id, now)?;
        self.health
            .set_held_for_review(self.outbox.held().next().is_some());
        Ok(())
    }

    /// Runs one synchronisation step at `now` (see the module documentation).
    pub fn tick(&mut self, now: TaiNs, transport: &mut dyn FhirTransport) -> TickReport {
        let mut report = TickReport {
            acknowledged: Vec::new(),
            held_for_review: Vec::new(),
            retry_scheduled: Vec::new(),
            poll: None,
            poll_error: None,
            store_error: None,
            technical_alarm: None,
            health: self.health.status(now),
        };
        let link_failed = self.send_due(now, transport, &mut report);
        if !link_failed && report.store_error.is_none() && now >= self.next_poll_at {
            self.poll(now, transport, &mut report);
        }
        self.health
            .set_held_for_review(self.outbox.held().next().is_some());
        report.technical_alarm = self.health.evaluate(now);
        report.health = self.health.status(now);
        report
    }

    /// Sends due records; returns whether the link failed.
    fn send_due(
        &mut self,
        now: TaiNs,
        transport: &mut dyn FhirTransport,
        report: &mut TickReport,
    ) -> bool {
        if let Err(e) = self.outbox.recover_in_flight(&mut self.store) {
            report.store_error = Some(e);
            return false;
        }
        let max = usize::try_from(self.config.max_sends_per_tick).unwrap_or(usize::MAX);
        for id in self.outbox.due(now).into_iter().take(max) {
            let (resource_type, resource) = match self.outbox.mark_in_flight(&mut self.store, id) {
                Ok(r) => (r.kind.resource_type(), r.resource.clone()),
                Err(e) => {
                    report.store_error = Some(e);
                    return false;
                }
            };
            let result = transport.create_conditional(resource_type, id, &resource);
            self.health.record(result.as_ref().map(|_| ()), now);
            match self.outbox.apply(&mut self.store, id, &result, now) {
                Ok(Disposition::Acknowledged) => report.acknowledged.push(id),
                Ok(Disposition::HeldForReview) => report.held_for_review.push(id),
                Ok(Disposition::RetryAt(t)) => report.retry_scheduled.push((id, t)),
                Err(e) => {
                    // The record stays in flight in memory; the next tick recovers it.
                    report.store_error = Some(e);
                    return false;
                }
            }
            if let Err(e) = &result {
                if !e.proves_contact() {
                    return true;
                }
            }
        }
        false
    }

    fn poll(&mut self, now: TaiNs, transport: &mut dyn FhirTransport, report: &mut TickReport) {
        let before = self.inbox.high_water().map(str::to_owned);
        match transport.search(&self.inbox.query()) {
            Ok(resources) => {
                self.health.record(Ok(()), now);
                let full_page = resources.len() >= self.config.page_size as usize;
                let result = self.inbox.accept(&mut self.store, &resources, &self.leap);
                let advanced = self.inbox.high_water() != before.as_deref();
                // A full page that moved the high-water mark: more may be waiting.
                self.next_poll_at = if full_page && advanced {
                    now
                } else {
                    TaiNs(now.0.saturating_add(self.config.poll_interval_ns))
                };
                report.poll = Some(result);
            }
            Err(e) => {
                self.health.record(Err(&e), now);
                self.next_poll_at = TaiNs(now.0.saturating_add(self.config.poll_interval_ns));
                report.poll_error = Some(e);
            }
        }
    }

    /// Current health.
    pub fn health(&self, now: TaiNs) -> MdsHealth {
        self.health.status(now)
    }

    /// Whether the technical alarm "medical data system unreachable" is raised.
    pub fn technical_alarm_active(&self) -> bool {
        self.health.technical_alarm_active()
    }

    /// The outbox (read-only).
    pub fn outbox(&self) -> &Outbox {
        &self.outbox
    }

    /// The read-back state (read-only).
    pub fn inbox(&self) -> &Inbox {
        &self.inbox
    }

    /// The store (read-only).
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Consumes the connector and returns its store (shutdown, or a restart in tests).
    pub fn into_store(self) -> S {
        self.store
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use caelum_domain::parameters::Parameter;
    use serde_json::json;

    use super::*;
    use crate::fake::{FakeFhirServer, FakeMode};
    use crate::fhir::{
        alarm_report, observation, AlarmEventRecord, Effective, ObservationRecord, ObservedValue,
        LOINC_SYSTEM, UCUM_SYSTEM,
    };
    use crate::health::TechnicalAlarmSignal;
    use crate::ids::ResourceKind;
    use crate::instant::tai_from_unix_s;
    use crate::outbox::{Backoff, Priority, RecordState};
    use crate::routing::Route;
    use crate::store::MemoryStore;

    const S: i64 = 1_000_000_000;

    fn leap() -> LeapSeconds {
        LeapSeconds::bundled().expect("table")
    }

    fn t0(l: &LeapSeconds) -> TaiNs {
        tai_from_unix_s(1_791_633_600, l).expect("2026-10-10T12:00:00Z")
    }

    fn at(l: &LeapSeconds, s: i64) -> TaiNs {
        TaiNs(t0(l).0 + s * S)
    }

    fn config() -> ConnectorConfig {
        ConnectorConfig {
            outbox: OutboxConfig {
                alarm: Backoff {
                    base_ns: S,
                    max_ns: 8 * S,
                },
                routine: Backoff {
                    base_ns: 5 * S,
                    max_ns: 60 * S,
                },
            },
            health: HealthConfig {
                outage_alarm_after_ns: 30 * S,
            },
            poll_interval_ns: 10 * S,
            page_size: 100,
            max_sends_per_tick: 100,
        }
    }

    const CREW: &str = "67e55044-10b1-426f-9247-bb680e5fe0c8";
    const ALARM: &str = "67e55044-10b1-426f-9247-bb680e5fe0a0";

    fn alarm_item(l: &LeapSeconds, local: &str, state: &str, t: TaiNs) -> OutboxItem {
        let client_id = ClientId::derive(ResourceKind::DiagnosticReport, local);
        let rec = AlarmEventRecord {
            alarm: ALARM.parse().expect("uuid"),
            crew: CREW.parse().expect("uuid"),
            raised_at: t,
            issued_at: t,
            rule_id: "SPO2_LOW".into(),
            ruleset_version: "1.0.0".into(),
            priority: "HIGH".into(),
            state: state.into(),
            conclusion: "SpO2 below limit".into(),
            results: Vec::new(),
        };
        OutboxItem {
            client_id,
            kind: ResourceKind::DiagnosticReport,
            priority: Priority::Alarm,
            resource: alarm_report(&rec, client_id, l).expect("report"),
        }
    }

    fn obs_item(
        l: &LeapSeconds,
        local: &str,
        priority: Priority,
        effective: Effective,
    ) -> OutboxItem {
        let client_id = ClientId::derive(ResourceKind::Observation, local);
        let rec = ObservationRecord {
            parameter: Parameter::Spo2,
            value: ObservedValue::Quantity(84.0),
            effective,
            crew: CREW.parse().expect("uuid"),
            device: None,
        };
        OutboxItem {
            client_id,
            kind: ResourceKind::Observation,
            priority,
            resource: observation(&rec, client_id, l).expect("observation"),
        }
    }

    fn external_hr(value: f64) -> serde_json::Value {
        json!({ "resourceType": "Observation", "status": "final",
            "code": { "coding": [{ "system": LOINC_SYSTEM, "code": "8867-4" }] },
            "valueQuantity": { "value": value, "system": UCUM_SYSTEM, "code": "/min" },
            "effectiveDateTime": "2026-10-10T12:00:30Z",
            "device": { "reference": "Device/mds-ecg-1" } })
    }

    /// The core's dispatcher, reduced to what the test needs: path B into a Vec standing in for
    /// the ground publisher, path A / held into the connector's outbox, A' into a CFDP Vec.
    struct Harness {
        conn: MdsConnector<MemoryStore>,
        server: FakeFhirServer,
        policy: FallbackPolicy,
        path_b: Vec<ClientId>,
        cfdp: Vec<ClientId>,
        reports: Vec<TickReport>,
    }

    impl Harness {
        fn new(l: &LeapSeconds, policy: FallbackPolicy) -> Self {
            let conn = MdsConnector::load(MemoryStore::default(), l.clone(), config(), t0(l))
                .expect("load");
            Self {
                conn,
                server: FakeFhirServer::new(l.clone(), t0(l)),
                policy,
                path_b: Vec::new(),
                cfdp: Vec::new(),
                reports: Vec::new(),
            }
        }

        fn submit(&mut self, kind: ItemKind, item: OutboxItem, now: TaiNs) -> Routes {
            let routes = self.conn.route(kind, self.policy, now);
            for r in routes.iter() {
                match r {
                    Route::PathB => self.path_b.push(item.client_id),
                    Route::PathAPrime => self.cfdp.push(item.client_id),
                    Route::PathA | Route::HoldLocal | Route::NotSent => {}
                }
            }
            if routes.uses_outbox() {
                self.conn.enqueue(item, now).expect("enqueue");
            }
            routes
        }

        fn tick(&mut self, now: TaiNs) -> TickReport {
            self.server.set_now(now);
            let r = self.conn.tick(now, &mut self.server);
            self.reports.push(r.clone());
            r
        }

        /// Drops the connector and rebuilds it from its store (process restart).
        fn restart(self, l: &LeapSeconds, now: TaiNs) -> Self {
            let store = self.conn.into_store();
            Self {
                conn: MdsConnector::load(store, l.clone(), config(), now).expect("reload"),
                server: self.server,
                policy: self.policy,
                path_b: self.path_b,
                cfdp: self.cfdp,
                reports: self.reports,
            }
        }

        fn acknowledged_counts(&self) -> BTreeMap<ClientId, usize> {
            let mut m = BTreeMap::new();
            for id in self.reports.iter().flat_map(|r| r.acknowledged.iter()) {
                *m.entry(*id).or_insert(0) += 1;
            }
            m
        }
    }

    /// M10 gate, SR-015, R-INT-06, R-INT-07, SR-016: unplug the medical data system
    /// mid-emergency. Path B gets every alarm item whatever the MDS state, the outbox keeps every
    /// record, health goes Unreachable and raises the technical alarm after the limit, and on
    /// reconnect every record syncs exactly once (including one whose response was lost) and
    /// read-back de-duplicates.
    #[test]
    fn sr_015_r_int_06_r_int_07_mds_unplugged_mid_emergency() {
        let l = leap();
        let mut h = Harness::new(&l, FallbackPolicy::HoldUntilReconnect);
        assert_eq!(
            h.conn.health(t0(&l)),
            MdsHealth::Unreachable,
            "never Connected before contact"
        );
        assert_eq!(h.tick(t0(&l)).health, MdsHealth::Connected);

        // Emergency starts while the MDS is healthy.
        let alarm1 = alarm_item(&l, "alarm-1/raised", "ACTIVE", at(&l, 1));
        let obs1 = obs_item(&l, "spo2-1", Priority::Alarm, Effective::At(at(&l, 1)));
        let routes = h.submit(ItemKind::AlarmEvent, alarm1.clone(), at(&l, 1));
        assert!(routes.contains(Route::PathB) && routes.contains(Route::PathA));
        let routes = h.submit(ItemKind::ObservationSummary, obs1.clone(), at(&l, 1));
        assert!(!routes.contains(Route::PathB), "no physiology on path B");
        let r = h.tick(at(&l, 1));
        let mut acked = r.acknowledged.clone();
        acked.sort();
        let mut want = vec![alarm1.client_id, obs1.client_id];
        want.sort();
        assert_eq!(acked, want);

        // Ground escalation: the create is applied but the response is lost.
        h.server.lose_next_responses(1);
        let esc1 = alarm_item(&l, "alarm-1/ground-escalation", "ESCALATED", at(&l, 2));
        h.submit(ItemKind::AlarmGroundEscalation, esc1.clone(), at(&l, 2));
        let r = h.tick(at(&l, 2));
        assert_eq!(r.retry_scheduled, [(esc1.client_id, at(&l, 3))]);
        assert_eq!(
            h.server.count_client_id(esc1.client_id),
            1,
            "applied on the server"
        );
        assert_eq!(r.health, MdsHealth::Unreachable);

        // Unplugged mid-emergency.
        h.server.set_mode(FakeMode::Unplugged);
        let alarm2 = alarm_item(&l, "alarm-2/raised", "ACTIVE", at(&l, 3));
        let obs2 = obs_item(&l, "spo2-2", Priority::Alarm, Effective::At(at(&l, 3)));
        let esc2 = alarm_item(&l, "alarm-2/ground-escalation", "ESCALATED", at(&l, 4));
        let summary = obs_item(
            &l,
            "summary-1",
            Priority::Routine,
            Effective::Period {
                start: t0(&l),
                end: at(&l, 5),
            },
        );
        let routes = h.submit(ItemKind::AlarmEvent, alarm2.clone(), at(&l, 3));
        assert!(routes.contains(Route::PathB) && routes.contains(Route::HoldLocal));
        h.submit(ItemKind::ObservationSummary, obs2.clone(), at(&l, 3));
        h.submit(ItemKind::AlarmGroundEscalation, esc2.clone(), at(&l, 4));
        h.submit(ItemKind::ObservationSummary, summary.clone(), at(&l, 5));
        // Re-raising the same record does not duplicate it.
        assert_eq!(
            h.conn.enqueue(alarm2.clone(), at(&l, 6)),
            Ok(EnqueueOutcome::AlreadyQueued)
        );
        // The MDS itself records a reading during the outage.
        let ext_id = h.server.insert_external(external_hr(58.0));

        // Path B carried every alarm item, healthy or not (SR-015, R-INT-06).
        assert_eq!(
            h.path_b,
            [
                alarm1.client_id,
                esc1.client_id,
                alarm2.client_id,
                esc2.client_id
            ]
        );
        assert!(h.cfdp.is_empty(), "hold-until-reconnect policy");

        let mut raised = Vec::new();
        for s in (10..=65).step_by(5) {
            let r = h.tick(at(&l, s));
            assert!(r.acknowledged.is_empty());
            assert_eq!(r.health, MdsHealth::Unreachable);
            if let Some(sig) = r.technical_alarm {
                raised.push((s, sig));
            }
        }
        // Outage since t+2 (lost response); limit 30 s: raised at the first tick after t+32.
        assert_eq!(
            raised,
            [(
                35,
                TechnicalAlarmSignal::Raise {
                    outage_since: at(&l, 2)
                }
            )]
        );
        assert!(h.conn.technical_alarm_active());
        // Nothing lost: every record is in the outbox, five unsent.
        assert_eq!(h.conn.outbox().records().count(), 7);
        assert_eq!(h.conn.outbox().unsent_count(), 5);

        // Reconnect.
        h.server.set_mode(FakeMode::Healthy);
        let mut cleared = 0;
        let mut inbound = Vec::new();
        for s in (70..=200).step_by(5) {
            let r = h.tick(at(&l, s));
            if r.technical_alarm == Some(TechnicalAlarmSignal::Clear) {
                cleared += 1;
            }
            inbound.extend(r.inbound().cloned());
        }
        assert_eq!(cleared, 1);
        assert!(!h.conn.technical_alarm_active());
        assert_eq!(h.conn.health(at(&l, 200)), MdsHealth::Connected);
        assert_eq!(h.conn.outbox().unsent_count(), 0);

        // Every record acknowledged exactly once and stored exactly once on the server.
        let all = [&alarm1, &obs1, &esc1, &alarm2, &obs2, &esc2, &summary];
        let acks = h.acknowledged_counts();
        for item in all {
            assert_eq!(acks.get(&item.client_id), Some(&1), "{}", item.client_id);
            assert_eq!(
                h.server.count_client_id(item.client_id),
                1,
                "{}",
                item.client_id
            );
            assert!(matches!(
                h.conn.outbox().get(item.client_id).expect("kept").state,
                RecordState::Acknowledged { .. }
            ));
        }
        assert_eq!(h.server.count("DiagnosticReport"), 4);
        assert_eq!(
            h.server.count("Observation"),
            4,
            "3 CAELUM + 1 MDS-authored"
        );

        // The response lost before the outage was acknowledged with the existing server id.
        let RecordState::Acknowledged { server_id, .. } =
            &h.conn.outbox().get(esc1.client_id).expect("kept").state
        else {
            unreachable!("acknowledged above")
        };
        let stored = h
            .server
            .search(&crate::transport::SearchQuery {
                resource_type: "DiagnosticReport".into(),
                last_updated_ge: None,
                count: 100,
            })
            .expect("search");
        assert_eq!(
            stored
                .iter()
                .filter(|r| r["id"] == server_id.as_str())
                .count(),
            1
        );

        // Read-back: the MDS-authored reading arrives once (SR-016); CAELUM's own are ignored.
        assert_eq!(inbound.len(), 1);
        assert_eq!(inbound[0].source_id.0, format!("Observation/{ext_id}"));
        assert_eq!(inbound[0].parameter, Parameter::HeartRate);
        let polls: Vec<&PollResult> = h.reports.iter().filter_map(|r| r.poll.as_ref()).collect();
        assert!(polls.iter().any(|p| p.own_ignored > 0));

        // A later MDS reading is delivered once; re-reading it at the boundary is a duplicate.
        h.server.set_now(at(&l, 205));
        let ext2 = h.server.insert_external(external_hr(59.0));
        let first: Vec<_> = h.tick(at(&l, 215)).inbound().cloned().collect();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].source_id.0, format!("Observation/{ext2}"));
        let again = h.tick(at(&l, 230));
        assert_eq!(again.inbound().count(), 0);
        assert_eq!(again.poll.as_ref().map(|p| p.duplicates), Some(1));
    }

    /// R-INT-07, F-GND-12: a restart mid-outage rebuilds the outbox from the store; after
    /// reconnect everything syncs once (including a create applied before the restart).
    #[test]
    fn r_int_07_restart_mid_outage_then_sync() {
        let l = leap();
        let mut h = Harness::new(&l, FallbackPolicy::DirectCfdp);
        h.tick(t0(&l));
        h.server.lose_next_responses(1);
        let a = alarm_item(&l, "alarm-9/raised", "ACTIVE", at(&l, 1));
        h.submit(ItemKind::AlarmEvent, a.clone(), at(&l, 1));
        h.tick(at(&l, 1));
        h.server.set_mode(FakeMode::Unplugged);
        let s = obs_item(&l, "summary-9", Priority::Routine, Effective::At(at(&l, 2)));
        let routes = h.submit(ItemKind::ObservationSummary, s.clone(), at(&l, 2));
        // A' policy while unreachable: by CFDP now and held for the onboard record.
        assert!(routes.contains(Route::PathAPrime) && routes.contains(Route::HoldLocal));
        assert_eq!(h.cfdp, [s.client_id]);
        h.tick(at(&l, 5));

        // Restart.
        let mut h = h.restart(&l, at(&l, 6));
        assert_eq!(h.conn.outbox().records().count(), 2);
        assert_eq!(h.conn.outbox().unsent_count(), 2);
        assert_eq!(h.conn.health(at(&l, 6)), MdsHealth::Unreachable);

        h.server.set_mode(FakeMode::Healthy);
        for s in (10..=120).step_by(5) {
            h.tick(at(&l, s));
        }
        assert_eq!(h.conn.outbox().unsent_count(), 0);
        for id in [a.client_id, s.client_id] {
            assert_eq!(h.server.count_client_id(id), 1);
        }
        assert_eq!(h.conn.health(at(&l, 120)), MdsHealth::Connected);
    }

    /// ICD-MDS §9: a 4xx is held for review (Degraded, not retried) and can be released.
    #[test]
    fn icd_mds_9_rejection_held_and_released() {
        let l = leap();
        let mut h = Harness::new(&l, FallbackPolicy::HoldUntilReconnect);
        h.server
            .set_mode(FakeMode::ClientError(422, "profile violation".into()));
        let o = obs_item(&l, "o", Priority::Routine, Effective::At(t0(&l)));
        h.conn.enqueue(o.clone(), t0(&l)).expect("enqueue");
        let r = h.tick(t0(&l));
        assert_eq!(r.held_for_review, [o.client_id]);
        assert_eq!(r.health, MdsHealth::Degraded);
        h.server.set_mode(FakeMode::Healthy);
        let r = h.tick(at(&l, 100));
        assert!(r.acknowledged.is_empty(), "held records are not retried");
        assert_eq!(r.health, MdsHealth::Degraded);
        h.conn
            .release_held(o.client_id, at(&l, 101))
            .expect("release");
        let r = h.tick(at(&l, 101));
        assert_eq!(r.acknowledged, [o.client_id]);
        assert_eq!(r.health, MdsHealth::Connected);
    }

    /// A 5xx stops the tick's sending and does not poll; alarm records go before summaries.
    #[test]
    fn f_gnd_12_server_errors_and_priority_order() {
        let l = leap();
        let mut h = Harness::new(&l, FallbackPolicy::HoldUntilReconnect);
        let s = obs_item(&l, "s", Priority::Routine, Effective::At(t0(&l)));
        let a = alarm_item(&l, "a", "ACTIVE", t0(&l));
        h.conn.enqueue(s.clone(), t0(&l)).expect("enqueue");
        h.conn.enqueue(a.clone(), at(&l, 1)).expect("enqueue");
        h.server.set_mode(FakeMode::ServerError(503));
        let r = h.tick(at(&l, 1));
        assert_eq!(
            r.retry_scheduled,
            [(a.client_id, at(&l, 2))],
            "stops after the first failure"
        );
        assert_eq!((r.poll.is_none(), r.poll_error.is_none()), (true, true));
        assert_eq!(r.health, MdsHealth::Degraded);
        h.server.set_mode(FakeMode::Healthy);
        let r = h.tick(at(&l, 2));
        assert_eq!(r.acknowledged, [a.client_id, s.client_id]);
        assert!(r.poll.is_some());
    }

    /// Store failures stop the tick and are reported; the record is recovered on the next tick.
    #[test]
    fn r_int_07_store_failure_is_reported_and_recovered() {
        let l = leap();
        let mut store = MemoryStore::default();
        let o = obs_item(&l, "o", Priority::Routine, Effective::At(t0(&l)));
        {
            let mut ob = Outbox::load(&mut store, config().outbox).expect("load");
            ob.enqueue(&mut store, o.clone(), t0(&l)).expect("enqueue");
        }
        store.fail_writes = true;
        let mut conn = MdsConnector::load(store, l.clone(), config(), t0(&l)).expect("load");
        let mut server = FakeFhirServer::new(l.clone(), t0(&l));
        let r = conn.tick(t0(&l), &mut server);
        assert!(r.store_error.is_some());
        assert_eq!(
            server.create_calls, 0,
            "nothing sent without a durable in-flight mark"
        );
        let mut store = conn.into_store();
        store.fail_writes = false;
        let mut conn = MdsConnector::load(store, l.clone(), config(), t0(&l)).expect("load");
        assert_eq!(conn.tick(t0(&l), &mut server).acknowledged, [o.client_id]);
        assert_eq!(conn.outbox().unsent_count(), 0);
        assert!(!conn.store().fail_writes);
    }

    #[test]
    fn invalid_configuration_is_rejected() {
        let l = leap();
        for (f, msg) in [
            (
                (|c: &mut ConnectorConfig| c.poll_interval_ns = 0) as fn(&mut ConnectorConfig),
                "poll_interval_ns must be > 0",
            ),
            (
                |c: &mut ConnectorConfig| c.page_size = 0,
                "page_size must be > 0",
            ),
            (
                |c: &mut ConnectorConfig| c.max_sends_per_tick = 0,
                "max_sends_per_tick must be > 0",
            ),
        ] {
            let mut c = config();
            f(&mut c);
            assert_eq!(
                MdsConnector::load(MemoryStore::default(), l.clone(), c, t0(&l)).err(),
                Some(ConnectorError::InvalidConfig(msg))
            );
        }
    }

    /// A full page that advances the high-water mark is followed by an immediate poll.
    #[test]
    fn full_pages_are_polled_again() {
        let l = leap();
        let mut c = config();
        c.page_size = 2;
        let mut conn =
            MdsConnector::load(MemoryStore::default(), l.clone(), c, t0(&l)).expect("load");
        let mut server = FakeFhirServer::new(l.clone(), t0(&l));
        for (s, v) in [(0, 60.0), (1, 61.0), (2, 62.0)] {
            server.set_now(at(&l, s));
            server.insert_external(external_hr(v));
        }
        let r = conn.tick(at(&l, 2), &mut server);
        assert_eq!(r.inbound().count(), 2);
        let r = conn.tick(at(&l, 2), &mut server);
        assert_eq!(r.inbound().count(), 1, "second page without waiting");
        assert_eq!(
            r.poll.as_ref().map(|p| p.duplicates),
            Some(1),
            "boundary re-read"
        );
        let r = conn.tick(at(&l, 2), &mut server);
        assert!(r.poll.is_some(), "the short page ends the burst");
        assert_eq!(r.inbound().count(), 0);
        let r = conn.tick(at(&l, 3), &mut server);
        assert!(r.poll.is_none(), "next poll waits for the interval");
    }
}
