//! Fail-over timing (F-SYS-04, R-OPS-02, `docs/03_ARCHITECTURE.md` §6, CR-017 §3).
//!
//! The standby promotes itself once it has heard nothing valid from the primary for
//! `heartbeat_interval × missed_heartbeats`. Detection is measured from the last valid message,
//! which is never earlier than the failure, so the worst case from failure to promotion is the
//! detection window plus one driver tick. Device reconnection then has its own budget. The sum
//! must stay within [`TAKEOVER_LIMIT`]; [`Timing::validate`] refuses any configuration that
//! would not.

use std::time::Duration;

/// R-OPS-02: the standby takes over within 10 s of primary failure.
pub const TAKEOVER_LIMIT: Duration = Duration::from_secs(10);

/// Fail-over timing parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// The primary sends a heartbeat this often. Default 1 s.
    pub heartbeat_interval: Duration,
    /// Heartbeats missed before the standby promotes itself. Default 5.
    pub missed_heartbeats: u32,
    /// The driver evaluates the state machine this often. Default 100 ms.
    pub tick_interval: Duration,
    /// Time allowed for devices to reconnect to the new primary. Default 4 s.
    pub device_reconnect_budget: Duration,
    /// Delay between connection attempts to the peer. Default 500 ms.
    pub connect_retry: Duration,
}

impl Default for Timing {
    /// The defaults of `docs/03_ARCHITECTURE.md` §6: 1 s heartbeat, promotion after 5 missed
    /// heartbeats, 100 ms tick, 4 s device reconnection: worst case 9.1 s ≤ 10 s.
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(1),
            missed_heartbeats: 5,
            tick_interval: Duration::from_millis(100),
            device_reconnect_budget: Duration::from_secs(4),
            connect_retry: Duration::from_millis(500),
        }
    }
}

/// A timing configuration that breaks R-OPS-02 or cannot work.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimingError {
    /// A zero interval.
    #[error("heartbeat and tick intervals must be positive")]
    ZeroInterval,
    /// Fewer than 3 missed heartbeats would promote on a single delayed packet.
    #[error("missed_heartbeats must be at least 3")]
    TooFewMissedHeartbeats,
    /// The tick is slower than the heartbeat.
    #[error("tick_interval must not exceed heartbeat_interval")]
    TickTooSlow,
    /// The worst-case takeover exceeds 10 s.
    #[error("worst-case takeover {worst_ms} ms exceeds the 10 s limit (R-OPS-02)")]
    TakeoverTooSlow {
        /// Worst case in milliseconds.
        worst_ms: u128,
    },
}

impl Timing {
    /// Silence after which the standby promotes itself.
    pub fn detection_window(&self) -> Duration {
        self.heartbeat_interval
            .saturating_mul(self.missed_heartbeats)
    }

    /// Worst case from primary failure to devices reconnected on the new primary.
    pub fn worst_case_takeover(&self) -> Duration {
        self.detection_window()
            .saturating_add(self.tick_interval)
            .saturating_add(self.device_reconnect_budget)
    }

    /// Checks the configuration against R-OPS-02.
    pub fn validate(&self) -> Result<(), TimingError> {
        if self.heartbeat_interval.is_zero() || self.tick_interval.is_zero() {
            return Err(TimingError::ZeroInterval);
        }
        if self.missed_heartbeats < 3 {
            return Err(TimingError::TooFewMissedHeartbeats);
        }
        if self.tick_interval > self.heartbeat_interval {
            return Err(TimingError::TickTooSlow);
        }
        let worst = self.worst_case_takeover();
        if worst > TAKEOVER_LIMIT {
            return Err(TimingError::TakeoverTooSlow {
                worst_ms: worst.as_millis(),
            });
        }
        Ok(())
    }

    pub(crate) fn heartbeat_ms(&self) -> u64 {
        duration_ms(self.heartbeat_interval)
    }

    pub(crate) fn detection_ms(&self) -> u64 {
        duration_ms(self.detection_window())
    }

    pub(crate) fn reconnect_ms(&self) -> u64 {
        duration_ms(self.device_reconnect_budget)
    }
}

pub(crate) fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r_ops_02_default_timing_meets_the_10_s_takeover() {
        let t = Timing::default();
        assert_eq!(t.validate(), Ok(()));
        assert_eq!(t.detection_window(), Duration::from_secs(5));
        assert_eq!(t.worst_case_takeover(), Duration::from_millis(9100));
        assert!(t.worst_case_takeover() <= TAKEOVER_LIMIT);
    }

    #[test]
    fn r_ops_02_timing_beyond_10_s_is_refused() {
        let t = Timing {
            missed_heartbeats: 7,
            ..Timing::default()
        };
        assert_eq!(
            t.validate(),
            Err(TimingError::TakeoverTooSlow { worst_ms: 11_100 })
        );
        let t = Timing {
            missed_heartbeats: 2,
            ..Timing::default()
        };
        assert_eq!(t.validate(), Err(TimingError::TooFewMissedHeartbeats));
        let t = Timing {
            heartbeat_interval: Duration::ZERO,
            ..Timing::default()
        };
        assert_eq!(t.validate(), Err(TimingError::ZeroInterval));
        let t = Timing {
            tick_interval: Duration::from_secs(2),
            ..Timing::default()
        };
        assert_eq!(t.validate(), Err(TimingError::TickTooSlow));
    }
}
