//! EDF and EDF+ (European Data Format): reader for ECG and other recordings (F-ACQ-07) and EDF+
//! writer for waveform export (F-SYS-08).
//!
//! References: Kemp et al., "A simple format for exchange of digitized polygraphic recordings"
//! (EDF, 1992) and Kemp & Olivan, "European data format 'plus' (EDF+)" (2003), as published at
//! edfplus.info.
//!
//! Supported: EDF, EDF+C (continuous) and EDF+D (discontinuous), 16-bit samples, any number of
//! signals with their own sample rates, the "EDF Annotations" signal (time-stamped annotation
//! lists, TALs). BDF (24-bit, version byte 0xFF) is refused.
//!
//! The reader is strict: header numbers must be plain decimal ASCII, the stated header size and
//! record count must match the data exactly, every EDF+ data record must start with its
//! time-keeping annotation, and EDF+C records must be contiguous. Samples outside the stated
//! digital range are kept (EDF+ uses them to mark clipping) and counted per signal.
//!
//! Times: EDF stores local wall-clock time with no offset from UTC, so [`EdfFile::start`] carries
//! no offset; the caller supplies one only if it knows it (nothing is assumed).

use super::{days_in_month, Bytes, CivilDateTime, WaveformChannel};

/// The label of the EDF+ annotations signal.
pub const ANNOTATIONS_LABEL: &str = "EDF Annotations";

const SIGNAL_HEADER: usize = 256;
/// Tolerance for record onsets of EDF+C files (s): onsets are decimal text.
const ONSET_TOLERANCE_S: f64 = 1e-6;
const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// EDF flavour, from the header's reserved field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdfVariant {
    /// Original EDF.
    Edf,
    /// EDF+ with contiguous data records.
    EdfPlusContinuous,
    /// EDF+ with data records that may have gaps between them.
    EdfPlusDiscontinuous,
}

impl EdfVariant {
    /// Whether this is an EDF+ variant.
    pub fn is_plus(self) -> bool {
        !matches!(self, Self::Edf)
    }
}

/// One ordinary signal, scaled to physical units.
#[derive(Debug, Clone, PartialEq)]
pub struct EdfSignal {
    /// Label (`ECG I`, `EEG Fpz-Cz`, …).
    pub label: String,
    /// Transducer type.
    pub transducer: String,
    /// Physical dimension (`uV`, `mV`, …), as written.
    pub physical_dimension: String,
    /// Physical value of `digital_min`.
    pub physical_min: f64,
    /// Physical value of `digital_max`.
    pub physical_max: f64,
    /// Lowest digital value.
    pub digital_min: i32,
    /// Highest digital value.
    pub digital_max: i32,
    /// Prefiltering text (`HP:0.1Hz LP:75Hz`, …).
    pub prefiltering: String,
    /// Samples in each data record.
    pub samples_per_record: usize,
    /// Sampling frequency, Hz (`samples_per_record / record duration`).
    pub sample_rate_hz: f64,
    /// All samples in physical units, records concatenated.
    pub samples: Vec<f64>,
    /// Samples whose digital value lay outside `[digital_min, digital_max]` (clipping marks).
    pub out_of_range_samples: usize,
}

impl EdfSignal {
    /// The signal as a waveform channel (label, dimension, rate, samples).
    pub fn to_channel(&self) -> WaveformChannel {
        WaveformChannel {
            label: self.label.clone(),
            unit: self.physical_dimension.clone(),
            sample_rate_hz: self.sample_rate_hz,
            samples: self.samples.clone(),
        }
    }
}

/// One EDF+ annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct EdfAnnotation {
    /// Seconds after the file start time.
    pub onset_s: f64,
    /// Duration, s, if stated.
    pub duration_s: Option<f64>,
    /// The annotation text (UTF-8).
    pub text: String,
}

/// A decoded EDF or EDF+ file.
#[derive(Debug, Clone, PartialEq)]
pub struct EdfFile {
    /// EDF, EDF+C or EDF+D.
    pub variant: EdfVariant,
    /// Local patient identification field (EDF+: `code sex birthdate name`), trimmed.
    pub patient: String,
    /// Local recording identification field (EDF+: `Startdate dd-MMM-yyyy …`), trimmed.
    pub recording: String,
    /// Start date and time of the recording (local time, whole seconds, no UTC offset).
    pub start: CivilDateTime,
    /// Duration of one data record, s.
    pub record_duration_s: f64,
    /// Start of each data record, s after `start` (EDF: `i × duration`; EDF+: time-keeping TALs).
    pub record_onsets_s: Vec<f64>,
    /// Ordinary signals (annotation signals excluded).
    pub signals: Vec<EdfSignal>,
    /// EDF+ annotations, in file order (time-keeping entries excluded).
    pub annotations: Vec<EdfAnnotation>,
}

impl EdfFile {
    /// EDF+ patient subfields (`code`, `sex`, `birthdate`, `name`); `X` (unknown) becomes `None`.
    /// `None` for plain EDF or a field that does not follow EDF+.
    pub fn plus_patient(&self) -> Option<[Option<String>; 4]> {
        if !self.variant.is_plus() {
            return None;
        }
        let parts: Vec<&str> = self.patient.split(' ').collect();
        if parts.len() < 4 {
            return None;
        }
        let f = |s: &str| (s != "X").then(|| s.replace('_', " "));
        Some([f(parts[0]), f(parts[1]), f(parts[2]), f(parts[3])])
    }
}

