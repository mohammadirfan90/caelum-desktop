//! Device clock offsets (F-ACQ-11, R-OPS-04, docs/05 §4).
//!
//! An offset is measured from exchanges in which the station sends a request at `t_send`, the
//! device answers with its own time `t_device`, and the answer arrives at `t_recv` (all ns). The
//! device's reading of the midpoint is compared with the station's midpoint; the exchange with the
//! shortest round trip is the most accurate, and half its round trip bounds the remaining error
//! (the residual). A residual above [`MAX_RESIDUAL_NS`] raises a technical alarm.

use caelum_domain::time::TaiNs;
use serde::Serialize;

/// R-OPS-04: residual offset above 50 ms raises a technical alarm.
pub const MAX_RESIDUAL_NS: u64 = 50_000_000;

/// One request/answer exchange with a device (station times in TAI ns, device time in the
/// device's clock on the same scale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncSample {
    /// Station time the request was sent.
    pub station_send: TaiNs,
    /// The device's clock in its answer.
    pub device_time: TaiNs,
    /// Station time the answer arrived.
    pub station_recv: TaiNs,
}

/// A measured device clock offset: add `offset_ns` to a device time to get station (TAI) time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClockOffset {
    /// Station minus device, ns.
    pub offset_ns: i64,
    /// Bound on the remaining error, ns (half the best round trip).
    pub residual_ns: u64,
    /// When the offset was measured (station TAI).
    pub measured_at: TaiNs,
}

/// Why no offset could be measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    /// No exchanges given.
    #[error("no clock exchanges")]
    NoSamples,
    /// Every exchange had its answer arrive before its request was sent.
    #[error("clock exchanges are inconsistent (answer before request)")]
    Inconsistent,
}

impl ClockOffset {
    /// Estimates the offset from exchanges, using the one with the shortest round trip.
    pub fn estimate(samples: &[SyncSample]) -> Result<Self, SyncError> {
        if samples.is_empty() {
            return Err(SyncError::NoSamples);
        }
        let best = samples
            .iter()
            .filter(|s| s.station_recv.0 >= s.station_send.0)
            .min_by_key(|s| s.station_recv.0 - s.station_send.0)
            .ok_or(SyncError::Inconsistent)?;
        let rtt = best.station_recv.0 - best.station_send.0;
        let midpoint = best.station_send.0 + rtt / 2;
        Ok(Self {
            offset_ns: midpoint - best.device_time.0,
            residual_ns: (rtt as u64).div_ceil(2),
            measured_at: best.station_recv,
        })
    }

    /// R-OPS-04: whether the remaining error is too large (technical alarm).
    pub fn residual_exceeds_limit(&self) -> bool {
        self.residual_ns > MAX_RESIDUAL_NS
    }

    /// A device time on the station's (TAI) clock.
    pub fn correct(&self, device_time: TaiNs) -> TaiNs {
        TaiNs(device_time.0.saturating_add(self.offset_ns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(send: i64, device: i64, recv: i64) -> SyncSample {
        SyncSample {
            station_send: TaiNs(send),
            device_time: TaiNs(device),
            station_recv: TaiNs(recv),
        }
    }

    /// F-ACQ-11: a device 3 s behind is corrected by +3 s; the shortest round trip wins.
    #[test]
    fn f_acq_11_offset_from_the_shortest_round_trip() {
        let s = 1_000_000_000_000;
        let samples = [
            sample(s, s - 3_000_000_000 + 40_000_000, s + 80_000_000), // rtt 80 ms
            sample(
                s + 1_000_000_000,
                s - 2_000_000_000 + 10_000_000,
                s + 1_020_000_000,
            ), // rtt 20 ms
        ];
        let o = ClockOffset::estimate(&samples).expect("offset");
        assert_eq!(o.offset_ns, 3_000_000_000);
        assert_eq!(o.residual_ns, 10_000_000);
        assert!(!o.residual_exceeds_limit());
        assert_eq!(o.correct(TaiNs(5)), TaiNs(3_000_000_005));
    }

    /// R-OPS-04: residual above 50 ms is flagged.
    #[test]
    fn r_ops_04_residual_over_50_ms_is_flagged() {
        let o = ClockOffset::estimate(&[sample(0, 0, 102_000_000)]).expect("offset");
        assert_eq!(o.residual_ns, 51_000_000);
        assert!(o.residual_exceeds_limit());
        let ok = ClockOffset::estimate(&[sample(0, 0, 100_000_000)]).expect("offset");
        assert!(
            !ok.residual_exceeds_limit(),
            "exactly 50 ms is within the limit"
        );
    }

    #[test]
    fn no_or_inconsistent_samples_give_no_offset() {
        assert_eq!(ClockOffset::estimate(&[]), Err(SyncError::NoSamples));
        assert_eq!(
            ClockOffset::estimate(&[sample(10, 0, 5)]),
            Err(SyncError::Inconsistent)
        );
    }
}
