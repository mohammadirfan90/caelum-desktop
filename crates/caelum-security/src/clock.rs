//! Time sources for security decisions, injected so tests control them.
//!
//! Session timing (auto-lock, re-authentication window, sign-in lockout) uses a monotonic clock:
//! setting the wall clock back can never extend a session. Audit timestamps use TAI derived from
//! the system clock with the bundled leap-second table (R-OPS-05).

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use caelum_domain::time::{LeapSeconds, TaiNs, TimeError};

/// A time source.
pub trait Clock: Send + Sync {
    /// Current TAI time, for audit timestamps.
    fn now_tai(&self) -> Result<TaiNs, ClockError>;
    /// Nanoseconds since an arbitrary fixed origin; never decreases.
    fn monotonic_ns(&self) -> u64;
}

/// Clock errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClockError {
    /// The system clock is before 1970 or outside the range TAI nanoseconds can hold.
    #[error("the system clock is not set to a valid time")]
    InvalidSystemTime,
    /// Leap-second conversion failed.
    #[error(transparent)]
    Time(#[from] TimeError),
}

/// The operating-system clock.
pub struct SystemClock {
    leap: LeapSeconds,
    origin: Instant,
}

impl SystemClock {
    /// A system clock using the bundled leap-second table.
    pub fn new() -> Result<Self, ClockError> {
        Ok(Self {
            leap: LeapSeconds::bundled()?,
            origin: Instant::now(),
        })
    }
}

impl Clock for SystemClock {
    fn now_tai(&self) -> Result<TaiNs, ClockError> {
        let since_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ClockError::InvalidSystemTime)?;
        let unix_ns =
            i64::try_from(since_unix.as_nanos()).map_err(|_| ClockError::InvalidSystemTime)?;
        // A time beyond the table's expiry still converts (with the last known offset); the
        // audit entry carries the TAI value either way.
        let (tai, _validity) = self.leap.utc_to_tai(unix_ns)?;
        Ok(tai)
    }

    fn monotonic_ns(&self) -> u64 {
        u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}