/// Reader errors. A file that fails any check yields nothing.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EdfError {
    /// The file ends inside the header or a data record.
    #[error("file truncated in {0}")]
    Truncated(&'static str),
    /// Header text outside printable US-ASCII.
    #[error("header field {0} is not printable ASCII")]
    NotAscii(&'static str),
    /// A numeric header field is not a plain decimal number.
    #[error("header field {field} is not a valid number: {text:?}")]
    InvalidNumber {
        /// The field.
        field: &'static str,
        /// Its text.
        text: String,
    },
    /// The version field is not `0` (BDF and other variants are not supported).
    #[error("unsupported version field {0:?} (only EDF/EDF+ version 0)")]
    UnsupportedVersion(String),
    /// A header value is outside what the format allows.
    #[error("invalid header: {0}")]
    InvalidHeader(&'static str),
    /// The stated header size disagrees with the number of signals.
    #[error("header size {stated} does not match {expected} for the number of signals")]
    HeaderSize {
        /// Stated in the header.
        stated: usize,
        /// 256 × (signals + 1).
        expected: usize,
    },
    /// The data section length disagrees with the stated record count.
    #[error("data is {got} bytes, header implies {expected}")]
    DataSize {
        /// From the header.
        expected: usize,
        /// In the file.
        got: usize,
    },
    /// A signal header is invalid.
    #[error("signal {index}: {reason}")]
    InvalidSignal {
        /// Signal index (0-based).
        index: usize,
        /// What is wrong.
        reason: &'static str,
    },
    /// An EDF+ annotation list is malformed.
    #[error("data record {record}: malformed annotation list ({reason})")]
    Annotation {
        /// Data record (0-based).
        record: usize,
        /// What is wrong.
        reason: &'static str,
    },
    /// An EDF+C file whose records are not contiguous, or an EDF+D file whose records go back.
    #[error("data record {0} does not follow the previous record in time")]
    RecordOrder(usize),
}

/// Parses an EDF or EDF+ file (F-ACQ-07).
pub fn read(data: &[u8]) -> Result<EdfFile, EdfError> {
    let mut b = Bytes::new(data);
    let version = ascii(b.take(8), "version")?;
    if version.trim() != "0" {
        return Err(EdfError::UnsupportedVersion(version.trim().to_owned()));
    }
    let patient = ascii(b.take(80), "patient")?.trim().to_owned();
    let recording = ascii(b.take(80), "recording")?.trim().to_owned();
    let start_date = ascii(b.take(8), "start date")?;
    let start_time = ascii(b.take(8), "start time")?;
    let header_bytes = int(b.take(8), "header bytes")?;
    let reserved = ascii(b.take(44), "reserved")?;
    let records = int(b.take(8), "number of data records")?;
    let record_duration_s = num(b.take(8), "duration of a data record")?;
    let ns = int(b.take(4), "number of signals")?;

    let variant = if reserved.starts_with("EDF+C") {
        EdfVariant::EdfPlusContinuous
    } else if reserved.starts_with("EDF+D") {
        EdfVariant::EdfPlusDiscontinuous
    } else {
        EdfVariant::Edf
    };
    if records < 1 {
        return Err(EdfError::InvalidHeader(
            "number of data records must be at least 1 (-1 marks an unfinished recording)",
        ));
    }
    if ns < 1 {
        return Err(EdfError::InvalidHeader("no signals"));
    }
    if record_duration_s.is_nan() || record_duration_s < 0.0 {
        return Err(EdfError::InvalidHeader("negative record duration"));
    }
    let (records, ns) = (records as usize, ns as usize);
    let expected = (ns + 1) * SIGNAL_HEADER;
    if header_bytes < 0 || header_bytes as usize != expected {
        return Err(EdfError::HeaderSize {
            stated: header_bytes.max(0) as usize,
            expected,
        });
    }
    let start = start_datetime(start_date, start_time, &recording, variant)?;

    // Signal headers are stored field by field across all signals.
    let labels = fields(&mut b, ns, 16, "label")?;
    let transducers = fields(&mut b, ns, 80, "transducer type")?;
    let dims = fields(&mut b, ns, 8, "physical dimension")?;
    let pmins = fields(&mut b, ns, 8, "physical minimum")?;
    let pmaxs = fields(&mut b, ns, 8, "physical maximum")?;
    let dmins = fields(&mut b, ns, 8, "digital minimum")?;
    let dmaxs = fields(&mut b, ns, 8, "digital maximum")?;
    let prefilters = fields(&mut b, ns, 80, "prefiltering")?;
    let sprs = fields(&mut b, ns, 8, "samples per record")?;
    let _reserved = fields(&mut b, ns, 32, "signal reserved")?;

    let mut headers = Vec::with_capacity(ns);
    for i in 0..ns {
        let annotation = labels[i] == ANNOTATIONS_LABEL;
        if annotation && !variant.is_plus() {
            return Err(EdfError::InvalidSignal {
                index: i,
                reason: "annotation signal in a file that is not EDF+",
            });
        }
        let spr = parse_int(sprs[i], "samples per record")?;
        if spr < 1 {
            return Err(EdfError::InvalidSignal {
                index: i,
                reason: "samples per record must be at least 1",
            });
        }
        let pmin = parse_num(pmins[i], "physical minimum")?;
        let pmax = parse_num(pmaxs[i], "physical maximum")?;
        let dmin = parse_int(dmins[i], "digital minimum")?;
        let dmax = parse_int(dmaxs[i], "digital maximum")?;
        if !annotation {
            if pmin == pmax {
                return Err(EdfError::InvalidSignal {
                    index: i,
                    reason: "physical minimum equals physical maximum",
                });
            }
            if dmin >= dmax || dmin < i64::from(i16::MIN) || dmax > i64::from(i16::MAX) {
                return Err(EdfError::InvalidSignal {
                    index: i,
                    reason: "digital range must be increasing and within 16 bits",
                });
            }
            if record_duration_s == 0.0 {
                return Err(EdfError::InvalidSignal {
                    index: i,
                    reason: "record duration 0 is only allowed for annotation-only files",
                });
            }
        }
        headers.push(SignalHeader {
            annotation,
            spr: spr as usize,
            pmin,
            pmax,
            dmin,
            dmax,
        });
    }
    if variant.is_plus() && !headers.iter().any(|h| h.annotation) {
        return Err(EdfError::InvalidHeader(
            "EDF+ file without an annotation signal",
        ));
    }

    let record_bytes: usize = headers.iter().map(|h| h.spr * 2).sum();
    let data_len = records
        .checked_mul(record_bytes)
        .ok_or(EdfError::InvalidHeader("data size overflows"))?;
    let body = &data[expected.min(data.len())..];
    if body.len() != data_len {
        return Err(EdfError::DataSize {
            expected: data_len,
            got: body.len(),
        });
    }

    let mut signals: Vec<EdfSignal> = Vec::new();
    let mut signal_slot = Vec::with_capacity(ns);
    for (i, h) in headers.iter().enumerate() {
        if h.annotation {
            signal_slot.push(None);
            continue;
        }
        signal_slot.push(Some(signals.len()));
        signals.push(EdfSignal {
            label: labels[i].to_owned(),
            transducer: transducers[i].to_owned(),
            physical_dimension: dims[i].to_owned(),
            physical_min: h.pmin,
            physical_max: h.pmax,
            digital_min: h.dmin as i32,
            digital_max: h.dmax as i32,
            prefiltering: prefilters[i].to_owned(),
            samples_per_record: h.spr,
            sample_rate_hz: h.spr as f64 / record_duration_s,
            samples: Vec::with_capacity(h.spr * records),
            out_of_range_samples: 0,
        });
    }

    let mut annotations = Vec::new();
    let mut record_onsets_s: Vec<f64> = Vec::with_capacity(records);
    let mut rb = Bytes::new(body);
    for r in 0..records {
        let mut timekeeping: Option<f64> = None;
        let mut first_annotation_signal = true;
        for (i, h) in headers.iter().enumerate() {
            let raw = rb
                .take(h.spr * 2)
                .ok_or(EdfError::Truncated("data record"))?;
            if h.annotation {
                let onset = parse_tals(raw, r, first_annotation_signal, &mut annotations)?;
                if first_annotation_signal {
                    timekeeping = onset;
                }
                first_annotation_signal = false;
                continue;
            }
            if let Some(Some(slot)) = signal_slot.get(i) {
                let sig = &mut signals[*slot];
                let gain = (h.pmax - h.pmin) / (h.dmax - h.dmin) as f64;
                for pair in raw.as_chunks::<2>().0 {
                    let d = i64::from(i16::from_le_bytes([pair[0], pair[1]]));
                    if d < h.dmin || d > h.dmax {
                        sig.out_of_range_samples += 1;
                    }
                    sig.samples.push(h.pmin + (d - h.dmin) as f64 * gain);
                }
            }
        }
        let onset = if variant.is_plus() {
            timekeeping.ok_or(EdfError::Annotation {
                record: r,
                reason: "no time-keeping annotation",
            })?
        } else {
            r as f64 * record_duration_s
        };
        if let Some(prev) = record_onsets_s.last().copied() {
            let ok = match variant {
                EdfVariant::Edf => true,
                EdfVariant::EdfPlusContinuous => {
                    (onset - (prev + record_duration_s)).abs() <= ONSET_TOLERANCE_S
                }
                EdfVariant::EdfPlusDiscontinuous => {
                    onset >= prev + record_duration_s - ONSET_TOLERANCE_S
                }
            };
            if !ok {
                return Err(EdfError::RecordOrder(r));
            }
        }
        record_onsets_s.push(onset);
    }

    Ok(EdfFile {
        variant,
        patient,
        recording,
        start,
        record_duration_s,
        record_onsets_s,
        signals,
        annotations,
    })
}

struct SignalHeader {
    annotation: bool,
    spr: usize,
    pmin: f64,
    pmax: f64,
    dmin: i64,
    dmax: i64,
}

fn ascii<'a>(raw: Option<&'a [u8]>, field: &'static str) -> Result<&'a str, EdfError> {
    let raw = raw.ok_or(EdfError::Truncated(field))?;
    if !raw.iter().all(|c| (0x20..=0x7E).contains(c)) {
        return Err(EdfError::NotAscii(field));
    }
    std::str::from_utf8(raw).map_err(|_| EdfError::NotAscii(field))
}

fn fields<'a>(
    b: &mut Bytes<'a>,
    n: usize,
    width: usize,
    field: &'static str,
) -> Result<Vec<&'a str>, EdfError> {
    (0..n)
        .map(|_| ascii(b.take(width), field).map(str::trim))
        .collect()
}

