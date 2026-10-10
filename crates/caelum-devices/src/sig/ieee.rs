//! Field formats shared by the Bluetooth SIG health profiles: IEEE 11073-20601 SFLOAT and FLOAT,
//! the GATT Date Time, and a bounds-checked little-endian reader.
//!
//! Special values (NaN, NRes, ±INFINITY, reserved) decode to `None`: the reading then has no value
//! and ingest rejects it; nothing is substituted (CLAUDE.md rule 2).

use caelum_domain::time::{LeapSeconds, TaiNs};

use super::DecodeError;

/// Reads little-endian fields, failing on truncated payloads instead of reading past the end.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take<const N: usize>(&mut self, field: &'static str) -> Result<[u8; N], DecodeError> {
        let end = self.pos + N;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or(DecodeError::Truncated {
                field,
                needed: end,
                got: self.bytes.len(),
            })?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        self.pos = end;
        Ok(out)
    }

    pub(crate) fn u8(&mut self, field: &'static str) -> Result<u8, DecodeError> {
        Ok(self.take::<1>(field)?[0])
    }

    pub(crate) fn u16(&mut self, field: &'static str) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take::<2>(field)?))
    }

    pub(crate) fn i16(&mut self, field: &'static str) -> Result<i16, DecodeError> {
        Ok(i16::from_le_bytes(self.take::<2>(field)?))
    }

    pub(crate) fn u24(&mut self, field: &'static str) -> Result<u32, DecodeError> {
        let [a, b, c] = self.take::<3>(field)?;
        Ok(u32::from_le_bytes([a, b, c, 0]))
    }

    pub(crate) fn u32(&mut self, field: &'static str) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take::<4>(field)?))
    }

    pub(crate) fn skip(&mut self, n: usize, field: &'static str) -> Result<(), DecodeError> {
        let end = self.pos + n;
        if end > self.bytes.len() {
            return Err(DecodeError::Truncated {
                field,
                needed: end,
                got: self.bytes.len(),
            });
        }
        self.pos = end;
        Ok(())
    }

    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    pub(crate) fn sfloat(&mut self, field: &'static str) -> Result<Option<f64>, DecodeError> {
        Ok(sfloat(self.u16(field)?))
    }

    pub(crate) fn float(&mut self, field: &'static str) -> Result<Option<f64>, DecodeError> {
        Ok(float(self.u32(field)?))
    }

    /// GATT Date Time (7 bytes). `None` if any date part is 0 ("not known") or out of range.
    pub(crate) fn date_time(
        &mut self,
        leap: &LeapSeconds,
        field: &'static str,
    ) -> Result<Option<TaiNs>, DecodeError> {
        let year = self.u16(field)?;
        let [month, day, hour, minute, second] = self.take::<5>(field)?;
        Ok(civil_to_tai(leap, year, month, day, hour, minute, second))
    }
}

/// `m × 10^e` without the rounding error of multiplying by an inexact power of ten.
fn scaled(mantissa: i32, exponent: i32) -> f64 {
    let m = f64::from(mantissa);
    if exponent >= 0 {
        m * 10f64.powi(exponent)
    } else {
        m / 10f64.powi(-exponent)
    }
}

/// IEEE 11073-20601 16-bit SFLOAT: 4-bit signed exponent, 12-bit signed mantissa.
pub fn sfloat(raw: u16) -> Option<f64> {
    // +INFINITY 0x07FE, NaN 0x07FF, NRes 0x0800, reserved 0x0801, -INFINITY 0x0802.
    if (0x07FE..=0x0802).contains(&raw) {
        return None;
    }
    let mut mantissa = i32::from(raw & 0x0FFF);
    if mantissa >= 0x0800 {
        mantissa -= 0x1000;
    }
    let mut exponent = i32::from(raw >> 12);
    if exponent >= 0x8 {
        exponent -= 0x10;
    }
    Some(scaled(mantissa, exponent))
}

