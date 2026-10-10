//! Link supervision for one device: automatic reconnection and link metrics (F-ACQ-01, F-ACQ-10).
//!
//! Sans-I/O: the transport (BLE, serial) reports events and asks [`LinkSupervisor::poll`] what to
//! do next, so the policy is tested with a simulated clock. After a link loss, the first attempt
//! is immediate and retries back off to at most [`ReconnectPolicy::max_backoff_ns`] (2 s), so a
//! device that comes back is reconnected within 2 s plus one connection time (M2 gate: unplug
//! and replug recovers in ≤ 5 s).
//!
//! Metrics are only ever measured values: RSSI as last reported by the radio, reconnect count,
//! and packet loss only for devices with a nominal notification interval (otherwise none is
//! reported, never an estimate).

use std::collections::VecDeque;

use caelum_domain::ids::DeviceId;
use caelum_domain::observation::{MeasurementMode, Quality};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_domain::units::Unit;
use caelum_ingest::{ClockOffset, DeviceFlags, RawReading, RawValue};

const NS_PER_S: u64 = 1_000_000_000;

/// Reconnection timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    /// Wait after the first failed attempt; doubles per failure.
    pub initial_backoff_ns: u64,
    /// Longest wait between attempts.
    pub max_backoff_ns: u64,
    /// An attempt that has not completed after this long counts as failed.
    pub connect_timeout_ns: u64,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_backoff_ns: 250_000_000,
            max_backoff_ns: 2 * NS_PER_S,
            connect_timeout_ns: 2 * NS_PER_S,
        }
    }
}

/// Link state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// Not connected; next attempt at `retry_at`.
    Disconnected {
        /// Monotonic ns of the next attempt.
        retry_at: u64,
        /// Failed attempts since the link was lost.
        failures: u32,
    },
    /// An attempt is running.
    Connecting {
        /// When it started.
        started: u64,
        /// Failed attempts before this one.
        failures: u32,
    },
    /// Connected.
    Connected {
        /// Since when.
        since: u64,
    },
}

/// What the transport should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Start a connection attempt (then report [`LinkSupervisor::connected`] or
    /// [`LinkSupervisor::connect_failed`]).
    Connect,
    /// Nothing until this monotonic time (or an event).
    WaitUntil(u64),
    /// Connected: keep receiving.
    Receive,
}

/// Measured link metrics (F-ACQ-10). `None` means not measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkMetrics {
    /// Last RSSI reported by the radio, dBm.
    pub rssi_dbm: Option<f64>,
    /// Lost notifications in the window, percent (devices with a nominal interval only).
    pub packet_loss_pct: Option<f64>,
    /// Reconnections since supervision started.
    pub reconnect_count: u64,
}

/// Supervises one device link.
#[derive(Debug, Clone)]
pub struct LinkSupervisor {
    policy: ReconnectPolicy,
    state: LinkState,
    reconnects: u64,
    ever_connected: bool,
    rssi: Option<f64>,
    nominal_interval_ns: Option<u64>,
    window_ns: u64,
    arrivals: VecDeque<u64>,
    connected_since: Option<u64>,
}

impl LinkSupervisor {
    /// A supervisor that starts disconnected and connects at once. `nominal_interval_ns` is the
    /// device profile's notification interval, if it has a fixed one (enables packet loss).
    pub fn new(policy: ReconnectPolicy, now: u64, nominal_interval_ns: Option<u64>) -> Self {
        Self {
            policy,
            state: LinkState::Disconnected {
                retry_at: now,
                failures: 0,
            },
            reconnects: 0,
            ever_connected: false,
            rssi: None,
            nominal_interval_ns,
            window_ns: 60 * NS_PER_S,
            arrivals: VecDeque::new(),
            connected_since: None,
        }
    }

    /// Current state.
    pub fn state(&self) -> LinkState {
        self.state
    }

    /// What to do at `now`.
    pub fn poll(&mut self, now: u64) -> Action {
        match self.state {
            LinkState::Connected { .. } => Action::Receive,
            LinkState::Disconnected { retry_at, failures } => {
                if now >= retry_at {
                    self.state = LinkState::Connecting {
                        started: now,
                        failures,
                    };
                    Action::Connect
                } else {
                    Action::WaitUntil(retry_at)
                }
            }
            LinkState::Connecting { started, failures } => {
                let deadline = started.saturating_add(self.policy.connect_timeout_ns);
                if now >= deadline {
                    self.fail(now, failures);
                    self.poll(now)
                } else {
                    Action::WaitUntil(deadline)
                }
            }
        }
    }