fn plain_decimal(text: &str) -> bool {
    let t = text.strip_prefix(['+', '-']).unwrap_or(text);
    !t.is_empty()
        && t.chars().all(|c| c.is_ascii_digit() || c == '.')
        && t.chars().filter(|c| *c == '.').count() <= 1
        && t.chars().any(|c| c.is_ascii_digit())
}

fn parse_num(text: &str, field: &'static str) -> Result<f64, EdfError> {
    let t = text.trim();
    let bad = || EdfError::InvalidNumber {
        field,
        text: t.to_owned(),
    };
    if !plain_decimal(t) {
        return Err(bad());
    }
    t.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(bad)
}

fn parse_int(text: &str, field: &'static str) -> Result<i64, EdfError> {
    let t = text.trim();
    t.parse::<i64>().map_err(|_| EdfError::InvalidNumber {
        field,
        text: t.to_owned(),
    })
}

fn num(raw: Option<&[u8]>, field: &'static str) -> Result<f64, EdfError> {
    parse_num(ascii(raw, field)?, field)
}

fn int(raw: Option<&[u8]>, field: &'static str) -> Result<i64, EdfError> {
    parse_int(ascii(raw, field)?, field)
}

fn two_digits(s: &str, field: &'static str) -> Result<u8, EdfError> {
    if s.len() == 2 && s.bytes().all(|c| c.is_ascii_digit()) {
        s.parse::<u8>().map_err(|_| EdfError::InvalidHeader(field))
    } else {
        Err(EdfError::InvalidHeader(field))
    }
}

