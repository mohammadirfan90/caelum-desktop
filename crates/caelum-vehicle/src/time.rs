//! Vehicle time correlation (F-ACQ-11, R-OPS-05, docs/05 §4, ICD-VEH §6).
//!
//! Vehicle time stamps (cFS secondary header) are converted to TAI with the configured epoch and
//! scale; a UTC-scale clock goes through the signed leap-second table. MET comes from the epoch
//! in signed mission configuration, passed in by the caller.
//!
//! Vehicle time is used as a reading's measurement time only while the vehicle reports its clock
//! as set (the configured time message, not older than `clock_state_max_age_s`). Otherwise the
//! reading gets the station receipt time, marked unsynchronised, exactly as an unsynchronised
//! device would (docs/05 §4). A vehicle time more than 1 s after receipt is a clock fault.

use std::collections::VecDeque;

use caelum_domain::time::{LeapSeconds, MetEpoch, MetNs, TaiNs, Validity};
use serde::Serialize;

use crate::config::VehicleEpoch;
use crate::packet::VehicleTime;

/// A vehicle time later than this after receipt is a clock fault (docs/05 §4), ns.
pub const MAX_FUTURE_NS: i64 = 1_000_000_000;
/// Offsets kept for the correlation statistics.
pub const WINDOW: usize = 64;

/// Vehicle time on the TAI scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VehicleTai {
    /// TAI.
    pub tai: TaiNs,
    /// False when a UTC-scale time lies past the leap-second table's expiry.
    pub exact: bool,
}

/// Converts a vehicle time stamp to TAI. `None` when it cannot be represented (UTC before 1972 or
/// overflow).
pub fn to_tai(epoch: VehicleEpoch, t: VehicleTime, leap: &LeapSeconds) -> Option<VehicleTai> {
    match epoch {
        VehicleEpoch::Tai(e) => Some(VehicleTai {
            tai: TaiNs(e.0.checked_add(t.nanos())?),
            exact: true,
        }),
        VehicleEpoch::Utc { unix_s } => {
            let unix_ns = unix_s.checked_mul(1_000_000_000)?.checked_add(t.nanos())?;
            let (tai, validity) = leap.utc_to_tai(unix_ns).ok()?;
            Some(VehicleTai {
                tai,
                exact: validity == Validity::Exact,
            })
        }
    }
}

/// Where a reading's measurement time came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TimeSource {
    /// Vehicle time (clock reported set), converted to TAI.
    Vehicle {
        /// Receipt minus vehicle time, ns (link latency plus clock offset).
        offset_ns: i64,
        /// False when a UTC-scale vehicle time lies past the leap-table expiry.
        exact: bool,
    },
    /// Vehicle clock not reported set (or no time message configured): station receipt time.
    ReceiptUnsynchronised,
}

/// The vehicle clock as last reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockState {
    /// No time message is configured: vehicle time is never used.
    NotConfigured,
    /// No time message received yet.
    Unknown,
    /// The vehicle reports its clock set.
    Set,
    /// The vehicle reports its clock not set.
    NotSet,
    /// The last report is older than `clock_state_max_age_s`.
    Stale,
}

/// Correlation status for the Environment screen and technical health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CorrelationStatus {
    /// Vehicle clock state.
    pub clock: ClockState,
    /// Offsets in the window.
    pub samples: usize,
    /// Receipt minus vehicle time of the latest packet, ns.
    pub latest_offset_ns: Option<i64>,
    /// Smallest offset in the window (closest to pure clock offset), ns.
    pub min_offset_ns: Option<i64>,
    /// Largest offset in the window, ns.
    pub max_offset_ns: Option<i64>,
    /// Latest vehicle time on the TAI scale.
    pub latest_vehicle_tai: Option<TaiNs>,
    /// MET of the latest vehicle time.
    pub latest_met: Option<MetNs>,
    /// TAI−UTC as the vehicle reports it, s.
    pub vehicle_leap_seconds: Option<i64>,
    /// TAI−UTC from the station's signed table, s.
    pub station_leap_seconds: i64,
}

#[derive(Debug, Clone, Copy)]
struct Report {
    set: bool,
    at: TaiNs,
}

/// Tracks the vehicle clock and the receipt–vehicle offset.
#[derive(Debug, Clone)]
pub struct Correlator {
    configured: bool,
    max_age_ns: i64,
    met: MetEpoch,
    station_leap_s: i64,
    report: Option<Report>,
    vehicle_leap_s: Option<i64>,
    offsets: VecDeque<i64>,
    latest: Option<TaiNs>,
}

