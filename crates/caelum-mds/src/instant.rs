//! FHIR `instant` / `dateTime` ↔ [`TaiNs`] (R-OPS-05: every timestamp is TAI internally).
//!
//! Output is UTC with millisecond precision and `Z`, including `23:59:60` during an inserted leap
//! second. Input must carry seconds and a time zone; a date-only or zone-less value is rejected,
//! never completed with guessed fields.

use caelum_domain::time::{LeapSeconds, TaiNs, Validity};

const NS_PER_S: i64 = 1_000_000_000;
const S_PER_DAY: i64 = 86_400;

/// Why a timestamp could not be converted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstantError {
    /// Not `YYYY-MM-DDThh:mm:ss[.f]` followed by `Z` or `±hh:mm`.
    #[error("not a FHIR instant with seconds and time zone: {0}")]
    Format(String),
    /// Outside the leap-second table (before 1972).
    #[error("time outside the leap-second table: {0}")]
    OutOfRange(String),
}

/// Days since 1970-01-01 to a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date of a day count since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Formats a TAI instant as a FHIR `instant` in UTC (millisecond precision). After the
/// leap-second table's expiry the UTC rendering uses the last known offset.
pub fn format_instant(t: TaiNs, leap: &LeapSeconds) -> Result<String, InstantError> {
    let (utc, _validity) = leap
        .tai_to_utc(t)
        .map_err(|e| InstantError::OutOfRange(e.to_string()))?;
    let secs = utc.unix_ns.div_euclid(NS_PER_S);
    let ms = utc.unix_ns.rem_euclid(NS_PER_S) / 1_000_000;
    let (y, mo, d) = civil_from_days(secs.div_euclid(S_PER_DAY));
    let sod = secs.rem_euclid(S_PER_DAY);
    let ss = if utc.leap_second { 60 } else { sod % 60 };
    Ok(format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{ss:02}.{ms:03}Z",
        sod / 3_600,
        sod % 3_600 / 60
    ))
}

fn num(s: &str, range: std::ops::Range<usize>) -> Option<i64> {
    let part = s.get(range)?;
    if part.bytes().all(|b| b.is_ascii_digit()) {
        part.parse().ok()
    } else {
        None
    }
}

/// Parses a FHIR `instant` or a `dateTime` with seconds and time zone into TAI.
pub fn parse_instant(s: &str, leap: &LeapSeconds) -> Result<(TaiNs, Validity), InstantError> {
    let bad = || InstantError::Format(s.to_owned());
    let b = s.as_bytes();
    if b.len() < 20 || [b[4], b[7], b[10], b[13], b[16]] != *b"--T::" {
        return Err(bad());
    }
    let field = |r| num(s, r).ok_or_else(bad);
    let (y, mo, d) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hh, mi, ss) = (field(11..13)?, field(14..16)?, field(17..19)?);
    if !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) || hh > 23 || mi > 59 {
        return Err(bad());
    }
    if ss > 60 {
        return Err(bad());
    }
    let mut rest = &s[19..];
    let mut frac_ns = 0i64;
    if let Some(f) = rest.strip_prefix('.') {
        let digits = f.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 9 {
            return Err(bad());
        }
        let padded = format!("{:0<9}", &f[..digits]);
        frac_ns = padded.parse().map_err(|_| bad())?;
        rest = &f[digits..];
    }
    let offset_s = match rest {
        "Z" => 0,
        _ if rest.len() == 6 && rest.as_bytes()[3] == b':' => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return Err(bad()),
            };
            let oh = num(rest, 1..3).ok_or_else(bad)?;
            let om = num(rest, 4..6).ok_or_else(bad)?;
            if oh > 14 || om > 59 {
                return Err(bad());
            }
            sign * (oh * 3_600 + om * 60)
        }
        _ => return Err(bad()),
    };
    // A leap second is parsed as the preceding second plus one TAI second, then checked.
    let local_s = days_from_civil(y, mo, d) * S_PER_DAY + hh * 3_600 + mi * 60 + ss.min(59);
    let unix_ns = (local_s - offset_s) * NS_PER_S + frac_ns;
    let (tai, validity) = leap
        .utc_to_tai(unix_ns)
        .map_err(|e| InstantError::OutOfRange(e.to_string()))?;
    if ss < 60 {
        return Ok((tai, validity));
    }
    let tai = TaiNs(tai.0 + NS_PER_S);
    match leap.tai_to_utc(tai) {
        Ok((utc, _)) if utc.leap_second => Ok((tai, validity)),
        _ => Err(bad()),
    }
}