/// Header date `dd.mm.yy` and time `hh.mm.ss`. Years 85–99 are 1985–1999, 00–84 are 2000–2084;
/// EDF+ files carry the full year in the recording field (`Startdate dd-MMM-yyyy`), which must
/// agree, and is used when the header year is `yy` (after 2084).
fn start_datetime(
    date: &str,
    time: &str,
    recording: &str,
    variant: EdfVariant,
) -> Result<CivilDateTime, EdfError> {
    let d: Vec<&str> = date.split('.').collect();
    let t: Vec<&str> = time.split('.').collect();
    if d.len() != 3 || t.len() != 3 {
        return Err(EdfError::InvalidHeader("start date or time format"));
    }
    let day = two_digits(d[0], "start day")?;
    let month = two_digits(d[1], "start month")?;
    let plus_date = if variant.is_plus() {
        plus_startdate(recording)
    } else {
        None
    };
    let year = if d[2] == "yy" {
        match plus_date {
            Some((y, _, _)) => y,
            None => {
                return Err(EdfError::InvalidHeader(
                    "start year 'yy' without EDF+ Startdate",
                ))
            }
        }
    } else {
        let yy = i32::from(two_digits(d[2], "start year")?);
        let year = if yy >= 85 { 1900 + yy } else { 2000 + yy };
        if let Some((y, m, dd)) = plus_date {
            if y.rem_euclid(100) != yy || m != month || dd != day {
                return Err(EdfError::InvalidHeader(
                    "EDF+ Startdate disagrees with the header start date",
                ));
            }
            y
        } else {
            year
        }
    };
    CivilDateTime::new(
        year,
        month,
        day,
        two_digits(t[0], "start hour")?,
        two_digits(t[1], "start minute")?,
        two_digits(t[2], "start second")?,
        0,
        None,
    )
    .map_err(|_| EdfError::InvalidHeader("start date or time out of range"))
}

/// `Startdate dd-MMM-yyyy …` → (year, month, day); `None` if absent or `Startdate X`.
fn plus_startdate(recording: &str) -> Option<(i32, u8, u8)> {
    let mut parts = recording.split(' ');
    if parts.next()? != "Startdate" {
        return None;
    }
    let date = parts.next()?;
    let mut dp = date.split('-');
    let day: u8 = dp.next()?.parse().ok()?;
    let mon = dp.next()?;
    let month = MONTHS.iter().position(|m| *m == mon)? as u8 + 1;
    let year_text = dp.next()?;
    if year_text.len() != 4 {
        return None;
    }
    let year: i32 = year_text.parse().ok()?;
    Some((year, month, day))
}

