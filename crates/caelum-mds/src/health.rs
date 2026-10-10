//! Connection health of the medical data system (F-GND-12, ICD-MDS §9, docs/04 §3a "Health").
//!
//! The state shown in the status footer is derived only from exchanges that actually happened:
//! the connector is never [`MdsHealth::Connected`] before a successful exchange, and an outage
//! (no exchange that proves contact) longer than the configured limit raises a CAELUM technical
//! alarm, which is cleared by the next exchange that proves contact. There is no built-in limit:
//! it comes from the signed mission configuration.
//!
//! The technical alarm is a signal for the alarm engine; patient alarms never depend on this
//! module or on the medical data system (SR-015, R-INT-06).

use caelum_domain::time::TaiNs;
use serde::{Deserialize, Serialize};

use crate::transport::TransportError;

/// Connection state of the medical data system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MdsHealth {
    /// No medical data system is configured for this mission (path A unavailable by design).
    NotConfigured,
    /// The last exchange succeeded and no record is held for review.
    Connected,
    /// The system answers, but rejected a request (4xx, records held for review) or is failing
    /// with 5xx / protocol errors for less than the outage limit.
    Degraded,
    /// No successful exchange yet, the link is down or timing out, or the system has been failing
    /// for longer than the outage limit.
    Unreachable,
}

impl MdsHealth {
    /// Whether path A can be used for new records (Connected or Degraded). Records go to the
    /// outbox either way; this only decides whether the per-mission fallback applies.
    pub fn accepts_path_a(self) -> bool {
        matches!(self, Self::Connected | Self::Degraded)
    }
}

/// Health monitoring configuration (signed mission configuration; no defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthConfig {
    /// An outage longer than this raises the technical alarm (ICD-MDS §9), in nanoseconds.
    pub outage_alarm_after_ns: i64,
}

/// A change of the technical alarm "medical data system unreachable" (ICD-MDS §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TechnicalAlarmSignal {
    /// Raise: no contact since `outage_since`, longer than the configured limit.
    Raise {
        /// Start of the outage.
        outage_since: TaiNs,
    },
    /// Clear: contact restored.
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Last {
    Never,
    Success,
    Rejected,
    ServerFault,
    LinkDown,
}

/// Tracks exchanges and derives [`MdsHealth`] and the technical alarm.
#[derive(Debug, Clone)]
pub struct HealthMonitor {
    config: Option<HealthConfig>,
    last: Last,
    last_contact_at: Option<TaiNs>,
    outage_since: Option<TaiNs>,
    held_for_review: bool,
    alarm_active: bool,
}

impl HealthMonitor {
    /// A configured connection that has not exchanged anything yet: `Unreachable`, with the outage
    /// counted from `now` (start-up), so a system that never answers also raises the alarm.
    pub fn new(config: HealthConfig, now: TaiNs) -> Self {
        Self {
            config: Some(config),
            last: Last::Never,
            last_contact_at: None,
            outage_since: Some(now),
            held_for_review: false,
            alarm_active: false,
        }
    }

    /// No medical data system configured: always `NotConfigured`, never an alarm.
    pub fn not_configured() -> Self {
        Self {
            config: None,
            last: Last::Never,
            last_contact_at: None,
            outage_since: None,
            held_for_review: false,
            alarm_active: false,
        }
    }

    /// Records the outcome of one exchange (create or search).
    pub fn record(&mut self, outcome: Result<(), &TransportError>, now: TaiNs) {
        if self.config.is_none() {
            return;
        }
        match outcome {
            Ok(()) => {
                self.last = Last::Success;
                self.last_contact_at = Some(now);
                self.outage_since = None;
            }
            Err(e) if e.proves_contact() => {
                self.last = Last::Rejected;
                self.last_contact_at = Some(now);
                self.outage_since = None;
            }
            Err(e) => {
                self.last = match e {
                    TransportError::Unreachable(_) | TransportError::Timeout => Last::LinkDown,
                    _ => Last::ServerFault,
                };
                self.outage_since.get_or_insert(now);
            }
        }
    }

    /// Whether records are held for review (makes an otherwise healthy link `Degraded`).
    pub fn set_held_for_review(&mut self, held: bool) {
        self.held_for_review = held;
    }

    fn outage_exceeded(&self, now: TaiNs) -> bool {
        match (self.config, self.outage_since) {
            (Some(c), Some(since)) => {
                now.0.saturating_sub(since.0) > c.outage_alarm_after_ns.max(0)
            }
            _ => false,
        }
    }

    /// The current state.
    pub fn status(&self, now: TaiNs) -> MdsHealth {
        if self.config.is_none() {
            return MdsHealth::NotConfigured;
        }
        match self.last {
            Last::Never | Last::LinkDown => MdsHealth::Unreachable,
            Last::ServerFault if self.outage_exceeded(now) => MdsHealth::Unreachable,
            Last::ServerFault | Last::Rejected => MdsHealth::Degraded,
            Last::Success if self.held_for_review => MdsHealth::Degraded,
            Last::Success => MdsHealth::Connected,
        }
    }

