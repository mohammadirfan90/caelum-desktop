//! CSV export, one file per parameter (F-SYS-08, docs/05 §5).
//!
//! The writer follows RFC 4180: records end with CRLF, a field is quoted when it contains a comma,
//! a double quote, CR or LF, and quotes inside a quoted field are doubled. The output is UTF-8
//! without a byte-order mark.
//!
//! A spreadsheet would run a cell that starts with `=`, `+`, `-` or `@` as a formula. Free-text
//! cells (labels, device names, states) that start with one of those characters are written with
//! a leading apostrophe (OWASP CSV-injection guidance). Numeric cells are never changed, so a
//! negative number stays a number.

use caelum_domain::ids::DeviceId;
use caelum_domain::observation::{Classification, Quality};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::{LeapSeconds, TaiNs, Validity};
use caelum_domain::units::Unit;

use super::format_utc;

/// An RFC 4180 writer into memory.
#[derive(Debug, Default)]
pub struct CsvWriter {
    out: String,
    columns: Option<usize>,
}

/// Writer errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CsvError {
    /// A record has a different number of fields from the first record (RFC 4180 §2.4).
    #[error("record has {got} fields, the file has {expected}")]
    FieldCount {
        /// Fields in the first record.
        expected: usize,
        /// Fields in this record.
        got: usize,
    },
    /// A record has no fields.
    #[error("empty record")]
    Empty,
    /// A time could not be converted to UTC.
    #[error("time {0} cannot be converted to UTC")]
    Time(i64),
    /// A row's parameter differs from the export's parameter.
    #[error("row for {got:?} in the export of {expected:?}")]
    WrongParameter {
        /// The export's parameter.
        expected: Parameter,
        /// The row's parameter.
        got: Parameter,
    },
    /// A numeric value is NaN or infinite.
    #[error("value is not finite")]
    NotFinite,
}

