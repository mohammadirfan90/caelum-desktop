//! Clinical file formats (M8, CR-014):
//!
//! | Module | Direction | Requirement |
//! |---|---|---|
//! | [`dicom`] | import: images, ultrasound frames, 12-lead ECG waveforms | F-ACQ-06, F-ACQ-07 |
//! | [`edf`] | import and export: EDF / EDF+ (signals and annotations) | F-ACQ-07, F-SYS-08 |
//! | [`aecg`] | import: HL7 aECG (XML) | F-ACQ-07 |
//! | [`scp_ecg`] | import: SCP-ECG (EN 1064), uncompressed rhythm data | F-ACQ-07 |
//! | [`fhir`] | import: FHIR R4 Observation, Device, DiagnosticReport | F-ACQ-08 |
//! | [`csv`] | export: one CSV per parameter (RFC 4180) | F-SYS-08 |
//!
//! Everything here is pure: functions take byte slices or in-memory values and return typed
//! results. File and network I/O (file drop, folder watch, the DICOM C-STORE SCP, export commands,
//! the attachment store) stay with the caller (docs/integration/M8-files.md).
//!
//! No value is defaulted, guessed or invented (CLAUDE.md rule 2): a field a file does not carry
//! stays `None`, and anything that cannot be interpreted unambiguously is rejected with an explicit
//! reason. Importers never write to a device (R-SEC-07).

pub mod aecg;
pub mod csv;
pub mod dicom;
pub mod edf;
pub mod fhir;
pub mod scp_ecg;

use std::fmt;

use caelum_domain::time::{LeapSeconds, TaiNs, Utc, Validity};

const NS_PER_S: i64 = 1_000_000_000;
const S_PER_DAY: i64 = 86_400;

/// A calendar date and time as a file states it. `utc_offset_minutes` is `None` when the file
/// gives no offset from UTC (EDF, most DICOM and SCP-ECG files record local time only); such a
/// time is never converted to TAI by assuming an offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilDateTime {
    /// Year (proleptic Gregorian).
    pub year: i32,
    /// Month 1–12.
    pub month: u8,
    /// Day 1–31 (checked against the month).
    pub day: u8,
    /// Hour 0–23.
    pub hour: u8,
    /// Minute 0–59.
    pub minute: u8,
    /// Second 0–59 (a leap-second label `60` is rejected; see [`CivilDateTime::new`]).
    pub second: u8,
    /// Fraction of the second, ns.
    pub nanosecond: u32,
    /// Offset from UTC in minutes (east positive), if the file states one.
    pub utc_offset_minutes: Option<i16>,
}

/// Errors for calendar times.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CivilTimeError {
    /// A component is outside its range (or the day does not exist in that month).
    #[error("invalid {0}")]
    Invalid(&'static str),
    /// The file gives no offset from UTC, so the instant is not known.
    #[error("no offset from UTC: the instant is not known")]
    NoUtcOffset,
    /// The time is outside the leap-second table (before 1972).
    #[error("time outside the leap-second table")]
    OutsideTable,
}

impl CivilDateTime {
    /// Builds a checked calendar time.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        year: i32,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        nanosecond: u32,
        utc_offset_minutes: Option<i16>,
    ) -> Result<Self, CivilTimeError> {
        if !(1..=12).contains(&month) {
            return Err(CivilTimeError::Invalid("month"));
        }
        if day == 0 || day > days_in_month(year, month) {
            return Err(CivilTimeError::Invalid("day"));
        }
        if hour > 23 {
            return Err(CivilTimeError::Invalid("hour"));
        }
        if minute > 59 {
            return Err(CivilTimeError::Invalid("minute"));
        }
        if second > 59 {
            return Err(CivilTimeError::Invalid("second"));
        }
        if nanosecond >= 1_000_000_000 {
            return Err(CivilTimeError::Invalid("fraction of second"));
        }
        if let Some(off) = utc_offset_minutes {
            // ISO 8601 / HL7 / DICOM offsets lie within −14:00..+14:00.
            if !(-14 * 60..=14 * 60).contains(&off) {
                return Err(CivilTimeError::Invalid("UTC offset"));
            }
        }
        Ok(Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
            nanosecond,
            utc_offset_minutes,
        })
    }

    /// Unix nanoseconds of this time read as UTC after removing the stated offset.
    pub fn to_unix_ns(&self) -> Result<i64, CivilTimeError> {
        let offset = self.utc_offset_minutes.ok_or(CivilTimeError::NoUtcOffset)?;
        let days = days_from_civil(self.year, self.month, self.day);
        let secs = days * S_PER_DAY
            + i64::from(self.hour) * 3_600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
            - i64::from(offset) * 60;
        secs.checked_mul(NS_PER_S)
            .and_then(|ns| ns.checked_add(i64::from(self.nanosecond)))
            .ok_or(CivilTimeError::Invalid("year"))
    }

    /// The instant as TAI. Fails when the file stated no UTC offset (nothing is assumed).
    pub fn to_tai(&self, leap: &LeapSeconds) -> Result<(TaiNs, Validity), CivilTimeError> {
        let unix_ns = self.to_unix_ns()?;
        leap.utc_to_tai(unix_ns)
            .map_err(|_| CivilTimeError::OutsideTable)
    }

    /// The same wall-clock reading with the offset dropped (for formats that store local time
    /// only, such as EDF).
    pub fn local(self) -> Self {
        Self {
            utc_offset_minutes: None,
            ..self
        }
    }
}