    /// Raises or clears the technical alarm on a change; `None` when nothing changed.
    pub fn evaluate(&mut self, now: TaiNs) -> Option<TechnicalAlarmSignal> {
        let exceeded = self.outage_exceeded(now);
        match (self.alarm_active, exceeded, self.outage_since) {
            (false, true, Some(outage_since)) => {
                self.alarm_active = true;
                Some(TechnicalAlarmSignal::Raise { outage_since })
            }
            (true, _, None) => {
                self.alarm_active = false;
                Some(TechnicalAlarmSignal::Clear)
            }
            _ => None,
        }
    }

    /// Whether the technical alarm is raised.
    pub fn technical_alarm_active(&self) -> bool {
        self.alarm_active
    }

    /// Start of the current outage, if any.
    pub fn outage_since(&self) -> Option<TaiNs> {
        self.outage_since
    }

    /// Time of the last exchange that proved contact, if any.
    pub fn last_contact_at(&self) -> Option<TaiNs> {
        self.last_contact_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;

    fn monitor() -> HealthMonitor {
        HealthMonitor::new(
            HealthConfig {
                outage_alarm_after_ns: 30 * S,
            },
            TaiNs(0),
        )
    }

    /// F-GND-12: never Connected before a successful exchange.
    #[test]
    fn f_gnd_12_never_connected_before_success() {
        let mut m = monitor();
        assert_eq!(m.status(TaiNs(0)), MdsHealth::Unreachable);
        assert_eq!(m.last_contact_at(), None);
        m.record(Err(&TransportError::Http5xx(503)), TaiNs(S));
        assert_eq!(m.status(TaiNs(S)), MdsHealth::Degraded);
        m.record(Ok(()), TaiNs(2 * S));
        assert_eq!(m.status(TaiNs(2 * S)), MdsHealth::Connected);
        assert_eq!(m.last_contact_at(), Some(TaiNs(2 * S)));
    }

    /// ICD-MDS §9: an outage longer than the limit raises the technical alarm once; contact clears it.
    #[test]
    fn icd_mds_9_technical_alarm_after_limit() {
        let mut m = monitor();
        m.record(Ok(()), TaiNs(0));
        m.record(Err(&TransportError::Unreachable("x".into())), TaiNs(10 * S));
        assert_eq!(m.status(TaiNs(10 * S)), MdsHealth::Unreachable);
        assert_eq!(
            m.evaluate(TaiNs(40 * S)),
            None,
            "exactly the limit is not over it"
        );
        m.record(Err(&TransportError::Timeout), TaiNs(39 * S));
        assert_eq!(
            m.outage_since(),
            Some(TaiNs(10 * S)),
            "outage start is kept"
        );
        assert_eq!(
            m.evaluate(TaiNs(41 * S)),
            Some(TechnicalAlarmSignal::Raise {
                outage_since: TaiNs(10 * S)
            })
        );
        assert!(m.technical_alarm_active());
        assert_eq!(m.evaluate(TaiNs(50 * S)), None, "raised once");
        // A 4xx proves contact: outage over, alarm cleared, state Degraded.
        m.record(
            Err(&TransportError::Http4xx {
                status: 422,
                outcome: String::new(),
            }),
            TaiNs(60 * S),
        );
        assert_eq!(m.status(TaiNs(60 * S)), MdsHealth::Degraded);
        assert_eq!(m.evaluate(TaiNs(60 * S)), Some(TechnicalAlarmSignal::Clear));
        assert_eq!(m.evaluate(TaiNs(61 * S)), None);
    }

    /// A system that never answers raises the alarm counted from start-up; a 5xx streak longer
    /// than the limit is Unreachable.
    #[test]
    fn never_answering_and_long_server_faults() {
        let mut m = monitor();
        assert!(matches!(
            m.evaluate(TaiNs(31 * S)),
            Some(TechnicalAlarmSignal::Raise {
                outage_since: TaiNs(0)
            })
        ));
        let mut m = monitor();
        m.record(Ok(()), TaiNs(0));
        m.record(Err(&TransportError::Protocol("x".into())), TaiNs(S));
        assert_eq!(m.status(TaiNs(31 * S)), MdsHealth::Degraded);
        assert_eq!(m.status(TaiNs(32 * S)), MdsHealth::Unreachable);
        m.record(Ok(()), TaiNs(33 * S));
        m.set_held_for_review(true);
        assert_eq!(m.status(TaiNs(33 * S)), MdsHealth::Degraded);
        m.set_held_for_review(false);
        assert_eq!(m.status(TaiNs(33 * S)), MdsHealth::Connected);
    }

    #[test]
    fn not_configured_never_alarms() {
        let mut m = HealthMonitor::not_configured();
        m.record(Ok(()), TaiNs(0));
        assert_eq!(m.status(TaiNs(i64::MAX)), MdsHealth::NotConfigured);
        assert_eq!(m.evaluate(TaiNs(i64::MAX)), None);
        assert!(!MdsHealth::NotConfigured.accepts_path_a());
        assert!(!MdsHealth::Unreachable.accepts_path_a());
        assert!(MdsHealth::Degraded.accepts_path_a());
    }
}