/// Parses the TALs of one annotation signal in one record; returns the time-keeping onset if
/// `timekeeping` (first annotation signal of the record).
fn parse_tals(
    raw: &[u8],
    record: usize,
    timekeeping: bool,
    out: &mut Vec<EdfAnnotation>,
) -> Result<Option<f64>, EdfError> {
    let err = |reason| EdfError::Annotation { record, reason };
    let mut record_onset = None;
    for (n, tal) in raw.split(|b| *b == 0).filter(|t| !t.is_empty()).enumerate() {
        let sep = tal
            .iter()
            .position(|b| *b == 0x14)
            .ok_or_else(|| err("onset not terminated"))?;
        let (head, rest) = (&tal[..sep], &tal[sep + 1..]);
        let mut hp = head.split(|b| *b == 0x15);
        let onset_text = std::str::from_utf8(hp.next().unwrap_or_default())
            .map_err(|_| err("onset not ASCII"))?;
        if !(onset_text.starts_with('+') || onset_text.starts_with('-')) {
            return Err(err("onset must start with + or -"));
        }
        let onset = parse_num(onset_text, "annotation onset").map_err(|_| err("onset"))?;
        let duration_s = match hp.next() {
            None => None,
            Some(d) => {
                let text = std::str::from_utf8(d).map_err(|_| err("duration not ASCII"))?;
                let v = parse_num(text, "annotation duration").map_err(|_| err("duration"))?;
                if v < 0.0 || text.starts_with(['+', '-']) {
                    return Err(err("duration must be an unsigned number"));
                }
                Some(v)
            }
        };
        if hp.next().is_some() {
            return Err(err("more than one duration"));
        }
        if rest.last() != Some(&0x14) {
            return Err(err("annotation not terminated"));
        }
        let texts: Vec<&[u8]> = rest[..rest.len() - 1].split(|b| *b == 0x14).collect();
        let mut texts = texts.into_iter();
        if timekeeping && n == 0 {
            match texts.next() {
                Some([]) => record_onset = Some(onset),
                _ => return Err(err("first annotation is not the time-keeping annotation")),
            }
        }
        for t in texts {
            if t.is_empty() {
                continue;
            }
            let text = std::str::from_utf8(t).map_err(|_| err("annotation text not UTF-8"))?;
            out.push(EdfAnnotation {
                onset_s: onset,
                duration_s,
                text: text.to_owned(),
            });
        }
    }
    Ok(record_onset)
}

// ---------------------------------------------------------------------------------------------
// Writer (F-SYS-08)
// ---------------------------------------------------------------------------------------------

/// Patient sex as EDF+ records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdfSex {
    /// `F`.
    Female,
    /// `M`.
    Male,
}

/// EDF+ patient identification. Unknown subfields are `None` and written as `X`; a
/// de-identified export leaves them all `None` or uses a pseudonymous code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdfPlusPatient {
    /// Hospital or pseudonymous code.
    pub code: Option<String>,
    /// Sex.
    pub sex: Option<EdfSex>,
    /// Birth date (year, month, day).
    pub birthdate: Option<(i32, u8, u8)>,
    /// Name.
    pub name: Option<String>,
}

/// EDF+ recording identification (after `Startdate dd-MMM-yyyy`). Unknown subfields are `X`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EdfPlusRecordingId {
    /// Hospital administration code of the investigation.
    pub admin_code: Option<String>,
    /// Code of the person responsible.
    pub technician: Option<String>,
    /// Equipment used.
    pub equipment: Option<String>,
}

/// A signal to write, with its encoding range.
#[derive(Debug, Clone, PartialEq)]
pub struct EdfWriteSignal {
    /// Label (max 16 ASCII characters).
    pub label: String,
    /// Transducer type (max 80).
    pub transducer: String,
    /// Physical dimension (max 8), e.g. `uV`.
    pub physical_dimension: String,
    /// Physical value of `digital_min`.
    pub physical_min: f64,
    /// Physical value of `digital_max`.
    pub physical_max: f64,
    /// Lowest digital value.
    pub digital_min: i16,
    /// Highest digital value.
    pub digital_max: i16,
    /// Prefiltering text (max 80).
    pub prefiltering: String,
    /// Samples per data record.
    pub samples_per_record: usize,
    /// Physical samples; the count must be a whole number of records.
    pub samples: Vec<f64>,
}

impl EdfWriteSignal {
    /// A signal using the full 16-bit range, with the physical range fitted to the samples
    /// (rounded outwards to what the 8-character header fields can hold). A constant signal
    /// gets the range `value ± 1` so that it stays encodable; the range is an encoding parameter,
    /// not data.
    pub fn fitted(
        label: &str,
        physical_dimension: &str,
        samples: Vec<f64>,
        samples_per_record: usize,
    ) -> Result<Self, EdfWriteError> {
        if samples.iter().any(|s| !s.is_finite()) {
            return Err(EdfWriteError::NotFinite(label.to_owned()));
        }
        let lo = samples.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = samples.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if !lo.is_finite() {
            return Err(EdfWriteError::NoSamples(label.to_owned()));
        }
        let (lo, hi) = if lo == hi {
            (lo - 1.0, hi + 1.0)
        } else {
            (lo, hi)
        };
        let field = |e| EdfWriteError::Field {
            field: "physical range",
            reason: e,
        };
        let pmin = fit8(lo, Round::Down).ok_or_else(|| field("does not fit 8 characters"))?;
        let pmax = fit8(hi, Round::Up).ok_or_else(|| field("does not fit 8 characters"))?;
        Ok(Self {
            label: label.to_owned(),
            transducer: String::new(),
            physical_dimension: physical_dimension.to_owned(),
            physical_min: pmin,
            physical_max: pmax,
            digital_min: i16::MIN,
            digital_max: i16::MAX,
            prefiltering: String::new(),
            samples_per_record,
            samples,
        })
    }