/// A cell: text is checked for formula prefixes, numbers are written as they are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cell<'a> {
    /// Free text.
    Text(&'a str),
    /// A number already formatted by the caller (written unchanged).
    Number(&'a str),
}

impl CsvWriter {
    /// An empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one record. Every record must have as many fields as the first.
    pub fn record(&mut self, fields: &[Cell<'_>]) -> Result<(), CsvError> {
        if fields.is_empty() {
            return Err(CsvError::Empty);
        }
        match self.columns {
            Some(n) if n != fields.len() => {
                return Err(CsvError::FieldCount {
                    expected: n,
                    got: fields.len(),
                })
            }
            _ => self.columns = Some(fields.len()),
        }
        for (i, f) in fields.iter().enumerate() {
            if i > 0 {
                self.out.push(',');
            }
            let text = match f {
                Cell::Number(n) => (*n).to_owned(),
                Cell::Text(t) if t.starts_with(['=', '+', '-', '@', '\t', '\r']) => {
                    format!("'{t}")
                }
                Cell::Text(t) => (*t).to_owned(),
            };
            push_escaped(&mut self.out, &text);
        }
        self.out.push_str("\r\n");
        Ok(())
    }

    /// The CSV text.
    pub fn finish(self) -> String {
        self.out
    }
}

fn push_escaped(out: &mut String, field: &str) {
    if field.contains([',', '"', '\r', '\n']) {
        out.push('"');
        out.push_str(&field.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(field);
    }
}

/// A stored value to export.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportValue {
    /// A number in the row's unit.
    Number(f64),
    /// An enumerated state.
    State(String),
}

/// One observation of the exported parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportRow {
    /// The parameter (must equal the export's parameter).
    pub parameter: Parameter,
    /// When it was measured (TAI).
    pub measured_at: TaiNs,
    /// The value.
    pub value: ExportValue,
    /// Unit of a numeric value (the catalog's canonical unit); `None` for states.
    pub unit: Option<Unit>,
    /// Signal quality.
    pub quality: Quality,
    /// Measured, derived, entered or imported.
    pub classification: Classification,
    /// Source device, if any (`None` for entered values and research exports).
    pub device_id: Option<DeviceId>,
}

/// The header of a per-parameter export.
pub const HEADER: [&str; 9] = [
    "parameter",
    "measured_at_utc",
    "utc_exact",
    "measured_at_tai_ns",
    "value",
    "unit",
    "quality",
    "classification",
    "device_id",
];

/// Writes the CSV export of one parameter (F-SYS-08). Rows are written in the order given.
/// `utc_exact` is `false` for times after the leap-second table's expiry (docs/05 §4).
pub fn export_parameter(
    parameter: Parameter,
    rows: &[ExportRow],
    leap: &LeapSeconds,
) -> Result<String, CsvError> {
    let mut w = CsvWriter::new();
    let header: Vec<Cell<'_>> = HEADER.iter().map(|h| Cell::Text(h)).collect();
    w.record(&header)?;
    let key = parameter.spec().key;
    for row in rows {
        if row.parameter != parameter {
            return Err(CsvError::WrongParameter {
                expected: parameter,
                got: row.parameter,
            });
        }
        let (utc, validity) = leap
            .tai_to_utc(row.measured_at)
            .map_err(|_| CsvError::Time(row.measured_at.0))?;
        let utc = format_utc(utc);
        let exact = if validity == Validity::Exact {
            "true"
        } else {
            "false"
        };
        let tai = row.measured_at.0.to_string();
        let value = match &row.value {
            ExportValue::Number(v) if v.is_finite() => v.to_string(),
            ExportValue::Number(_) => return Err(CsvError::NotFinite),
            ExportValue::State(s) => s.clone(),
        };
        let value_cell = match row.value {
            ExportValue::Number(_) => Cell::Number(&value),
            ExportValue::State(_) => Cell::Text(&value),
        };
        let device = row.device_id.map(|d| d.to_string()).unwrap_or_default();
        w.record(&[
            Cell::Text(key),
            Cell::Text(&utc),
            Cell::Text(exact),
            Cell::Number(&tai),
            value_cell,
            Cell::Text(row.unit.map(Unit::ucum).unwrap_or("")),
            Cell::Text(quality_name(row.quality)),
            Cell::Text(classification_name(row.classification)),
            Cell::Text(&device),
        ])?;
    }
    Ok(w.finish())
}

fn quality_name(q: Quality) -> &'static str {
    match q {
        Quality::Good => "good",
        Quality::Degraded => "degraded",
        Quality::Poor => "poor",
        Quality::Unusable => "unusable",
    }
}

fn classification_name(c: Classification) -> &'static str {
    match c {
        Classification::Measured => "measured",
        Classification::Derived => "derived",
        Classification::Entered => "entered",
        Classification::Imported => "imported",
        #[allow(unreachable_patterns)]
        _ => "simulated",
    }
}

/// Parses RFC 4180 text into records (used to check exports reopen; tolerant of a missing final
/// CRLF, strict about quotes).
pub fn parse(text: &str) -> Result<Vec<Vec<String>>, CsvParseError> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut at_field_start = true;
    let mut line = 1usize;
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => {
                    quoted = false;
                    if !matches!(chars.peek(), None | Some(',') | Some('\r') | Some('\n')) {
                        return Err(CsvParseError { line });
                    }
                }
                '\n' => {
                    line += 1;
                    field.push(c);
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if at_field_start => {
                quoted = true;
                at_field_start = false;
            }
            '"' => return Err(CsvParseError { line }),
            ',' => {
                record.push(std::mem::take(&mut field));
                at_field_start = true;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                at_field_start = true;
                line += 1;
            }
            _ => {
                field.push(c);
                at_field_start = false;
            }
        }
    }
    if quoted {
        return Err(CsvParseError { line });
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}

/// A malformed CSV text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed CSV at line {line}")]
pub struct CsvParseError {
    /// 1-based line.
    pub line: usize,
}