impl Correlator {
    /// A correlator; `configured` says whether a time message exists in the MID table.
    pub fn new(configured: bool, max_age_s: u32, met: MetEpoch, leap: &LeapSeconds) -> Self {
        Self {
            configured,
            max_age_ns: i64::from(max_age_s) * 1_000_000_000,
            met,
            station_leap_s: leap.latest_offset_s(),
            report: None,
            vehicle_leap_s: None,
            offsets: VecDeque::with_capacity(WINDOW),
            latest: None,
        }
    }

    /// The MET of a TAI instant.
    pub fn met(&self, at: TaiNs) -> MetNs {
        self.met.elapsed(at)
    }

    /// Records a time message: clock set or not, and the vehicle's leap seconds if reported.
    pub fn report(&mut self, set: bool, leap_s: Option<i64>, received_at: TaiNs) {
        self.report = Some(Report {
            set,
            at: received_at,
        });
        if leap_s.is_some() {
            self.vehicle_leap_s = leap_s;
        }
    }

    /// The clock state at `now`.
    pub fn clock(&self, now: TaiNs) -> ClockState {
        if !self.configured {
            return ClockState::NotConfigured;
        }
        match self.report {
            None => ClockState::Unknown,
            Some(r) if now.0.saturating_sub(r.at.0) > self.max_age_ns => ClockState::Stale,
            Some(r) if r.set => ClockState::Set,
            Some(_) => ClockState::NotSet,
        }
    }

    /// Chooses the measurement time for data stamped `vehicle` and received at `received_at`.
    /// `Err(ahead_ns)` when the vehicle time is more than 1 s after receipt (clock fault).
    pub fn measured_at(
        &mut self,
        vehicle: VehicleTai,
        received_at: TaiNs,
    ) -> Result<(TaiNs, TimeSource), i64> {
        if self.clock(received_at) != ClockState::Set {
            return Ok((received_at, TimeSource::ReceiptUnsynchronised));
        }
        let offset_ns = received_at.0.saturating_sub(vehicle.tai.0);
        if -offset_ns > MAX_FUTURE_NS {
            return Err(-offset_ns);
        }
        self.sample(vehicle.tai, offset_ns);
        Ok((
            vehicle.tai,
            TimeSource::Vehicle {
                offset_ns,
                exact: vehicle.exact,
            },
        ))
    }

    fn sample(&mut self, vehicle: TaiNs, offset_ns: i64) {
        if self.offsets.len() == WINDOW {
            self.offsets.pop_front();
        }
        self.offsets.push_back(offset_ns);
        self.latest = Some(vehicle);
    }

    /// Records the stamp of a time message itself (when the clock is set) for the statistics.
    pub fn observe_time_packet(&mut self, vehicle: VehicleTai, received_at: TaiNs) {
        if self.clock(received_at) == ClockState::Set {
            let offset_ns = received_at.0.saturating_sub(vehicle.tai.0);
            if -offset_ns <= MAX_FUTURE_NS {
                self.sample(vehicle.tai, offset_ns);
            }
        }
    }

