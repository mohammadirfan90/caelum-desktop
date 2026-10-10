//! HL7 aECG (Annotated ECG, HL7 v3, ANSI/HL7 V3 ECG R1) reader for ECG leads (F-ACQ-07).
//!
//! Supported: `AnnotatedECG` documents in UTF-8 with rhythm `series` and representative-beat
//! `derivedSeries`; each `sequenceSet` must hold one time sequence (`TIME_ABSOLUTE` as
//! `GLIST_TS`, or `TIME_RELATIVE` as `GLIST_PQ`, increment in `s` or `ms`) and lead sequences as
//! `SLIST_PQ` (`origin`, `scale`, `digits`). Physical value = origin + scale × digit, in the
//! scale's unit. Annotation sets are not read.
//!
//! Security: document type declarations are refused (no DTD processing, no external or custom
//! entities; only the five predefined XML entities and character references are resolved).

use std::collections::BTreeMap;

use quick_xml::events::Event;
use quick_xml::Reader;
use quick_xml::XmlVersion;

use super::{CivilDateTime, WaveformChannel};

/// Deepest element nesting accepted.
const MAX_DEPTH: usize = 128;

/// A decoded aECG document.
#[derive(Debug, Clone, PartialEq)]
pub struct AecgDocument {
    /// `id/@root` (and `@extension`, joined with `^` when present).
    pub id: Option<String>,
    /// `effectiveTime/low/@value` of the document, as written.
    pub effective_low: Option<String>,
    /// Trial subject identifier, if present (`trialSubject/id/@extension`).
    pub subject_id: Option<String>,
    /// Series in document order.
    pub series: Vec<AecgSeries>,
}

/// The kind of a series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesKind {
    /// `RHYTHM`.
    Rhythm,
    /// `REPRESENTATIVE_BEAT`.
    RepresentativeBeat,
    /// Another code, as written (or empty if none).
    Other(String),
}

/// One sequence set of a series: leads sharing a time base.
#[derive(Debug, Clone, PartialEq)]
pub struct AecgSeries {
    /// Rhythm, representative beat, or other.
    pub kind: SeriesKind,
    /// Absolute start (`TIME_ABSOLUTE` head, or the series `effectiveTime/low` for relative
    /// time), if stated to the second. `None` when the document gives no usable absolute time.
    pub start: Option<CivilDateTime>,
    /// Head of a `TIME_RELATIVE` sequence, s (0 for absolute time).
    pub time_offset_s: f64,
    /// Sampling frequency, Hz.
    pub sample_rate_hz: f64,
    /// Leads (label without the `MDC_ECG_LEAD_` prefix; unit from `scale`).
    pub leads: Vec<WaveformChannel>,
    /// Lead codes as written (`MDC_ECG_LEAD_II`, …), parallel to `leads`.
    pub lead_codes: Vec<String>,
}

/// Reader errors.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AecgError {
    /// The document is not UTF-8.
    #[error("document is not UTF-8")]
    NotUtf8,
    /// XML is not well formed.
    #[error("XML error: {0}")]
    Xml(String),
    /// A document type declaration (DTD) was present.
    #[error("document type declarations are not accepted")]
    Dtd,
    /// An entity other than the five predefined ones.
    #[error("unknown entity &{0};")]
    Entity(String),
    /// Nesting deeper than the limit.
    #[error("elements nested too deeply")]
    TooDeep,
    /// The root element is not `AnnotatedECG`.
    #[error("root element is {0:?}, not AnnotatedECG")]
    NotAecg(String),
    /// No series with leads.
    #[error("no ECG series")]
    NoSeries,
    /// A series could not be interpreted.
    #[error("series {series}: {reason}")]
    Series {
        /// Series index (document order, 0-based).
        series: usize,
        /// What is wrong.
        reason: String,
    },
}