    /// A signal from a waveform channel with `record_duration_s`-long records (the sample count
    /// per record must be a whole number).
    pub fn from_channel(
        channel: &WaveformChannel,
        record_duration_s: f64,
    ) -> Result<Self, EdfWriteError> {
        let spr = channel.sample_rate_hz * record_duration_s;
        if (spr.is_nan() || spr < 1.0) || (spr - spr.round()).abs() > 1e-9 {
            return Err(EdfWriteError::Field {
                field: "samples per record",
                reason: "sample rate × record duration is not a whole number",
            });
        }
        Self::fitted(
            &channel.label,
            &channel.unit,
            channel.samples.clone(),
            spr.round() as usize,
        )
    }
}

/// An EDF+C file to write.
#[derive(Debug, Clone, PartialEq)]
pub struct EdfPlusWrite {
    /// Patient identification.
    pub patient: EdfPlusPatient,
    /// Recording identification.
    pub recording: EdfPlusRecordingId,
    /// Time of the first sample. EDF has no UTC offset: the wall-clock fields are written as
    /// given and any offset is dropped (the caller chooses UTC or local time and documents it).
    /// A sub-second part is written as the onset of the first record.
    pub start: CivilDateTime,
    /// Duration of a data record, s (must be exactly representable in 8 characters).
    pub record_duration_s: f64,
    /// Ordinary signals (at least one).
    pub signals: Vec<EdfWriteSignal>,
    /// Annotations; `onset_s` is relative to `start`.
    pub annotations: Vec<EdfAnnotation>,
}

/// Writer errors. Nothing is truncated, clipped or padded silently.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EdfWriteError {
    /// A header field does not fit or holds characters EDF does not allow.
    #[error("{field}: {reason}")]
    Field {
        /// The field.
        field: &'static str,
        /// What is wrong.
        reason: &'static str,
    },
    /// No ordinary signal.
    #[error("nothing to write: no signals")]
    NoSignals,
    /// A signal has no samples.
    #[error("signal {0:?} has no samples")]
    NoSamples(String),
    /// A sample is NaN or infinite.
    #[error("signal {0:?} has a sample that is not finite")]
    NotFinite(String),
    /// Signals cover different numbers of records, or a signal ends inside a record.
    #[error("signal {0:?} does not fill a whole number of the common data records")]
    RecordMismatch(String),
    /// A sample lies outside the signal's physical range.
    #[error("signal {label:?}: sample {value} outside physical range [{min}, {max}]")]
    OutOfRange {
        /// The signal.
        label: String,
        /// The sample.
        value: f64,
        /// Physical minimum written.
        min: f64,
        /// Physical maximum written.
        max: f64,
    },
    /// An annotation contains a TAL control character or lies outside the recording.
    #[error("annotation {0:?} is invalid")]
    Annotation(String),
}

