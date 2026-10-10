//! SCP-ECG (EN 1064:2005 / ISO 11073-91064, protocol versions 1.x and 2.x) reader (F-ACQ-07).
//!
//! Supported (CR-014 §3):
//! - record and section CRCs (CRC-CCITT, polynomial 0x1021, initial value 0xFFFF) are verified;
//! - Section 0 (pointers), Section 1 (patient and acquisition data: last and first name, patient
//!   ID, acquisition date, time and time zone), Section 3 (lead definition) and Section 6 (rhythm
//!   data) **without Huffman encoding**, with raw, first-difference or second-difference samples;
//! - Sections 4, 5 and 7–11 (QRS locations, reference beats, measurements, interpretation) are
//!   not read and are listed in [`ScpEcg::ignored_sections`].
//!
//! Refused with an explicit error: Huffman-encoded data (a non-empty Section 2), bimodal
//! compression, reference-beat subtraction, records without Section 6, and protocol version 3.0
//! or later. Lead codes are named only where the code table entry is certain (I, II, III, aVR,
//! aVL, aVF, V1–V6); every other code is kept as its number, never guessed.

use super::{Bytes, CivilDateTime, WaveformChannel};

/// The 16-byte section header.
const SECTION_HEADER: usize = 16;

/// A decoded SCP-ECG record.
#[derive(Debug, Clone, PartialEq)]
pub struct ScpEcg {
    /// Protocol version byte of Section 0 (e.g. 13 = v1.3, 20 = v2.0).
    pub protocol_version: u8,
    /// Patient ID (Section 1 tag 2).
    pub patient_id: Option<String>,
    /// Last name (tag 0).
    pub last_name: Option<String>,
    /// First name (tag 1).
    pub first_name: Option<String>,
    /// Acquisition date and time (tags 25, 26), with the UTC offset from tag 34 if stated.
    pub acquired: Option<CivilDateTime>,
    /// Sampling frequency of the rhythm data, Hz.
    pub sample_rate_hz: f64,
    /// Leads in µV.
    pub leads: Vec<WaveformChannel>,
    /// SCP lead codes (Section 3), parallel to `leads`.
    pub lead_codes: Vec<u8>,
    /// Sections present but not read.
    pub ignored_sections: Vec<u16>,
}