impl fmt::Display for CivilDateTime {
    /// ISO 8601 (`2026-10-10T08:15:30.250Z`, `…+05:30`, or no designator for local time).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )?;
        if self.nanosecond != 0 {
            let frac = format!("{:09}", self.nanosecond);
            write!(f, ".{}", frac.trim_end_matches('0'))?;
        }
        match self.utc_offset_minutes {
            None => Ok(()),
            Some(0) => f.write_str("Z"),
            Some(off) => {
                let sign = if off < 0 { '-' } else { '+' };
                let a = off.unsigned_abs();
                write!(f, "{sign}{:02}:{:02}", a / 60, a % 60)
            }
        }
    }
}

/// Days in a month of the proleptic Gregorian calendar.
pub(crate) fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01 (H. Hinnant's `days_from_civil`).
pub(crate) fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Year, month, day of a day count since 1970-01-01 (inverse of [`days_from_civil`]).
pub(crate) fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    // d in 1..=31 and m in 1..=12 by construction.
    (y, m as u8, d as u8)
}

/// ISO 8601 UTC text for a [`Utc`] instant, `23:59:60` during an inserted leap second, with
/// nanosecond precision trimmed of trailing zeros (`2026-10-10T08:15:30.25Z`).
pub fn format_utc(utc: Utc) -> String {
    let secs = utc.unix_ns.div_euclid(NS_PER_S);
    let frac = utc.unix_ns.rem_euclid(NS_PER_S);
    let (y, m, d) = civil_from_days(secs.div_euclid(S_PER_DAY));
    let sod = secs.rem_euclid(S_PER_DAY);
    let second = if utc.leap_second { 60 } else { sod % 60 };
    let mut out = format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{second:02}",
        sod / 3_600,
        (sod % 3_600) / 60
    );
    if frac != 0 {
        let f = format!("{frac:09}");
        out.push('.');
        out.push_str(f.trim_end_matches('0'));
    }
    out.push('Z');
    out
}

/// One sampled channel of a recording (an ECG lead, or any EDF signal), in physical units.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveformChannel {
    /// Lead or signal label as the file states it (`I`, `II`, `V1`, `ECG Lead aVR`, …).
    pub label: String,
    /// Physical unit as the file states it (`uV`, `mV`, …).
    pub unit: String,
    /// Sampling frequency, Hz.
    pub sample_rate_hz: f64,
    /// Samples in `unit`.
    pub samples: Vec<f64>,
}

/// Errors converting channel units.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChannelUnitError {
    /// The unit is not a voltage CAELUM converts (only `uV`, `mV`, `V` and their spellings).
    #[error("unit {0:?} is not a supported voltage unit")]
    NotAVoltage(String),
}

impl WaveformChannel {
    /// Duration covered by the samples, s.
    pub fn duration_s(&self) -> f64 {
        self.samples.len() as f64 / self.sample_rate_hz
    }

    /// Samples in microvolts. Only the listed voltage spellings are converted; anything else is
    /// an error, never a pass-through (R-CLN-02).
    pub fn microvolts(&self) -> Result<Vec<f64>, ChannelUnitError> {
        let factor = match self.unit.trim() {
            "uV" | "µV" | "μV" | "microvolt" => 1.0,
            "mV" | "millivolt" => 1_000.0,
            "V" | "volt" => 1_000_000.0,
            other => return Err(ChannelUnitError::NotAVoltage(other.to_owned())),
        };
        Ok(self.samples.iter().map(|s| s * factor).collect())
    }
}

/// Little-endian cursor over a byte slice for the binary formats (EDF, SCP-ECG).
pub(crate) struct Bytes<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bytes<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub(crate) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let out = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(out)
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    pub(crate) fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_days_round_trip() {
        for days in [-1_000_000i64, -1, 0, 1, 365, 10_957, 20_736, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y as i32, m, d), days);
        }
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }

    #[test]
    fn civil_time_is_checked_and_formatted() {
        assert!(CivilDateTime::new(2023, 2, 29, 0, 0, 0, 0, None).is_err());
        assert!(CivilDateTime::new(2024, 2, 29, 0, 0, 0, 0, None).is_ok());
        assert!(CivilDateTime::new(2024, 1, 1, 0, 0, 60, 0, None).is_err());
        let t = CivilDateTime::new(2026, 10, 10, 8, 15, 30, 250_000_000, Some(330)).unwrap();
        assert_eq!(t.to_string(), "2026-10-10T08:15:30.25+05:30");
        assert_eq!(t.local().to_string(), "2026-10-10T08:15:30.25");
        assert_eq!(
            t.to_unix_ns().unwrap(),
            (days_from_civil(2026, 10, 10) * 86_400 + 8 * 3_600 + 15 * 60 + 30 - 330 * 60)
                * 1_000_000_000
                + 250_000_000
        );
        assert_eq!(t.local().to_unix_ns(), Err(CivilTimeError::NoUtcOffset));
    }

    #[test]
    fn utc_formatting_includes_leap_second() {
        let utc = Utc {
            unix_ns: 1_483_228_799 * NS_PER_S,
            leap_second: true,
        };
        assert_eq!(format_utc(utc), "2016-12-31T23:59:60Z");
        let utc = Utc {
            unix_ns: 1_500_000_000 * NS_PER_S + 5_000_000,
            leap_second: false,
        };
        assert_eq!(format_utc(utc), "2017-07-14T02:40:00.005Z");
    }

    #[test]
    fn microvolt_conversion_is_explicit() {
        let mut ch = WaveformChannel {
            label: "II".into(),
            unit: "mV".into(),
            sample_rate_hz: 500.0,
            samples: vec![1.0, -0.5],
        };
        assert_eq!(ch.microvolts().unwrap(), vec![1_000.0, -500.0]);
        ch.unit = "mmHg".into();
        assert!(ch.microvolts().is_err());
        assert!((ch.duration_s() - 0.004).abs() < 1e-12);
    }
}