#[derive(Debug, Default)]
struct Node {
    name: String,
    attrs: BTreeMap<String, String>,
    children: Vec<Node>,
    text: String,
}

impl Node {
    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    fn path(&self, names: &[&str]) -> Option<&Node> {
        names.iter().try_fold(self, |n, name| n.child(name))
    }

    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.get(name).map(String::as_str)
    }

    /// Every descendant with this name, in document order (not descending into matches).
    fn find_all<'a>(&'a self, name: &str, out: &mut Vec<&'a Node>) {
        for c in &self.children {
            if c.name == name {
                out.push(c);
            }
            c.find_all(name, out);
        }
    }
}

fn parse_xml(data: &[u8]) -> Result<Node, AecgError> {
    let text = std::str::from_utf8(data).map_err(|_| AecgError::NotUtf8)?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().check_end_names = true;
    let mut stack: Vec<Node> = vec![Node::default()];
    let xml = |e: quick_xml::Error| AecgError::Xml(e.to_string());
    loop {
        match reader.read_event().map_err(xml)? {
            Event::DocType(_) => return Err(AecgError::Dtd),
            Event::Start(e) => {
                if stack.len() > MAX_DEPTH {
                    return Err(AecgError::TooDeep);
                }
                stack.push(element(&e)?);
            }
            Event::Empty(e) => {
                let node = element(&e)?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                }
            }
            Event::End(_) => {
                let node = stack
                    .pop()
                    .ok_or(AecgError::Xml("unbalanced end tag".into()))?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => return Err(AecgError::Xml("unbalanced end tag".into())),
                }
            }
            Event::Text(t) => {
                if let Some(n) = stack.last_mut() {
                    n.text.push_str(&t.xml10_content());
                }
            }
            Event::CData(t) => {
                if let Some(n) = stack.last_mut() {
                    n.text.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(r) => {
                let ch = match r.resolve_char_ref().map_err(xml)? {
                    Some(c) => c,
                    None => match &*r {
                        "lt" => '<',
                        "gt" => '>',
                        "amp" => '&',
                        "apos" => '\'',
                        "quot" => '"',
                        other => return Err(AecgError::Entity(other.to_owned())),
                    },
                };
                if let Some(n) = stack.last_mut() {
                    n.text.push(ch);
                }
            }
            Event::Eof => break,
            Event::Decl(_) | Event::PI(_) | Event::Comment(_) => {}
        }
    }
    if stack.len() != 1 {
        return Err(AecgError::Xml("unclosed element".into()));
    }
    let mut doc = stack.pop().unwrap_or_default();
    match doc.children.len() {
        1 => Ok(doc.children.remove(0)),
        _ => Err(AecgError::Xml(
            "document must have exactly one root element".into(),
        )),
    }
}

fn element(e: &quick_xml::events::BytesStart<'_>) -> Result<Node, AecgError> {
    let mut node = Node {
        name: e.local_name().as_ref().to_owned(),
        ..Node::default()
    };
    for a in e.attributes() {
        let a = a.map_err(|err| AecgError::Xml(err.to_string()))?;
        let value = a
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|err| AecgError::Xml(err.to_string()))?;
        node.attrs
            .insert(a.key.local_name().as_ref().to_owned(), value.into_owned());
    }
    Ok(node)
}

