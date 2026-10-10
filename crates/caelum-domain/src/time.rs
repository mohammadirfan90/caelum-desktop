//! Time references (R-OPS-05, docs/05_DATA_MODEL.md §4).
//!
//! - [`TaiNs`]: TAI nanoseconds since the CCSDS epoch 1958-01-01T00:00:00 TAI. Every timestamp is
//!   stored this way; it never jumps and never repeats.
//! - UTC is derived from TAI with the IERS leap-second table bundled in `data/Leap_Second.dat`
//!   ([`LeapSeconds::bundled`]), which only signed configuration may replace in operation.
//! - [`MetEpoch`]: mission elapsed time from an epoch set by signed mission configuration.
//!
//! UTC before 1972-01-01 is not supported (no integral leap-second offsets before then). A time
//! after the table's expiry is converted with the last known offset and reported as
//! [`Validity::BeyondTable`], never silently as exact.

use std::fmt;

use serde::{Deserialize, Serialize};

const NS_PER_S: i64 = 1_000_000_000;
const S_PER_DAY: i64 = 86_400;
/// Seconds from 1958-01-01 to 1970-01-01 in a uniform day count (12 years, 3 leap days).
const S_1958_TO_1970: i64 = 4_383 * S_PER_DAY;
/// Modified Julian Date of 1970-01-01.
const MJD_UNIX_EPOCH: i64 = 40_587;
/// Unix seconds of 1972-01-01T00:00:00 UTC, the start of integral leap seconds.
const UNIX_1972: i64 = 63_072_000;

/// TAI nanoseconds since 1958-01-01T00:00:00 TAI (CCSDS epoch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaiNs(pub i64);

impl TaiNs {
    /// TAI seconds and nanoseconds parts (seconds floor, nanos in 0..1e9).
    pub fn split(self) -> (i64, i64) {
        (self.0.div_euclid(NS_PER_S), self.0.rem_euclid(NS_PER_S))
    }
}

/// A UTC instant as Unix time (seconds since 1970-01-01, leap seconds not counted) plus a flag
/// for the inserted leap second itself, which Unix time cannot represent (shown as `23:59:60`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Utc {
    /// Unix nanoseconds; during a leap second this is the preceding second (`23:59:59`).
    pub unix_ns: i64,
    /// True during an inserted leap second.
    pub leap_second: bool,
}

/// Whether a conversion is backed by the leap-second table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Validity {
    /// Inside the period the table covers: exact.
    Exact,
    /// After the table's expiry: a leap second may have been announced that the table lacks.
    BeyondTable,
}

/// Time conversion errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimeError {
    /// UTC before 1972 has no integral TAI−UTC offset.
    #[error("UTC before 1972-01-01 is not supported")]
    BeforeTable,
    /// The leap-second file could not be parsed.
    #[error("leap-second table: {0}")]
    Table(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    /// Unix seconds (UTC) from which `offset_s` applies.
    unix_start: i64,
    /// TAI−UTC in seconds.
    offset_s: i64,
}

/// The IERS TAI−UTC table (`Leap_Second.dat` format).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeapSeconds {
    entries: Vec<Entry>,
    /// Unix seconds (UTC) at which the table expires.
    expires_unix: i64,
}

const BUNDLED: &str = include_str!("../data/Leap_Second.dat");

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Days from 1970-01-01 to a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

impl LeapSeconds {
    /// The table bundled with this build (IERS Bulletin C, see `data/README.md`).
    pub fn bundled() -> Result<Self, TimeError> {
        Self::parse(BUNDLED)
    }

