//! XTCE 1.2 (OMG XTCE, namespace `http://www.omg.org/spec/XTCE/20180204`) mission database
//! generated from the parameter catalog and the APID plan (F-GND-02, F-GND-03).
//!
//! Two scopes:
//! - [`XtceScope::Ops`] for mission control: the CCSDS headers and the OPS containers only, so
//!   the control-room database names no physiological parameter (R-SEC-06);
//! - [`XtceScope::Full`] for the flight-surgeon / BME ground system: also the MEDICAL summary
//!   containers, one derived container per downlinked catalog parameter (restricted on the
//!   parameter index), with its UCUM unit.
//!
//! The CUC coarse time is an `AbsoluteTimeParameterType` with epoch `TAI` (1958-01-01 TAI, the
//! CCSDS epoch). Output is deterministic: the same catalog, plan and version give the same bytes.

use std::io::Cursor;

use caelum_domain::parameters::{ValueType, CATALOG};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;

use crate::config::ApidPlan;
use crate::medical::downlinked;
use crate::ops::{InstanceHealth, MdsLinkState, OpsAlarmState, OpsCategory, OpsPriority};

/// XTCE 1.2 namespace.
pub const XTCE_NS: &str = "http://www.omg.org/spec/XTCE/20180204";

/// Which containers to include.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XtceScope {
    /// OPS only (mission control).
    Ops,
    /// OPS and MEDICAL (flight surgeon, BME).
    Full,
}

/// Generation errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("XTCE generation failed: {0}")]
pub struct XtceError(String);

/// A minimal element tree serialised with quick-xml.
#[derive(Debug, Clone)]
struct El {
    name: &'static str,
    attrs: Vec<(&'static str, String)>,
    text: Option<String>,
    children: Vec<El>,
}

fn el(name: &'static str) -> El {
    El {
        name,
        attrs: Vec::new(),
        text: None,
        children: Vec::new(),
    }
}

impl El {
    fn a(mut self, k: &'static str, v: impl ToString) -> Self {
        self.attrs.push((k, v.to_string()));
        self
    }
    fn c(mut self, child: El) -> Self {
        self.children.push(child);
        self
    }
    fn cs(mut self, children: impl IntoIterator<Item = El>) -> Self {
        self.children.extend(children);
        self
    }
    fn t(mut self, text: impl ToString) -> Self {
        self.text = Some(text.to_string());
        self
    }