/// Parses an HL7 aECG document (F-ACQ-07).
pub fn read(data: &[u8]) -> Result<AecgDocument, AecgError> {
    let root = parse_xml(data)?;
    if root.name != "AnnotatedECG" {
        return Err(AecgError::NotAecg(root.name));
    }
    let id = root.child("id").and_then(|i| {
        let r = i.attr("root")?;
        Some(match i.attr("extension") {
            Some(x) => format!("{r}^{x}"),
            None => r.to_owned(),
        })
    });
    let effective_low = root
        .path(&["effectiveTime", "low"])
        .and_then(|l| l.attr("value"))
        .map(str::to_owned);
    let mut subjects = Vec::new();
    root.find_all("trialSubject", &mut subjects);
    let subject_id = subjects
        .first()
        .and_then(|s| s.child("id"))
        .and_then(|i| i.attr("extension").or(i.attr("root")))
        .map(str::to_owned);

    let mut series_nodes = Vec::new();
    collect_series(&root, &mut series_nodes);
    let mut series = Vec::new();
    for (index, node) in series_nodes.iter().enumerate() {
        let err = |reason: String| AecgError::Series {
            series: index,
            reason,
        };
        let kind = match node.child("code").and_then(|c| c.attr("code")) {
            Some("RHYTHM") => SeriesKind::Rhythm,
            Some("REPRESENTATIVE_BEAT") => SeriesKind::RepresentativeBeat,
            other => SeriesKind::Other(other.unwrap_or("").to_owned()),
        };
        let series_low = node
            .path(&["effectiveTime", "low"])
            .and_then(|l| l.attr("value"))
            .and_then(|v| parse_ts(v).ok());
        for set in node
            .children("component")
            .filter_map(|c| c.child("sequenceSet"))
        {
            series.push(sequence_set(set, kind.clone(), series_low).map_err(err)?);
        }
    }
    if series.iter().all(|s| s.leads.is_empty()) {
        return Err(AecgError::NoSeries);
    }
    Ok(AecgDocument {
        id,
        effective_low,
        subject_id,
        series,
    })
}

/// `series` and `derivedSeries` elements in document order.
fn collect_series<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
    for c in &n.children {
        if c.name == "series" || c.name == "derivedSeries" {
            out.push(c);
        }
        collect_series(c, out);
    }
}

fn sequence_set(
    set: &Node,
    kind: SeriesKind,
    series_low: Option<CivilDateTime>,
) -> Result<AecgSeries, String> {
    let mut time: Option<(Option<CivilDateTime>, f64, f64)> = None;
    let mut leads = Vec::new();
    let mut lead_codes = Vec::new();
    for seq in set
        .children("component")
        .filter_map(|c| c.child("sequence"))
    {
        let code = seq
            .child("code")
            .and_then(|c| c.attr("code"))
            .ok_or("sequence without a code")?;
        let value = seq
            .child("value")
            .ok_or_else(|| format!("{code}: no value"))?;
        match code {
            "TIME_ABSOLUTE" | "TIME_RELATIVE" => {
                if time.is_some() {
                    return Err("more than one time sequence".into());
                }
                let head = value
                    .child("head")
                    .ok_or_else(|| format!("{code}: no head"))?;
                let inc = value
                    .child("increment")
                    .ok_or_else(|| format!("{code}: no increment"))?;
                let step_s = seconds(inc).ok_or_else(|| format!("{code}: increment"))?;
                if step_s.is_nan() || step_s <= 0.0 {
                    return Err(format!("{code}: increment must be positive"));
                }
                time = Some(if code == "TIME_ABSOLUTE" {
                    let v = head.attr("value").ok_or("TIME_ABSOLUTE: head value")?;
                    (parse_ts(v).ok(), 0.0, step_s)
                } else {
                    let offset = seconds(head).ok_or("TIME_RELATIVE: head")?;
                    (series_low, offset, step_s)
                });
            }
            _ => {
                if value.child("digits").is_none() {
                    // Not a sampled lead (for example a GLIST of another quantity): not read.
                    continue;
                }
                let (unit, samples) = slist(value).map_err(|r| format!("{code}: {r}"))?;
                let label = code
                    .strip_prefix("MDC_ECG_LEAD_")
                    .unwrap_or(code)
                    .to_owned();
                lead_codes.push(code.to_owned());
                leads.push(WaveformChannel {
                    label,
                    unit,
                    sample_rate_hz: 0.0,
                    samples,
                });
            }
        }
    }
    let (start, time_offset_s, step_s) = time.ok_or("no time sequence")?;
    let sample_rate_hz = 1.0 / step_s;
    for l in &mut leads {
        l.sample_rate_hz = sample_rate_hz;
    }
    Ok(AecgSeries {
        kind,
        start,
        time_offset_s,
        sample_rate_hz,
        leads,
        lead_codes,
    })
}