/// Writes an EDF+C file (F-SYS-08). The output reopens with [`read`] and with reference EDF+
/// readers (EDFbrowser, MNE, pyedflib).
pub fn write_edf_plus(w: &EdfPlusWrite) -> Result<Vec<u8>, EdfWriteError> {
    if w.signals.is_empty() {
        return Err(EdfWriteError::NoSignals);
    }
    let duration_text = fmt8(w.record_duration_s).ok_or(EdfWriteError::Field {
        field: "record duration",
        reason: "does not fit 8 characters",
    })?;
    if (w.record_duration_s.is_nan() || w.record_duration_s <= 0.0)
        || duration_text.parse::<f64>().ok() != Some(w.record_duration_s)
    {
        return Err(EdfWriteError::Field {
            field: "record duration",
            reason: "must be positive and exactly representable in 8 characters",
        });
    }
    if !(1985..=2084).contains(&w.start.year) {
        return Err(EdfWriteError::Field {
            field: "start date",
            reason: "EDF headers hold years 1985 to 2084",
        });
    }

    // Number of records, common to every signal.
    let mut records: Option<usize> = None;
    for s in &w.signals {
        if s.samples.is_empty() {
            return Err(EdfWriteError::NoSamples(s.label.clone()));
        }
        if s.samples_per_record == 0 || s.samples.len() % s.samples_per_record != 0 {
            return Err(EdfWriteError::RecordMismatch(s.label.clone()));
        }
        let n = s.samples.len() / s.samples_per_record;
        if records.is_some_and(|r| r != n) {
            return Err(EdfWriteError::RecordMismatch(s.label.clone()));
        }
        records = Some(n);
    }
    let records = records.ok_or(EdfWriteError::NoSignals)?;

    // Encode every signal first (validates ranges before anything is written).
    let mut encoded: Vec<(Vec<i16>, String, String)> = Vec::with_capacity(w.signals.len());
    for s in &w.signals {
        encoded.push(encode_signal(s)?);
    }

    // Annotation lists per record; the sub-second start goes into the record onsets.
    let frac = f64::from(w.start.nanosecond) / 1e9;
    let mut tals: Vec<Vec<u8>> = (0..records)
        .map(|r| {
            let mut t = Vec::new();
            t.push(b'+');
            t.extend_from_slice(fmt_seconds(frac + r as f64 * w.record_duration_s).as_bytes());
            t.extend_from_slice(&[0x14, 0x14, 0x00]);
            t
        })
        .collect();
    let total = records as f64 * w.record_duration_s;
    for a in &w.annotations {
        if a.text.is_empty()
            || a.text.bytes().any(|b| matches!(b, 0x00 | 0x14 | 0x15))
            || !a.onset_s.is_finite()
            || a.duration_s
                .is_some_and(|d| (d.is_nan() || d < 0.0) || !d.is_finite())
        {
            return Err(EdfWriteError::Annotation(a.text.clone()));
        }
        let onset = a.onset_s + frac;
        let r = ((a.onset_s / w.record_duration_s).floor().max(0.0) as usize).min(records - 1);
        if a.onset_s > total {
            return Err(EdfWriteError::Annotation(a.text.clone()));
        }
        let t = &mut tals[r];
        t.push(if onset < 0.0 { b'-' } else { b'+' });
        t.extend_from_slice(fmt_seconds(onset.abs()).as_bytes());
        if let Some(d) = a.duration_s {
            t.push(0x15);
            t.extend_from_slice(fmt_seconds(d).as_bytes());
        }
        t.push(0x14);
        t.extend_from_slice(a.text.as_bytes());
        t.extend_from_slice(&[0x14, 0x00]);
    }
    let annot_spr = tals
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .div_ceil(2)
        .max(1);

    let ns = w.signals.len() + 1;
    let mut out = Vec::with_capacity((ns + 1) * 256 + records * 2 * annot_spr);
    put(&mut out, "0", 8, "version")?;
    put(&mut out, &patient_field(&w.patient)?, 80, "patient")?;
    put(&mut out, &recording_field(w)?, 80, "recording")?;
    let s = &w.start;
    put(
        &mut out,
        &format!("{:02}.{:02}.{:02}", s.day, s.month, s.year % 100),
        8,
        "start date",
    )?;
    put(
        &mut out,
        &format!("{:02}.{:02}.{:02}", s.hour, s.minute, s.second),
        8,
        "start time",
    )?;
    put(&mut out, &((ns + 1) * 256).to_string(), 8, "header bytes")?;
    put(&mut out, "EDF+C", 44, "reserved")?;
    put(&mut out, &records.to_string(), 8, "number of data records")?;
    put(&mut out, &duration_text, 8, "record duration")?;
    put(&mut out, &ns.to_string(), 4, "number of signals")?;

    // Signal headers: one row per signal (annotation signal last), written column by column.
    let mut rows: Vec<[String; 10]> = w
        .signals
        .iter()
        .zip(&encoded)
        .map(|(s, (_, pmin, pmax))| {
            [
                s.label.clone(),
                s.transducer.clone(),
                s.physical_dimension.clone(),
                pmin.clone(),
                pmax.clone(),
                s.digital_min.to_string(),
                s.digital_max.to_string(),
                s.prefiltering.clone(),
                s.samples_per_record.to_string(),
                String::new(),
            ]
        })
        .collect();
    rows.push([
        ANNOTATIONS_LABEL.to_owned(),
        String::new(),
        String::new(),
        "-1".to_owned(),
        "1".to_owned(),
        "-32768".to_owned(),
        "32767".to_owned(),
        String::new(),
        annot_spr.to_string(),
        String::new(),
    ]);
    const COLUMNS: [(usize, &str); 10] = [
        (16, "label"),
        (80, "transducer"),
        (8, "physical dimension"),
        (8, "physical minimum"),
        (8, "physical maximum"),
        (8, "digital minimum"),
        (8, "digital maximum"),
        (80, "prefiltering"),
        (8, "samples per record"),
        (32, "signal reserved"),
    ];
    for (c, (width, name)) in COLUMNS.iter().enumerate() {
        for row in &rows {
            put(&mut out, &row[c], *width, name)?;
        }
    }

    for (r, tal) in tals.iter().enumerate() {
        for (s, (digital, _, _)) in w.signals.iter().zip(&encoded) {
            let spr = s.samples_per_record;
            for d in &digital[r * spr..(r + 1) * spr] {
                out.extend_from_slice(&d.to_le_bytes());
            }
        }
        let mut block = tal.clone();
        block.resize(annot_spr * 2, 0);
        out.extend_from_slice(&block);
    }
    Ok(out)
}

/// Quantises one signal with the header values exactly as they will be written.
fn encode_signal(s: &EdfWriteSignal) -> Result<(Vec<i16>, String, String), EdfWriteError> {
    let field = |field, reason| EdfWriteError::Field { field, reason };
    if s.digital_min >= s.digital_max {
        return Err(field("digital range", "minimum must be below maximum"));
    }
    let pmin_text = fmt8(s.physical_min).ok_or(field("physical minimum", "does not fit"))?;
    let pmax_text = fmt8(s.physical_max).ok_or(field("physical maximum", "does not fit"))?;
    let (pmin, pmax) = match (pmin_text.parse::<f64>(), pmax_text.parse::<f64>()) {
        (Ok(a), Ok(b)) if a != b => (a, b),
        _ => return Err(field("physical range", "minimum equals maximum")),
    };
    let (dmin, dmax) = (f64::from(s.digital_min), f64::from(s.digital_max));
    let scale = (dmax - dmin) / (pmax - pmin);
    let (lo, hi) = (pmin.min(pmax), pmin.max(pmax));
    let half_step = 0.5 / scale.abs();
    let mut digital = Vec::with_capacity(s.samples.len());
    for &v in &s.samples {
        if !v.is_finite() {
            return Err(EdfWriteError::NotFinite(s.label.clone()));
        }
        if v < lo - half_step || v > hi + half_step {
            return Err(EdfWriteError::OutOfRange {
                label: s.label.clone(),
                value: v,
                min: pmin,
                max: pmax,
            });
        }
        let d = ((v - pmin) * scale + dmin).round().clamp(dmin, dmax);
        digital.push(d as i16);
    }
    Ok((digital, pmin_text, pmax_text))
}