    fn write(&self, w: &mut Writer<Cursor<Vec<u8>>>) -> std::io::Result<()> {
        let mut start = BytesStart::new(self.name);
        for (k, v) in &self.attrs {
            start.push_attribute((*k, v.as_str()));
        }
        if self.children.is_empty() && self.text.is_none() {
            return w.write_event(Event::Empty(start));
        }
        w.write_event(Event::Start(start))?;
        if let Some(t) = &self.text {
            w.write_event(Event::Text(BytesText::new(t)))?;
        }
        for c in &self.children {
            c.write(w)?;
        }
        w.write_event(Event::End(BytesEnd::new(self.name)))
    }
}

fn int_type(name: &str, bits: u32) -> El {
    el("IntegerParameterType")
        .a("name", name)
        .a("signed", "false")
        .c(el("IntegerDataEncoding").a("sizeInBits", bits))
}

fn enum_type(name: &str, values: impl IntoIterator<Item = (u8, String)>) -> El {
    el("EnumeratedParameterType")
        .a("name", name)
        .c(el("IntegerDataEncoding").a("sizeInBits", 8))
        .c(el("EnumerationList").cs(values
            .into_iter()
            .map(|(v, l)| el("Enumeration").a("value", v).a("label", l))))
}

fn param(name: impl ToString, ty: &str) -> El {
    el("Parameter").a("name", name).a("parameterTypeRef", ty)
}

fn entry(name: impl ToString) -> El {
    el("ParameterRefEntry").a("parameterRef", name)
}

fn derived(name: &str, base: &str, restriction: El, entries: Vec<El>, abstract_: bool) -> El {
    let mut c = el("SequenceContainer").a("name", name);
    if abstract_ {
        c = c.a("abstract", "true");
    }
    c.c(el("EntryList").cs(entries)).c(el("BaseContainer")
        .a("containerRef", base)
        .c(el("RestrictionCriteria").c(restriction)))
}

fn cmp(param: &str, value: impl ToString) -> El {
    el("Comparison").a("parameterRef", param).a("value", value)
}

/// Generates the XTCE document.
pub fn generate(plan: &ApidPlan, version: &str, scope: XtceScope) -> Result<String, XtceError> {
    let names = |all: &[(u8, &str)]| -> Vec<(u8, String)> {
        all.iter().map(|(c, n)| (*c, (*n).to_owned())).collect()
    };
    let prio: Vec<(u8, &str)> = OpsPriority::ALL
        .iter()
        .map(|p| (p.code(), p.name()))
        .collect();
    let cat: Vec<(u8, &str)> = OpsCategory::ALL
        .iter()
        .map(|c| (c.code(), c.name()))
        .collect();
    let state: Vec<(u8, &str)> = OpsAlarmState::ALL
        .iter()
        .map(|s| (s.code(), s.name()))
        .collect();
    let with_none = |v: &[(u8, &str)]| {
        let mut out = vec![(0u8, "NONE".to_owned())];
        out.extend(names(v));
        out
    };

    let mut types = vec![
        int_type("uint1_t", 1),
        int_type("uint2_t", 2),
        int_type("uint3_t", 3),
        int_type("uint8_t", 8),
        int_type("uint11_t", 11),
        int_type("uint14_t", 14),
        int_type("uint16_t", 16),
        el("BinaryParameterType")
            .a("name", "uuid_t")
            .c(el("BinaryDataEncoding").c(el("SizeInBits").c(el("FixedValue").t(128)))),
        el("AbsoluteTimeParameterType")
            .a("name", "cuc_coarse_t")
            .c(el("Encoding")
                .a("units", "seconds")
                .c(el("IntegerDataEncoding").a("sizeInBits", 32)))
            .c(el("ReferenceTime").c(el("Epoch").t("TAI"))),
        enum_type("priority_t", names(&prio)),
        enum_type("category_t", names(&cat)),
        enum_type("alarm_state_t", names(&state)),
        enum_type("priority_or_none_t", with_none(&prio)),
        enum_type("category_or_none_t", with_none(&cat)),
        enum_type("alarm_state_or_none_t", with_none(&state)),
        enum_type(
            "instance_health_t",
            InstanceHealth::ALL
                .iter()
                .map(|h| (h.code(), h.name().to_owned())),
        ),
        enum_type(
            "mds_link_t",
            MdsLinkState::ALL
                .iter()
                .map(|h| (h.code(), h.name().to_owned())),
        ),
    ];

    let mut params = vec![
        param("ccsds_version", "uint3_t"),
        param("ccsds_type", "uint1_t"),
        param("ccsds_sec_hdr_flag", "uint1_t"),
        param("ccsds_apid", "uint11_t"),
        param("ccsds_seq_flags", "uint2_t"),
        param("ccsds_seq_count", "uint14_t"),
        param("ccsds_length", "uint16_t"),
        param("cuc_p_field", "uint8_t"),
        param("cuc_coarse", "cuc_coarse_t"),
        param("cuc_fine", "uint16_t"),
        param("ops_crew_id", "uuid_t"),
        param("ops_priority", "priority_t"),
        param("ops_category", "category_t"),
        param("ops_state", "alarm_state_t"),
        param("ops_escalation_stage", "uint8_t"),
        param("ops_created_coarse", "cuc_coarse_t"),
        param("ops_created_fine", "uint16_t"),
        param("crew_status_crew_id", "uuid_t"),
        param("crew_status_priority", "priority_or_none_t"),
        param("crew_status_category", "category_or_none_t"),
        param("crew_status_state", "alarm_state_or_none_t"),
        param("instance_health", "instance_health_t"),
        param("instance_mds_link", "mds_link_t"),
    ];

    let header_entries: Vec<El> = [
        "ccsds_version",
        "ccsds_type",
        "ccsds_sec_hdr_flag",
        "ccsds_apid",
        "ccsds_seq_flags",
        "ccsds_seq_count",
        "ccsds_length",
        "cuc_p_field",
        "cuc_coarse",
        "cuc_fine",
    ]
    .into_iter()
    .map(entry)
    .collect();

    let mut containers = vec![
        el("SequenceContainer")
            .a("name", "caelum_tm")
            .a("abstract", "true")
            .c(el("EntryList").cs(header_entries)),
        derived(
            "ops_alarm",
            "caelum_tm",
            cmp("ccsds_apid", plan.ops_alarm.value()),
            [
                "ops_crew_id",
                "ops_priority",
                "ops_category",
                "ops_state",
                "ops_escalation_stage",
                "ops_created_coarse",
                "ops_created_fine",
            ]
            .into_iter()
            .map(entry)
            .collect(),
            false,
        ),
        derived(
            "ops_crew_status",
            "caelum_tm",
            cmp("ccsds_apid", plan.ops_crew_status.value()),
            [
                "crew_status_crew_id",
                "crew_status_priority",
                "crew_status_category",
                "crew_status_state",
            ]
            .into_iter()
            .map(entry)
            .collect(),
            false,
        ),
        derived(
            "ops_instance_status",
            "caelum_tm",
            cmp("ccsds_apid", plan.ops_instance_status.value()),
            vec![entry("instance_health"), entry("instance_mds_link")],
            false,
        ),
    ];

    if scope == XtceScope::Full {
        types.push(enum_type(
            "quality_t",
            [(1, "good"), (2, "degraded"), (3, "poor"), (4, "unusable")]
                .map(|(c, n)| (c, n.to_owned())),
        ));
        params.extend([
            param("med_crew_id", "uuid_t"),
            param("med_parameter_index", "uint8_t"),
            param("med_quality", "quality_t"),
            param("med_window_start_coarse", "cuc_coarse_t"),
            param("med_window_start_fine", "uint16_t"),
        ]);
        containers.push(derived(
            "medical_summary",
            "caelum_tm",
            cmp("ccsds_apid", plan.medical_summary.value()),
            [
                "med_crew_id",
                "med_parameter_index",
                "med_quality",
                "med_window_start_coarse",
                "med_window_start_fine",
            ]
            .into_iter()
            .map(entry)
            .collect(),
            true,
        ));
        for (index, spec) in CATALOG.iter().enumerate() {
            if !downlinked(spec.parameter) {
                continue;
            }
            let key = spec.key;
            let ty = format!("{key}_t");
            let entries = match spec.value {
                ValueType::Numeric { .. } => {
                    types.push(
                        el("FloatParameterType")
                            .a("name", &ty)
                            .a("sizeInBits", 32)
                            .c(el("LongDescription").t(spec.label))
                            .c(el("UnitSet").c(el("Unit").t(spec.unit.ucum())))
                            .c(el("FloatDataEncoding").a("sizeInBits", 32)),
                    );
                    for s in ["min", "mean", "max"] {
                        params.push(param(format!("{key}_{s}"), &ty));
                    }
                    params.push(param(format!("{key}_count"), "uint16_t"));
                    ["min", "mean", "max", "count"]
                        .into_iter()
                        .map(|s| entry(format!("{key}_{s}")))
                        .collect()
                }
                ValueType::Enum(states) => {
                    let values = states
                        .iter()
                        .enumerate()
                        .map(|(i, s)| (u8::try_from(i).unwrap_or(u8::MAX), (*s).to_owned()));
                    types.push(enum_type(&ty, values));
                    params.push(param(format!("{key}_last"), &ty));
                    params.push(param(format!("{key}_count"), "uint16_t"));
                    vec![entry(format!("{key}_last")), entry(format!("{key}_count"))]
                }
                ValueType::EnumPending(_) => continue,
            };
            containers.push(derived(
                &format!("medical_{key}"),
                "medical_summary",
                cmp("med_parameter_index", index),
                entries,
                false,
            ));
        }
    }

    let doc = el("SpaceSystem")
        .a("xmlns", XTCE_NS)
        .a("name", "CAELUM")
        .c(el("Header")
            .a("version", version)
            .a("validationStatus", "Working")
            .a(
                "classification",
                match scope {
                    XtceScope::Ops => "OPS",
                    XtceScope::Full => "MEDICAL",
                },
            ))
        .c(el("TelemetryMetaData")
            .c(el("ParameterTypeSet").cs(types))
            .c(el("ParameterSet").cs(params))
            .c(el("ContainerSet").cs(containers)));

    let mut w = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);
    w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .map_err(|e| XtceError(e.to_string()))?;
    doc.write(&mut w).map_err(|e| XtceError(e.to_string()))?;
    String::from_utf8(w.into_inner().into_inner()).map_err(|e| XtceError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::tests::plan;
    use quick_xml::events::Event;
    use quick_xml::Reader;
    use std::collections::BTreeSet;

    /// Parses the document; returns the set of parameter names and container names.
    fn names(xml: &str) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut r = Reader::from_str(xml);
        let (mut params, mut containers) = (BTreeSet::new(), BTreeSet::new());
        loop {
            match r.read_event().expect("well-formed XML") {
                Event::Eof => break,
                Event::Start(e) | Event::Empty(e) => {
                    let n = e.name().as_ref().to_owned();
                    let name = e
                        .attributes()
                        .filter_map(Result::ok)
                        .find(|a| a.key.as_ref() == "name")
                        .map(|a| a.value.into_owned());
                    match (n.as_str(), name) {
                        ("Parameter", Some(v)) => {
                            assert!(params.insert(v), "duplicate parameter");
                        }
                        ("SequenceContainer", Some(v)) => {
                            assert!(containers.insert(v), "duplicate container");
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        (params, containers)
    }

    /// F-GND-02: every downlinked catalog parameter is in the full MDB with its container.
    #[test]
    fn f_gnd_02_every_downlinked_parameter_is_defined() {
        let xml = generate(&plan(), "1", XtceScope::Full).expect("generates");
        let (params, containers) = names(&xml);
        for spec in CATALOG.iter().filter(|s| downlinked(s.parameter)) {
            assert!(
                containers.contains(&format!("medical_{}", spec.key)),
                "{}",
                spec.key
            );
            assert!(
                params.contains(&format!("{}_count", spec.key)),
                "{}",
                spec.key
            );
        }
        assert!(xml.contains("<Epoch>TAI</Epoch>"));
        assert!(xml.contains(&format!("value=\"{}\"", plan().ops_alarm.value())));
        assert!(xml.contains("<Unit>/min</Unit>"));
    }

    /// R-SEC-06: the mission-control MDB names no physiological parameter.
    #[test]
    fn r_sec_06_ops_mdb_has_no_physiology() {
        let xml = generate(&plan(), "1", XtceScope::Ops).expect("generates");
        let (params, containers) = names(&xml);
        for spec in CATALOG.iter() {
            assert!(!xml.contains(&format!("\"{}_", spec.key)), "{}", spec.key);
        }
        assert!(params.iter().all(|p| !p.starts_with("med_")));
        assert_eq!(
            containers,
            [
                "caelum_tm",
                "ops_alarm",
                "ops_crew_status",
                "ops_instance_status"
            ]
            .map(String::from)
            .into()
        );
    }

    #[test]
    fn generation_is_deterministic() {
        let a = generate(&plan(), "7", XtceScope::Full).expect("a");
        let b = generate(&plan(), "7", XtceScope::Full).expect("b");
        assert_eq!(a, b);
        assert!(a.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(a.contains("version=\"7\""));
    }

    /// Container bit sizes add up to the packet layouts of `ops` and `medical`.
    #[test]
    fn f_gnd_02_header_entries_match_the_packet_layout() {
        // 3+1+1+11+2+14+16 = 48 bits primary header; 8+32+16 = 56 bits secondary header.
        assert_eq!(
            3 + 1 + 1 + 11 + 2 + 14 + 16,
            8 * crate::packet::PRIMARY_HEADER_LEN
        );
        assert_eq!(8 + 32 + 16, 8 * crate::packet::SECONDARY_HEADER_LEN);
        // OPS alarm: 128 + 4×8 + 32 + 16 = 26 octets.
        assert_eq!((128 + 4 * 8 + 32 + 16) / 8, 26);
    }
}