/// TAI instant of a Unix time in whole seconds (for configuration and fixtures that think in UTC).
pub fn tai_from_unix_s(unix_s: i64, leap: &LeapSeconds) -> Result<TaiNs, InstantError> {
    leap.utc_to_tai(unix_s * NS_PER_S)
        .map(|(t, _)| t)
        .map_err(|e| InstantError::OutOfRange(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leap() -> LeapSeconds {
        LeapSeconds::bundled().expect("bundled table")
    }

    fn unix(y: i64, m: i64, d: i64, hh: i64, mm: i64, ss: i64) -> i64 {
        days_from_civil(y, m, d) * S_PER_DAY + hh * 3_600 + mm * 60 + ss
    }

    #[test]
    fn civil_round_trip() {
        for z in [-1, 0, 59, 365, 11_016, 20_736, 100_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
    }

    /// R-OPS-05: TAI ↔ FHIR instant round-trips at millisecond precision.
    #[test]
    fn format_and_parse_round_trip() {
        let l = leap();
        let t = tai_from_unix_s(unix(2026, 10, 10, 12, 34, 56), &l).expect("in table");
        let t = TaiNs(t.0 + 789_000_000);
        let s = format_instant(t, &l).expect("formats");
        assert_eq!(s, "2026-10-10T12:34:56.789Z");
        assert_eq!(parse_instant(&s, &l).expect("parses"), (t, Validity::Exact));
        let offset = parse_instant("2026-10-10T14:34:56.789+02:00", &l).expect("offset");
        assert_eq!(offset.0, t);
        let no_frac = parse_instant("2026-10-10T12:34:56Z", &l).expect("no fraction");
        assert_eq!(no_frac.0 .0, t.0 - 789_000_000);
    }

    /// The inserted leap second is written and read as `23:59:60`.
    #[test]
    fn leap_second_is_represented() {
        let l = leap();
        let before = tai_from_unix_s(unix(2016, 12, 31, 23, 59, 59), &l).expect("in table");
        let leap_s = TaiNs(before.0 + NS_PER_S);
        let s = format_instant(leap_s, &l).expect("formats");
        assert_eq!(s, "2016-12-31T23:59:60.000Z");
        assert_eq!(parse_instant(&s, &l).expect("parses").0, leap_s);
        assert!(parse_instant("2016-12-30T23:59:60Z", &l).is_err());
    }

    /// Imprecise or malformed times are rejected, never completed.
    #[test]
    fn imprecise_times_are_rejected() {
        let l = leap();
        for s in [
            "2026-10-10",
            "2026-10-10T12:00",
            "2026-10-10T12:00:00",
            "2026-13-10T12:00:00Z",
            "2026-02-30T12:00:00Z",
            "2026-10-10T24:00:00Z",
            "2026-10-10T12:00:00.Z",
            "2026-10-10T12:00:00.1234567890Z",
            "2026-10-10T12:00:00+2:00",
            "2026-10-10T12:00:00*02:00",
            "2026-10-10T12:00:00+15:00",
            "2026-1a-10T12:00:00Z",
        ] {
            assert!(
                matches!(parse_instant(s, &l), Err(InstantError::Format(_))),
                "{s}"
            );
        }
        assert!(matches!(
            parse_instant("1960-01-01T00:00:00Z", &l),
            Err(InstantError::OutOfRange(_))
        ));
        assert!(format_instant(TaiNs(0), &l).is_err());
    }
}