/// A PQ (`value`, `unit` in `s` or `ms`) as seconds.
fn seconds(pq: &Node) -> Option<f64> {
    let v: f64 = pq
        .attr("value")?
        .trim()
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite())?;
    match pq.attr("unit")? {
        "s" => Some(v),
        "ms" => Some(v / 1_000.0),
        _ => None,
    }
}

/// SLIST_PQ → (unit, physical samples).
fn slist(value: &Node) -> Result<(String, Vec<f64>), String> {
    let pq = |name: &str| -> Result<(f64, String), String> {
        let n = value.child(name).ok_or_else(|| format!("no {name}"))?;
        let v: f64 = n
            .attr("value")
            .and_then(|v| v.trim().parse().ok())
            .filter(|v: &f64| v.is_finite())
            .ok_or_else(|| format!("{name} value"))?;
        let unit = n
            .attr("unit")
            .ok_or_else(|| format!("{name} without unit"))?;
        Ok((v, unit.to_owned()))
    };
    let (origin, origin_unit) = pq("origin")?;
    let (scale, unit) = pq("scale")?;
    if origin_unit != unit && origin != 0.0 {
        return Err(format!(
            "origin unit {origin_unit} differs from scale unit {unit}"
        ));
    }
    if scale == 0.0 {
        return Err("scale is 0".into());
    }
    let digits = value.child("digits").map(|d| d.text.as_str()).unwrap_or("");
    let samples = digits
        .split_ascii_whitespace()
        .map(|t| t.parse::<i64>().map(|d| origin + scale * d as f64))
        .collect::<Result<Vec<f64>, _>>()
        .map_err(|_| "digits are not integers".to_owned())?;
    if samples.is_empty() {
        return Err("no digits".into());
    }
    Ok((unit, samples))
}

/// HL7 TS `YYYYMMDDHHMMSS[.F+][±ZZZZ]`; less than second precision is an error.
pub fn parse_ts(text: &str) -> Result<CivilDateTime, &'static str> {
    let t = text.trim();
    let (body, offset) = match t.find(['+', '-']) {
        Some(i) => {
            let tz = &t[i..];
            if tz.len() != 5 || !tz[1..].bytes().all(|c| c.is_ascii_digit()) {
                return Err("time zone");
            }
            let h: i16 = tz[1..3].parse().map_err(|_| "time zone")?;
            let m: i16 = tz[3..5].parse().map_err(|_| "time zone")?;
            let o = h * 60 + m;
            (&t[..i], Some(if tz.starts_with('-') { -o } else { o }))
        }
        None => (t, None),
    };
    let (main, frac) = match body.split_once('.') {
        Some((m, f)) => (m, Some(f)),
        None => (body, None),
    };
    if main.len() != 14 || !main.bytes().all(|c| c.is_ascii_digit()) {
        return Err("precision below seconds or malformed");
    }
    let n = |r: std::ops::Range<usize>| main[r].parse::<u32>().map_err(|_| "digits");
    let nanos = match frac {
        None => 0,
        Some(f) if !f.is_empty() && f.len() <= 9 && f.bytes().all(|c| c.is_ascii_digit()) => {
            format!("{f:0<9}").parse().map_err(|_| "fraction")?
        }
        Some(_) => return Err("fraction"),
    };
    CivilDateTime::new(
        n(0..4)? as i32,
        n(4..6)? as u8,
        n(6..8)? as u8,
        n(8..10)? as u8,
        n(10..12)? as u8,
        n(12..14)? as u8,
        nanos,
        offset,
    )
    .map_err(|_| "out of range")
}