/// Reader errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScpError {
    /// The data ends inside a structure.
    #[error("record truncated in {0}")]
    Truncated(&'static str),
    /// The record length field disagrees with the data.
    #[error("record length {stated} does not match {got} bytes")]
    Length {
        /// Stated.
        stated: usize,
        /// Received.
        got: usize,
    },
    /// A CRC does not match.
    #[error("CRC mismatch in {0}")]
    Crc(String),
    /// A structure violates the standard.
    #[error("invalid {0}")]
    Invalid(&'static str),
    /// A feature that is not supported (see the module documentation).
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    /// A required section is missing.
    #[error("section {0} is missing")]
    MissingSection(u16),
}

/// CRC-CCITT (0x1021, initial 0xFFFF, no reflection, no final XOR).
pub fn crc_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// A section's place in the record.
#[derive(Debug, Clone, Copy)]
struct Pointer {
    id: u16,
    length: usize,
    /// 0-based byte offset in the record.
    offset: usize,
}

/// Parses an SCP-ECG record (F-ACQ-07).
pub fn read(data: &[u8]) -> Result<ScpEcg, ScpError> {
    let mut b = Bytes::new(data);
    let crc = b.u16().ok_or(ScpError::Truncated("record header"))?;
    let stated = b.u32().ok_or(ScpError::Truncated("record header"))? as usize;
    if stated != data.len() {
        return Err(ScpError::Length {
            stated,
            got: data.len(),
        });
    }
    if crc_ccitt(&data[2..]) != crc {
        return Err(ScpError::Crc("record".into()));
    }

    // Section 0 starts right after the record header.
    let s0 = section(data, 6, 0)?;
    let protocol_version = s0.protocol_version;
    if protocol_version >= 30 {
        return Err(ScpError::Unsupported(
            "SCP-ECG protocol version 3.0 or later",
        ));
    }
    if protocol_version < 10 {
        return Err(ScpError::Invalid("protocol version"));
    }
    let mut pointers = Vec::new();
    let mut pb = Bytes::new(s0.body);
    while pb.remaining() >= 10 {
        let id = pb.u16().ok_or(ScpError::Truncated("section 0"))?;
        let length = pb.u32().ok_or(ScpError::Truncated("section 0"))? as usize;
        let index = pb.u32().ok_or(ScpError::Truncated("section 0"))? as usize;
        if length == 0 {
            continue;
        }
        if index == 0 {
            return Err(ScpError::Invalid("section pointer index 0"));
        }
        pointers.push(Pointer {
            id,
            length,
            offset: index - 1,
        });
    }
    let find = |id: u16| pointers.iter().copied().find(|p| p.id == id);
    let load = |p: Pointer| -> Result<Section<'_>, ScpError> {
        let s = section(data, p.offset, p.id)?;
        if s.length != p.length {
            return Err(ScpError::Invalid(
                "section length disagrees with its pointer",
            ));
        }
        Ok(s)
    };

    if find(2).is_some() {
        return Err(ScpError::Unsupported("Huffman-encoded data (section 2)"));
    }
    let s3 = load(find(3).ok_or(ScpError::MissingSection(3))?)?;
    let s6 = load(find(6).ok_or(ScpError::MissingSection(6))?)?;

    let (mut patient_id, mut last_name, mut first_name, mut acquired) = (None, None, None, None);
    if let Some(p) = find(1) {
        let s1 = load(p)?;
        let info = patient(s1.body)?;
        patient_id = info.patient_id;
        last_name = info.last_name;
        first_name = info.first_name;
        acquired = info.acquired;
    }

    // Section 3: lead definition.
    let mut l = Bytes::new(s3.body);
    let n_leads = usize::from(l.u8().ok_or(ScpError::Truncated("section 3"))?);
    let flags = l.u8().ok_or(ScpError::Truncated("section 3"))?;
    if flags & 0x01 != 0 {
        return Err(ScpError::Unsupported("reference beat subtraction"));
    }
    if n_leads == 0 {
        return Err(ScpError::Invalid("section 3: no leads"));
    }
    let mut lead_defs = Vec::with_capacity(n_leads);
    for _ in 0..n_leads {
        let start = l.u32().ok_or(ScpError::Truncated("section 3"))?;
        let end = l.u32().ok_or(ScpError::Truncated("section 3"))?;
        let code = l.u8().ok_or(ScpError::Truncated("section 3"))?;
        if start == 0 || end < start {
            return Err(ScpError::Invalid("section 3: lead sample range"));
        }
        lead_defs.push((code, (end - start + 1) as usize));
    }

    // Section 6: rhythm data.
    let mut r = Bytes::new(s6.body);
    let avm_nv = r.u16().ok_or(ScpError::Truncated("section 6"))?;
    let interval_us = r.u16().ok_or(ScpError::Truncated("section 6"))?;
    let difference = r.u8().ok_or(ScpError::Truncated("section 6"))?;
    let bimodal = r.u8().ok_or(ScpError::Truncated("section 6"))?;
    if bimodal != 0 {
        return Err(ScpError::Unsupported("bimodal compression"));
    }
    if avm_nv == 0 || interval_us == 0 {
        return Err(ScpError::Invalid(
            "section 6: amplitude multiplier or sample interval 0",
        ));
    }
    if difference > 2 {
        return Err(ScpError::Invalid("section 6: difference encoding"));
    }
    let lengths: Vec<usize> = (0..n_leads)
        .map(|_| r.u16().map(usize::from))
        .collect::<Option<_>>()
        .ok_or(ScpError::Truncated("section 6"))?;
    let sample_rate_hz = 1_000_000.0 / f64::from(interval_us);
    let mut leads = Vec::with_capacity(n_leads);
    let mut lead_codes = Vec::with_capacity(n_leads);
    for ((code, n_samples), bytes) in lead_defs.iter().zip(&lengths) {
        if bytes % 2 != 0 || bytes / 2 != *n_samples {
            return Err(ScpError::Invalid(
                "section 6: lead data length disagrees with section 3",
            ));
        }
        let raw = r
            .take(*bytes)
            .ok_or(ScpError::Truncated("section 6 data"))?;
        let d: Vec<i64> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i64::from(i16::from_le_bytes([c[0], c[1]])))
            .collect();
        let values = undifference(&d, difference);
        let scale = f64::from(avm_nv) / 1_000.0;
        leads.push(WaveformChannel {
            label: lead_name(*code),
            unit: "uV".to_owned(),
            sample_rate_hz,
            samples: values.iter().map(|v| *v as f64 * scale).collect(),
        });
        lead_codes.push(*code);
    }

    let mut ignored_sections: Vec<u16> = pointers
        .iter()
        .map(|p| p.id)
        .filter(|id| !matches!(id, 0 | 1 | 3 | 6))
        .collect();
    ignored_sections.sort_unstable();
    ignored_sections.dedup();
    Ok(ScpEcg {
        protocol_version,
        patient_id,
        last_name,
        first_name,
        acquired,
        sample_rate_hz,
        leads,
        lead_codes,
        ignored_sections,
    })
}

/// Reverses first (`X(n) = D(n) + X(n−1)`) or second (`X(n) = D(n) + 2X(n−1) − X(n−2)`)
/// differences; the first one or two samples are stored as they are.
fn undifference(d: &[i64], order: u8) -> Vec<i64> {
    let mut x: Vec<i64> = Vec::with_capacity(d.len());
    for (n, v) in d.iter().enumerate() {
        let value = match (order, n) {
            (1, n) if n >= 1 => v + x[n - 1],
            (2, n) if n >= 2 => v + 2 * x[n - 1] - x[n - 2],
            _ => *v,
        };
        x.push(value);
    }
    x
}