    /// Parses an IERS `Leap_Second.dat` file.
    pub fn parse(text: &str) -> Result<Self, TimeError> {
        let mut entries = Vec::new();
        let mut expires_unix = None;
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix('#') {
                if let Some(date) = rest.trim().strip_prefix("File expires on") {
                    expires_unix = Some(Self::parse_expiry(date.trim())?);
                }
                continue;
            }
            if line.is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [mjd, _d, _m, _y, offset] = fields.as_slice() else {
                return Err(TimeError::Table(format!("unexpected line: {line}")));
            };
            let mjd: f64 = mjd
                .parse()
                .map_err(|_| TimeError::Table(format!("bad MJD: {mjd}")))?;
            let offset_s: i64 = offset
                .parse()
                .map_err(|_| TimeError::Table(format!("bad offset: {offset}")))?;
            if mjd.fract() != 0.0 {
                return Err(TimeError::Table(format!("non-integral MJD: {mjd}")));
            }
            // Integral MJD values within i64 range: the cast is exact.
            #[allow(clippy::cast_possible_truncation)]
            let unix_start = (mjd as i64 - MJD_UNIX_EPOCH) * S_PER_DAY;
            entries.push(Entry {
                unix_start,
                offset_s,
            });
        }
        let expires_unix =
            expires_unix.ok_or_else(|| TimeError::Table("no 'File expires on' line".into()))?;
        if entries.is_empty() {
            return Err(TimeError::Table("no entries".into()));
        }
        if entries.windows(2).any(|w| {
            w[1].unix_start <= w[0].unix_start || (w[1].offset_s - w[0].offset_s).abs() != 1
        }) {
            return Err(TimeError::Table(
                "entries must be increasing, one second apart".into(),
            ));
        }
        if entries[0].unix_start != UNIX_1972 {
            return Err(TimeError::Table("table must start at 1972-01-01".into()));
        }
        Ok(Self {
            entries,
            expires_unix,
        })
    }

    fn parse_expiry(date: &str) -> Result<i64, TimeError> {
        let bad = || TimeError::Table(format!("bad expiry date: {date}"));
        let parts: Vec<&str> = date.split_whitespace().collect();
        let [day, month, year] = parts.as_slice() else {
            return Err(bad());
        };
        let d: i64 = day.parse().map_err(|_| bad())?;
        let y: i64 = year.parse().map_err(|_| bad())?;
        let m = MONTHS.iter().position(|n| n == month).ok_or_else(bad)?;
        // `position` is < 12.
        #[allow(clippy::cast_possible_wrap)]
        let m = m as i64 + 1;
        Ok(days_from_civil(y, m, d) * S_PER_DAY)
    }

    /// The current TAI−UTC offset in seconds (last entry).
    pub fn latest_offset_s(&self) -> i64 {
        self.entries.last().map_or(0, |e| e.offset_s)
    }

    /// Unix seconds at which the table expires.
    pub fn expires_unix(&self) -> i64 {
        self.expires_unix
    }

    fn validity(&self, unix_s: i64) -> Validity {
        if unix_s >= self.expires_unix {
            Validity::BeyondTable
        } else {
            Validity::Exact
        }
    }

    /// UTC (Unix nanoseconds) to TAI.
    pub fn utc_to_tai(&self, unix_ns: i64) -> Result<(TaiNs, Validity), TimeError> {
        let unix_s = unix_ns.div_euclid(NS_PER_S);
        if unix_s < UNIX_1972 {
            return Err(TimeError::BeforeTable);
        }
        let offset = self
            .entries
            .iter()
            .rev()
            .find(|e| unix_s >= e.unix_start)
            .map_or(0, |e| e.offset_s);
        let tai = unix_ns + (S_1958_TO_1970 + offset) * NS_PER_S;
        Ok((TaiNs(tai), self.validity(unix_s)))
    }

    /// TAI to UTC, including the inserted leap second itself.
    pub fn tai_to_utc(&self, tai: TaiNs) -> Result<(Utc, Validity), TimeError> {
        let (tai_s, frac) = tai.split();
        // For each entry, the TAI second at which it starts.
        for (i, e) in self.entries.iter().enumerate().rev() {
            let start_tai_s = e.unix_start + S_1958_TO_1970 + e.offset_s;
            if tai_s >= start_tai_s {
                let unix_s = tai_s - S_1958_TO_1970 - e.offset_s;
                let utc = Utc {
                    unix_ns: unix_s * NS_PER_S + frac,
                    leap_second: false,
                };
                return Ok((utc, self.validity(unix_s)));
            }
            // The second just before an inserted leap second's entry is the leap second.
            let inserted = i > 0 && e.offset_s == self.entries[i - 1].offset_s + 1;
            if inserted && tai_s == start_tai_s - 1 {
                let unix_s = e.unix_start - 1;
                let utc = Utc {
                    unix_ns: unix_s * NS_PER_S + frac,
                    leap_second: true,
                };
                return Ok((utc, self.validity(unix_s)));
            }
        }
        Err(TimeError::BeforeTable)
    }
}

/// Mission elapsed time epoch, from signed mission configuration (R-OPS-05).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetEpoch(pub TaiNs);

/// Signed mission elapsed time in nanoseconds (negative before the epoch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MetNs(pub i64);

impl MetEpoch {
    /// MET at a TAI instant.
    pub fn elapsed(self, at: TaiNs) -> MetNs {
        MetNs(at.0 - (self.0).0)
    }
}