    /// Current correlation status.
    pub fn status(&self, now: TaiNs) -> CorrelationStatus {
        CorrelationStatus {
            clock: self.clock(now),
            samples: self.offsets.len(),
            latest_offset_ns: self.offsets.back().copied(),
            min_offset_ns: self.offsets.iter().copied().min(),
            max_offset_ns: self.offsets.iter().copied().max(),
            latest_vehicle_tai: self.latest,
            latest_met: self.latest.map(|t| self.met.elapsed(t)),
            vehicle_leap_seconds: self.vehicle_leap_s,
            station_leap_seconds: self.station_leap_s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;

    fn leap() -> LeapSeconds {
        LeapSeconds::bundled().expect("bundled")
    }

    /// F-ACQ-11 / R-OPS-05: a TAI-scale vehicle clock maps linearly onto TAI from its epoch.
    #[test]
    fn r_int_04_tai_scale_vehicle_time_to_tai() {
        let epoch = VehicleEpoch::Tai(TaiNs(8_040 * 86_400 * S));
        let t = VehicleTime {
            seconds: 1_400_000_000,
            subseconds: 0x4000,
        };
        let v = to_tai(epoch, t, &leap()).expect("representable");
        assert_eq!(v.tai.0, (8_040 * 86_400 + 1_400_000_000) * S + S / 4);
        assert!(v.exact);
    }

    /// R-OPS-05: a UTC-scale vehicle clock goes through the signed leap-second table.
    #[test]
    fn r_ops_05_utc_scale_vehicle_time_uses_leap_table() {
        // Epoch 2017-01-01T00:00:00 UTC (TAI−UTC = 37 s from then on).
        let unix_s = 1_483_228_800;
        let epoch = VehicleEpoch::Utc { unix_s };
        let t = VehicleTime {
            seconds: 100,
            subseconds: 0,
        };
        let v = to_tai(epoch, t, &leap()).expect("representable");
        let s_1958_to_1970 = 4_383 * 86_400;
        assert_eq!(v.tai.0, (unix_s + 100 + s_1958_to_1970 + 37) * S);
        assert!(v.exact);
        let far = VehicleTime {
            seconds: 400_000_000,
            subseconds: 0,
        };
        assert!(!to_tai(epoch, far, &leap()).expect("representable").exact);
    }

    fn correlator() -> Correlator {
        Correlator::new(true, 30, MetEpoch(TaiNs(1_000 * S)), &leap())
    }

    fn vt(s: i64) -> VehicleTai {
        VehicleTai {
            tai: TaiNs(s * S),
            exact: true,
        }
    }

    /// docs/05 §4: vehicle time is used only while the vehicle reports its clock set and fresh.
    #[test]
    fn f_acq_11_vehicle_time_needs_a_fresh_clock_set_report() {
        let mut c = correlator();
        let rx = TaiNs(5_000 * S);
        assert_eq!(c.clock(rx), ClockState::Unknown);
        assert_eq!(
            c.measured_at(vt(4_999), rx),
            Ok((rx, TimeSource::ReceiptUnsynchronised))
        );
        c.report(false, None, rx);
        assert_eq!(c.clock(rx), ClockState::NotSet);
        assert_eq!(
            c.measured_at(vt(4_999), rx),
            Ok((rx, TimeSource::ReceiptUnsynchronised))
        );
        c.report(true, Some(37), rx);
        assert_eq!(c.clock(rx), ClockState::Set);
        let (at, src) = c.measured_at(vt(4_999), rx).expect("in range");
        assert_eq!(at, TaiNs(4_999 * S));
        assert_eq!(
            src,
            TimeSource::Vehicle {
                offset_ns: S,
                exact: true
            }
        );
        // 31 s later the report is stale.
        let later = TaiNs(5_031 * S);
        assert_eq!(c.clock(later), ClockState::Stale);
        assert_eq!(
            c.measured_at(vt(5_030), later),
            Ok((later, TimeSource::ReceiptUnsynchronised))
        );
        let none = Correlator::new(false, 30, MetEpoch(TaiNs(0)), &leap());
        assert_eq!(none.clock(rx), ClockState::NotConfigured);
    }

    /// docs/05 §4: a vehicle time more than 1 s after receipt is a clock fault.
    #[test]
    fn f_acq_11_future_vehicle_time_is_a_clock_fault() {
        let mut c = correlator();
        let rx = TaiNs(5_000 * S);
        c.report(true, None, rx);
        assert_eq!(c.measured_at(vt(5_002), rx), Err(2 * S));
        assert!(c.measured_at(vt(5_001), rx).is_ok());
    }

    /// R-OPS-05: the status reports offsets, MET from the signed epoch and both leap counts.
    #[test]
    fn r_ops_05_status_reports_offsets_met_and_leap_seconds() {
        let mut c = correlator();
        let rx = TaiNs(5_000 * S);
        c.report(true, Some(37), rx);
        for (v, r) in [(4_999, 5_000), (4_999, 5_001), (5_000, 5_000)] {
            c.measured_at(vt(v), TaiNs(r * S)).expect("ok");
        }
        let s = c.status(rx);
        assert_eq!(s.clock, ClockState::Set);
        assert_eq!(s.samples, 3);
        assert_eq!(s.latest_offset_ns, Some(0));
        assert_eq!(s.min_offset_ns, Some(0));
        assert_eq!(s.max_offset_ns, Some(2 * S));
        assert_eq!(s.latest_met, Some(MetNs(4_000 * S)));
        assert_eq!(s.vehicle_leap_seconds, Some(37));
        assert_eq!(s.station_leap_seconds, 37);
        for _ in 0..(2 * WINDOW) {
            c.observe_time_packet(vt(5_000), rx);
        }
        assert_eq!(c.status(rx).samples, WINDOW);
    }
}