    fn backoff(&self, failures: u32) -> u64 {
        let doublings = failures.saturating_sub(1).min(16);
        self.policy
            .initial_backoff_ns
            .saturating_mul(1 << doublings)
            .min(self.policy.max_backoff_ns)
    }

    fn fail(&mut self, now: u64, failures: u32) {
        let failures = failures.saturating_add(1);
        self.state = LinkState::Disconnected {
            retry_at: now.saturating_add(self.backoff(failures)),
            failures,
        };
    }

    /// The attempt succeeded.
    pub fn connected(&mut self, now: u64) {
        if self.ever_connected {
            self.reconnects += 1;
        }
        self.ever_connected = true;
        self.state = LinkState::Connected { since: now };
        self.connected_since = Some(now);
        self.arrivals.clear();
    }

    /// The attempt failed.
    pub fn connect_failed(&mut self, now: u64) {
        let failures = match self.state {
            LinkState::Connecting { failures, .. } | LinkState::Disconnected { failures, .. } => {
                failures
            }
            LinkState::Connected { .. } => 0,
        };
        self.fail(now, failures);
    }

    /// The link was lost: reconnect immediately.
    pub fn disconnected(&mut self, now: u64) {
        self.state = LinkState::Disconnected {
            retry_at: now,
            failures: 0,
        };
        self.connected_since = None;
        self.arrivals.clear();
    }

    /// A notification arrived.
    pub fn notification(&mut self, now: u64) {
        self.arrivals.push_back(now);
        let cutoff = now.saturating_sub(self.window_ns);
        while self.arrivals.front().is_some_and(|t| *t < cutoff) {
            self.arrivals.pop_front();
        }
    }

    /// The radio reported a signal strength.
    pub fn rssi(&mut self, dbm: f64) {
        if dbm.is_finite() {
            self.rssi = Some(dbm);
        }
    }

    /// Metrics at `now`. Packet loss needs a nominal interval and a full window of connection.
    pub fn metrics(&self, now: u64) -> LinkMetrics {
        LinkMetrics {
            rssi_dbm: self.rssi,
            packet_loss_pct: self.packet_loss(now),
            reconnect_count: self.reconnects,
        }
    }

    fn packet_loss(&self, now: u64) -> Option<f64> {
        let interval = self.nominal_interval_ns?;
        let since = self.connected_since?;
        if interval == 0 || now.saturating_sub(since) < self.window_ns {
            return None;
        }
        let cutoff = now.saturating_sub(self.window_ns);
        let received = self.arrivals.iter().filter(|t| **t >= cutoff).count() as f64;
        let expected = (self.window_ns / interval) as f64;
        Some(((expected - received) / expected * 100.0).clamp(0.0, 100.0))
    }
}