/// Lead names for the codes whose table entries are certain; others keep their number.
pub fn lead_name(code: u8) -> String {
    match code {
        1 => "I".to_owned(),
        2 => "II".to_owned(),
        3..=8 => format!("V{}", code - 2),
        61 => "III".to_owned(),
        62 => "aVR".to_owned(),
        63 => "aVL".to_owned(),
        64 => "aVF".to_owned(),
        other => format!("SCP lead {other}"),
    }
}

struct Section<'a> {
    length: usize,
    protocol_version: u8,
    body: &'a [u8],
}

fn section(data: &[u8], offset: usize, expected_id: u16) -> Result<Section<'_>, ScpError> {
    let header = data
        .get(offset..offset + SECTION_HEADER)
        .ok_or(ScpError::Truncated("section header"))?;
    let mut h = Bytes::new(header);
    let crc = h.u16().ok_or(ScpError::Truncated("section header"))?;
    let id = h.u16().ok_or(ScpError::Truncated("section header"))?;
    let length = h.u32().ok_or(ScpError::Truncated("section header"))? as usize;
    let _section_version = h.u8();
    let protocol_version = h.u8().ok_or(ScpError::Truncated("section header"))?;
    if id != expected_id {
        return Err(ScpError::Invalid("section ID disagrees with its pointer"));
    }
    if length < SECTION_HEADER {
        return Err(ScpError::Invalid("section length"));
    }
    let end = offset
        .checked_add(length)
        .ok_or(ScpError::Invalid("section length"))?;
    let whole = data
        .get(offset..end)
        .ok_or(ScpError::Truncated("section"))?;
    if crc_ccitt(&whole[2..]) != crc {
        return Err(ScpError::Crc(format!("section {id}")));
    }
    Ok(Section {
        length,
        protocol_version,
        body: &whole[SECTION_HEADER..],
    })
}

struct PatientInfo {
    patient_id: Option<String>,
    last_name: Option<String>,
    first_name: Option<String>,
    acquired: Option<CivilDateTime>,
}

/// Section 1 tags. Text is kept only when it is valid UTF-8 (ASCII included); other character
/// sets are not guessed.
fn patient(body: &[u8]) -> Result<PatientInfo, ScpError> {
    let mut b = Bytes::new(body);
    let mut info = PatientInfo {
        patient_id: None,
        last_name: None,
        first_name: None,
        acquired: None,
    };
    let (mut date, mut time, mut offset) = (None, None, None);
    while b.remaining() >= 3 {
        let tag = b.u8().ok_or(ScpError::Truncated("section 1"))?;
        let len = usize::from(b.u16().ok_or(ScpError::Truncated("section 1"))?);
        if tag == 255 {
            break;
        }
        let v = b.take(len).ok_or(ScpError::Truncated("section 1 tag"))?;
        match tag {
            0 => info.last_name = text(v),
            1 => info.first_name = text(v),
            2 => info.patient_id = text(v),
            25 if len >= 4 => date = Some((u16::from_le_bytes([v[0], v[1]]), v[2], v[3])),
            26 if len >= 3 => time = Some((v[0], v[1], v[2])),
            34 if len >= 2 => {
                let o = i16::from_le_bytes([v[0], v[1]]);
                if o != 0x7FFF {
                    offset = Some(o);
                }
            }
            _ => {}
        }
    }
    if let (Some((y, mo, d)), Some((h, mi, s))) = (date, time) {
        info.acquired = CivilDateTime::new(i32::from(y), mo, d, h, mi, s, 0, offset).ok();
    }
    Ok(info)
}

fn text(v: &[u8]) -> Option<String> {
    let end = v.iter().position(|b| *b == 0).unwrap_or(v.len());
    let s = std::str::from_utf8(&v[..end]).ok()?.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_ccitt_check_value() {
        assert_eq!(crc_ccitt(b"123456789"), 0x29B1);
    }

    #[test]
    fn differences_are_reversed() {
        // x = 10, 12, 15, 15, 11
        assert_eq!(
            undifference(&[10, 2, 3, 0, -4], 1),
            vec![10, 12, 15, 15, 11]
        );
        // second differences of the same series: 10, 12, then 1, -3, -4
        assert_eq!(
            undifference(&[10, 12, 1, -3, -4], 2),
            vec![10, 12, 15, 15, 11]
        );
        assert_eq!(undifference(&[1, 2], 0), vec![1, 2]);
    }

    #[test]
    fn only_certain_lead_codes_are_named() {
        assert_eq!(lead_name(1), "I");
        assert_eq!(lead_name(8), "V6");
        assert_eq!(lead_name(64), "aVF");
        assert_eq!(lead_name(9), "SCP lead 9");
    }
}