impl fmt::Display for MetNs {
    /// `[-]DDD/HH:MM:SS` (whole seconds).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let total = (self.0 / NS_PER_S).abs();
        let (d, rem) = (total / S_PER_DAY, total % S_PER_DAY);
        write!(
            f,
            "{sign}{d:03}/{:02}:{:02}:{:02}",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> LeapSeconds {
        LeapSeconds::bundled().expect("bundled table parses")
    }

    fn unix(y: i64, m: i64, d: i64, hh: i64, mm: i64, ss: i64) -> i64 {
        (days_from_civil(y, m, d) * S_PER_DAY + hh * 3600 + mm * 60 + ss) * NS_PER_S
    }

    /// R-OPS-05: the bundled table is IERS Bulletin C with TAI−UTC = 37 s since 2017.
    #[test]
    fn bundled_table_is_current_iers_bulletin() {
        let t = table();
        assert_eq!(t.latest_offset_s(), 37);
        assert_eq!(t.entries.len(), 28);
        assert_eq!(t.expires_unix() * NS_PER_S, unix(2027, 6, 28, 0, 0, 0));
    }

    /// R-OPS-05: TAI = UTC + ΔAT, counted from the CCSDS epoch 1958-01-01.
    #[test]
    fn utc_to_tai_uses_the_offset_in_force() {
        let t = table();
        let (tai, v) = t
            .utc_to_tai(unix(1972, 1, 1, 0, 0, 0))
            .expect("1972 supported");
        assert_eq!(tai.0, (S_1958_TO_1970 + UNIX_1972 + 10) * NS_PER_S);
        assert_eq!(v, Validity::Exact);
        let (tai, _) = t
            .utc_to_tai(unix(2026, 10, 10, 12, 0, 0))
            .expect("2026 supported");
        assert_eq!(
            tai.0 - unix(2026, 10, 10, 12, 0, 0),
            (S_1958_TO_1970 + 37) * NS_PER_S
        );
    }

    /// R-OPS-05: across the 2016-12-31 leap second, TAI runs on and UTC shows 23:59:60 once.
    #[test]
    fn leap_second_2016_is_represented_exactly() {
        let t = table();
        let (before, _) = t.utc_to_tai(unix(2016, 12, 31, 23, 59, 59)).expect("valid");
        let leap = TaiNs(before.0 + NS_PER_S);
        let after = TaiNs(before.0 + 2 * NS_PER_S);

        let (u, _) = t.tai_to_utc(before).expect("valid");
        assert_eq!(
            u,
            Utc {
                unix_ns: unix(2016, 12, 31, 23, 59, 59),
                leap_second: false
            }
        );
        let (u, _) = t.tai_to_utc(leap).expect("valid");
        assert_eq!(
            u,
            Utc {
                unix_ns: unix(2016, 12, 31, 23, 59, 59),
                leap_second: true
            }
        );
        let (u, _) = t.tai_to_utc(after).expect("valid");
        assert_eq!(
            u,
            Utc {
                unix_ns: unix(2017, 1, 1, 0, 0, 0),
                leap_second: false
            }
        );
    }

    /// Times past the table's expiry are converted but never reported as exact.
    #[test]
    fn beyond_expiry_is_flagged() {
        let t = table();
        let (tai, v) = t
            .utc_to_tai(unix(2027, 7, 1, 0, 0, 0))
            .expect("convertible");
        assert_eq!(v, Validity::BeyondTable);
        assert_eq!(
            t.tai_to_utc(tai).expect("convertible").1,
            Validity::BeyondTable
        );
    }

    #[test]
    fn utc_before_1972_is_rejected() {
        assert_eq!(
            table().utc_to_tai(unix(1971, 12, 31, 23, 59, 59)),
            Err(TimeError::BeforeTable)
        );
        assert_eq!(table().tai_to_utc(TaiNs(0)), Err(TimeError::BeforeTable));
    }

    #[test]
    fn malformed_tables_are_rejected_not_guessed() {
        assert!(LeapSeconds::parse("41317.0 1 1 1972 10\n").is_err()); // no expiry
        let gap = "# File expires on 28 June 2027\n41317.0 1 1 1972 10\n41499.0 1 7 1972 12\n";
        assert!(LeapSeconds::parse(gap).is_err());
        let late_start = "# File expires on 28 June 2027\n41499.0 1 7 1972 11\n";
        assert!(LeapSeconds::parse(late_start).is_err());
    }

    #[test]
    fn met_formats_days_and_sign() {
        let epoch = MetEpoch(TaiNs(1_000 * NS_PER_S));
        let at = TaiNs((1_000 + 2 * S_PER_DAY + 3 * 3600 + 4 * 60 + 5) * NS_PER_S);
        assert_eq!(epoch.elapsed(at).to_string(), "002/03:04:05");
        assert_eq!(
            epoch.elapsed(TaiNs(940 * NS_PER_S)).to_string(),
            "-000/00:01:00"
        );
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// R-OPS-05: UTC → TAI → UTC is the identity for every non-leap instant 1972–2027.
            #[test]
            fn utc_tai_round_trip(ns in UNIX_1972 * NS_PER_S..unix(2027, 6, 1, 0, 0, 0)) {
                let t = table();
                let (tai, _) = t.utc_to_tai(ns).expect("in range");
                let (utc, _) = t.tai_to_utc(tai).expect("in range");
                prop_assert_eq!(utc, Utc { unix_ns: ns, leap_second: false });
            }

            /// TAI never runs backwards as UTC advances.
            #[test]
            fn tai_is_monotonic(a in UNIX_1972 * NS_PER_S..unix(2027, 1, 1, 0, 0, 0), step in 0i64..10 * NS_PER_S) {
                let t = table();
                let (ta, _) = t.utc_to_tai(a).expect("in range");
                let (tb, _) = t.utc_to_tai(a + step).expect("in range");
                prop_assert!(tb >= ta);
            }
        }
    }
}