impl LinkMetrics {
    /// The measured metrics as technical readings for ingest (F-ACQ-10); metrics that were not
    /// measured produce no reading. The station measures these itself, so quality is good.
    pub fn readings(
        &self,
        device_id: DeviceId,
        at: TaiNs,
        clock: Option<&ClockOffset>,
    ) -> Vec<RawReading> {
        let values = [
            (Parameter::Rssi, self.rssi_dbm, Unit::DbMilliwatt),
            (
                Parameter::PacketLossPct,
                self.packet_loss_pct,
                Unit::Percent,
            ),
            (
                Parameter::ReconnectCount,
                Some(self.reconnect_count as f64),
                Unit::One,
            ),
            (
                Parameter::DeviceClockOffset,
                clock.map(|c| c.offset_ns as f64 / 1_000_000.0),
                Unit::Millisecond,
            ),
        ];
        values
            .into_iter()
            .filter_map(|(parameter, value, unit)| {
                value.map(|v| RawReading {
                    device_id,
                    parameter,
                    value: Some(RawValue::Number(v)),
                    unit: Some(unit),
                    device_time: None,
                    received_at: at,
                    quality: Quality::Good,
                    mode: MeasurementMode::Continuous,
                    flags: DeviceFlags::default(),
                    site: None,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = NS_PER_S;
    const MS: u64 = 1_000_000;

    /// M2 gate (F-ACQ-01): a device unplugged and plugged back in is reconnected within 5 s, no
    /// matter how long it was away.
    #[test]
    fn f_acq_01_replugged_device_recovers_within_five_seconds() {
        for away_s in [1, 3, 10, 60, 600] {
            let mut link = LinkSupervisor::new(ReconnectPolicy::default(), 0, None);
            assert_eq!(link.poll(0), Action::Connect);
            link.connected(100 * MS);
            let lost = 10 * S;
            link.disconnected(lost);
            let back = lost + away_s * S;
            let connect_time = 700 * MS; // a slow BLE connection
            let mut now = lost;
            let recovered = loop {
                match link.poll(now) {
                    Action::Connect if now >= back => {
                        link.connected(now + connect_time);
                        break now + connect_time;
                    }
                    Action::Connect => {
                        now += 100 * MS; // device absent: the attempt fails after a while
                        link.connect_failed(now);
                    }
                    Action::WaitUntil(t) => now = t,
                    Action::Receive => unreachable!(),
                }
            };
            assert!(
                recovered - back <= 5 * S,
                "away {away_s} s: {} ms",
                (recovered - back) / MS
            );
            assert_eq!(link.metrics(recovered).reconnect_count, 1);
        }
    }

    #[test]
    fn backoff_doubles_up_to_two_seconds() {
        let mut link = LinkSupervisor::new(ReconnectPolicy::default(), 0, None);
        let mut waits = Vec::new();
        let mut now = 0;
        for _ in 0..6 {
            assert_eq!(link.poll(now), Action::Connect);
            link.connect_failed(now);
            if let Action::WaitUntil(t) = link.poll(now) {
                waits.push((t - now) / MS);
                now = t;
            }
        }
        assert_eq!(waits, [250, 500, 1_000, 2_000, 2_000, 2_000]);
    }

    #[test]
    fn a_hanging_attempt_times_out() {
        let mut link = LinkSupervisor::new(ReconnectPolicy::default(), 0, None);
        assert_eq!(link.poll(0), Action::Connect);
        assert_eq!(link.poll(S), Action::WaitUntil(2 * S));
        assert_eq!(link.poll(2 * S), Action::WaitUntil(2 * S + 250 * MS));
    }

    /// F-ACQ-10: packet loss is measured only with a nominal interval and a full window.
    #[test]
    fn f_acq_10_packet_loss_needs_a_nominal_interval_and_a_full_window() {
        let mut link = LinkSupervisor::new(ReconnectPolicy::default(), 0, Some(S));
        link.poll(0);
        link.connected(0);
        for i in 0..60 {
            if i % 4 != 0 {
                link.notification(i * S);
            }
        }
        assert_eq!(
            link.metrics(30 * S).packet_loss_pct,
            None,
            "window not full yet"
        );
        let loss = link.metrics(60 * S).packet_loss_pct.unwrap();
        assert!((loss - 25.0).abs() < 2.0, "{loss}");
        let unknown = LinkSupervisor::new(ReconnectPolicy::default(), 0, None);
        assert_eq!(unknown.metrics(100 * S).packet_loss_pct, None);
    }

    /// F-ACQ-10/11: measured metrics become technical readings; unmeasured ones produce none.
    #[test]
    fn f_acq_10_metrics_become_readings_only_when_measured() {
        let device = DeviceId::from_uuid(uuid::Uuid::nil());
        let none = LinkMetrics {
            rssi_dbm: None,
            packet_loss_pct: None,
            reconnect_count: 0,
        };
        let params: Vec<_> = none
            .readings(device, TaiNs(1), None)
            .iter()
            .map(|r| r.parameter)
            .collect();
        assert_eq!(params, [Parameter::ReconnectCount]);
        let all = LinkMetrics {
            rssi_dbm: Some(-60.0),
            packet_loss_pct: Some(2.0),
            reconnect_count: 3,
        };
        let offset = ClockOffset {
            offset_ns: -4_000_000,
            residual_ns: 0,
            measured_at: TaiNs(0),
        };
        let readings = all.readings(device, TaiNs(1), Some(&offset));
        assert_eq!(readings.len(), 4);
        assert_eq!(readings[3].value, Some(RawValue::Number(-4.0)));
    }

    #[test]
    fn rssi_is_the_last_finite_report_and_none_before() {
        let mut link = LinkSupervisor::new(ReconnectPolicy::default(), 0, None);
        assert_eq!(link.metrics(0).rssi_dbm, None);
        link.rssi(-61.0);
        link.rssi(f64::NAN);
        assert_eq!(link.metrics(0).rssi_dbm, Some(-61.0));
    }
}