fn put(
    out: &mut Vec<u8>,
    value: &str,
    width: usize,
    field: &'static str,
) -> Result<(), EdfWriteError> {
    if !value.bytes().all(|c| (0x20..=0x7E).contains(&c)) {
        return Err(EdfWriteError::Field {
            field,
            reason: "only printable ASCII is allowed",
        });
    }
    if value.len() > width {
        return Err(EdfWriteError::Field {
            field,
            reason: "too long for the header field",
        });
    }
    out.extend_from_slice(value.as_bytes());
    out.resize(out.len() + width - value.len(), b' ');
    Ok(())
}

/// EDF+ subfield: unknown is `X`, spaces become `_` (EDF+ §2.1.3).
fn sub(value: Option<&str>) -> String {
    match value {
        Some(v) if !v.is_empty() => v.replace(' ', "_"),
        _ => "X".to_owned(),
    }
}

fn patient_field(p: &EdfPlusPatient) -> Result<String, EdfWriteError> {
    let sex = match p.sex {
        Some(EdfSex::Female) => "F",
        Some(EdfSex::Male) => "M",
        None => "X",
    };
    let birth = match p.birthdate {
        None => "X".to_owned(),
        Some((y, m, d)) => {
            if !(1..=12).contains(&m)
                || d == 0
                || d > days_in_month(y, m)
                || !(0..=9999).contains(&y)
            {
                return Err(EdfWriteError::Field {
                    field: "patient birthdate",
                    reason: "not a valid date",
                });
            }
            format!("{d:02}-{}-{y:04}", MONTHS[usize::from(m - 1)])
        }
    };
    Ok(format!(
        "{} {sex} {birth} {}",
        sub(p.code.as_deref()),
        sub(p.name.as_deref())
    ))
}

fn recording_field(w: &EdfPlusWrite) -> Result<String, EdfWriteError> {
    let s = &w.start;
    let month = MONTHS
        .get(usize::from(s.month).wrapping_sub(1))
        .ok_or(EdfWriteError::Field {
            field: "start date",
            reason: "invalid month",
        })?;
    Ok(format!(
        "Startdate {:02}-{month}-{:04} {} {} {}",
        s.day,
        s.year,
        sub(w.recording.admin_code.as_deref()),
        sub(w.recording.technician.as_deref()),
        sub(w.recording.equipment.as_deref())
    ))
}

/// Seconds as TAL text (plain decimal, never exponent notation).
fn fmt_seconds(s: f64) -> String {
    let text = format!("{s:.9}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() {
        "0".to_owned()
    } else {
        text.to_owned()
    }
}

/// The closest decimal text of at most 8 characters.
fn fmt8(x: f64) -> Option<String> {
    if !x.is_finite() {
        return None;
    }
    (0..=7usize).rev().find_map(|d| {
        let t = format!("{x:.d$}");
        let t = if t.contains('.') {
            t.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            t
        };
        let t = if t == "-0" { "0".to_owned() } else { t };
        (t.len() <= 8).then_some(t)
    })
}

#[derive(Clone, Copy)]
enum Round {
    Down,
    Up,
}

/// A value of at most 8 characters that is ≤ `x` (Down) or ≥ `x` (Up).
fn fit8(x: f64, round: Round) -> Option<f64> {
    (0..=7i32).rev().find_map(|d| {
        let p = 10f64.powi(d);
        let v = match round {
            Round::Down => (x * p).floor() / p,
            Round::Up => (x * p).ceil() / p,
        };
        let text = fmt8(v)?;
        let back: f64 = text.parse().ok()?;
        let ok = match round {
            Round::Down => back <= x,
            Round::Up => back >= x,
        };
        ok.then_some(back)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formatting_fits_eight_characters() {
        assert_eq!(fmt8(1.0).as_deref(), Some("1"));
        assert_eq!(fmt8(-3276.8).as_deref(), Some("-3276.8"));
        assert_eq!(fmt8(0.004).as_deref(), Some("0.004"));
        assert_eq!(fmt8(-0.0).as_deref(), Some("0"));
        assert_eq!(fmt8(123_456_789.0), None);
        // "-1234.568" would be 9 characters: the sign costs a digit.
        assert_eq!(fit8(-1_234.567_891, Round::Down), Some(-1234.57));
        assert_eq!(fit8(1_234.567_891, Round::Up), Some(1234.568));
        assert_eq!(fmt_seconds(0.25), "0.25");
        assert_eq!(fmt_seconds(3.0), "3");
        assert_eq!(fmt_seconds(0.0), "0");
    }

    #[test]
    fn plain_decimals_only() {
        assert!(plain_decimal("-3276.8"));
        assert!(plain_decimal("+1"));
        assert!(!plain_decimal("inf"));
        assert!(!plain_decimal("1e3"));
        assert!(!plain_decimal("."));
        assert!(!plain_decimal("1.2.3"));
    }
}