/// IEEE 11073-20601 32-bit FLOAT: 8-bit signed exponent, 24-bit signed mantissa.
pub fn float(raw: u32) -> Option<f64> {
    // +INFINITY 0x007FFFFE, NaN 0x007FFFFF, NRes 0x00800000, reserved 0x00800001, -INFINITY 0x00800002.
    if (0x007F_FFFE..=0x0080_0002).contains(&raw) {
        return None;
    }
    let mut mantissa = i32::try_from(raw & 0x00FF_FFFF).ok()?;
    if mantissa >= 0x0080_0000 {
        mantissa -= 0x0100_0000;
    }
    let mut exponent = i32::try_from(raw >> 24).ok()?;
    if exponent >= 0x80 {
        exponent -= 0x100;
    }
    Some(scaled(mantissa, exponent))
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

/// A device's calendar time as TAI on the device's own clock. The device's time zone and clock
/// error are unknown here; the measured clock offset corrects both (docs/05 §4).
pub(crate) fn civil_to_tai(
    leap: &LeapSeconds,
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
) -> Option<TaiNs> {
    // GATT Date Time: year 1582..=9999, 0 = not known; month/day 0 = not known.
    let valid = (1582..=9999).contains(&year)
        && (1..=12).contains(&month)
        && day >= 1
        && day <= days_in_month(year, month)
        && hour < 24
        && minute < 60
        && second < 60;
    if !valid {
        return None;
    }
    let days = days_from_civil(i64::from(year), i64::from(month), i64::from(day));
    let unix_s =
        days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second);
    let (tai, _validity) = leap.utc_to_tai(unix_s.checked_mul(1_000_000_000)?).ok()?;
    Some(tai)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sfloat_decodes_values_and_refuses_special_values() {
        assert_eq!(sfloat(0x0048), Some(72.0));
        assert_eq!(sfloat(0xF3CF), Some(97.5)); // 975 × 10^-1
        assert_eq!(sfloat(0xFFFF), Some(-0.1)); // mantissa -1, exponent -1
        assert_eq!(sfloat(0x2001), Some(100.0));
        for special in [0x07FF, 0x0800, 0x07FE, 0x0802, 0x0801] {
            assert_eq!(sfloat(special), None, "{special:#06x}");
        }
    }

    #[test]
    fn float_decodes_values_and_refuses_special_values() {
        assert_eq!(float(0xFF00_0172), Some(37.0)); // 370 × 10^-1
        assert_eq!(float(0xFE00_0E74), Some(37.0)); // 3700 × 10^-2
        assert_eq!(float(0xFD00_90A5), Some(37.029)); // 37029 × 10^-3
        assert_eq!(float(0x00FF_FFFF), Some(-1.0)); // negative mantissa
        for special in [
            0x007F_FFFF,
            0x0080_0000,
            0x007F_FFFE,
            0x0080_0002,
            0x0080_0001,
        ] {
            assert_eq!(float(special), None, "{special:#010x}");
        }
    }

    #[test]
    fn date_time_converts_and_rejects_unknown_or_invalid_parts() {
        let leap = LeapSeconds::bundled().unwrap();
        let t = civil_to_tai(&leap, 2017, 1, 1, 0, 0, 0).unwrap();
        // 2017-01-01T00:00:00 UTC = Unix 1483228800, TAI−UTC 37 s, 1958→1970 = 378691200 s.
        assert_eq!(t, TaiNs((1_483_228_800 + 37 + 378_691_200) * 1_000_000_000));
        assert_eq!(
            civil_to_tai(&leap, 0, 1, 1, 0, 0, 0),
            None,
            "year not known"
        );
        assert_eq!(
            civil_to_tai(&leap, 2026, 0, 1, 0, 0, 0),
            None,
            "month not known"
        );
        assert_eq!(
            civil_to_tai(&leap, 2026, 2, 29, 0, 0, 0),
            None,
            "not a leap year"
        );
        assert!(civil_to_tai(&leap, 2024, 2, 29, 0, 0, 0).is_some());
        assert_eq!(civil_to_tai(&leap, 2026, 1, 1, 24, 0, 0), None);
    }

    #[test]
    fn reader_refuses_truncated_fields() {
        let mut r = Reader::new(&[1, 2, 3]);
        assert_eq!(r.u16("a").unwrap(), 0x0201);
        assert!(matches!(
            r.u16("b"),
            Err(DecodeError::Truncated {
                field: "b",
                needed: 4,
                got: 3
            })
        ));
    }
}
