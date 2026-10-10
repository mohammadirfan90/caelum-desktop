//! Ground console profile (F-GND-08, F-GND-04 gateway side; CR-025): a CAELUM installation at
//! ground receives the downlink: MEDICAL summaries, OPS records and CFDP files (event packets).
//! It stores them and serves them to the flight surgeon and BME, and to Open MCT through the
//! gateway in the shell. The flight surgeon keeps the private medical conference (PMC) record
//! here: drafted, then signed once and immutable.
//!
//! Everything shown is what was received, with its packet time; nothing is interpolated. A
//! parameter with no summary shows NO DATA.

use caelum_alarms::signing::{built_in_keys, verify_config, TrustPolicy};
use caelum_devices::files::format_utc;
use caelum_domain::ids::CrewMemberId;
use caelum_domain::observation::Quality;
use caelum_domain::parameters::ValueType;
use caelum_domain::time::{LeapSeconds, TaiNs};
use caelum_ground::cfdp::{CfdpConfig, CfdpEntity, CfdpEvent};
use caelum_ground::config::{ApidPlan, CfdpEntities};
use caelum_ground::medical::{MedicalSummary, SummaryValue};
use caelum_ground::ops::OpsRecord;
use caelum_ground::packet::ParsedTm;
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::Command;
use caelum_store::{GroundFileRecord, GroundSummaryRecord, PmcRecord};
use serde::Deserialize;
use serde_json::json;

use super::{Station, StationError};

/// Configuration kind of the ground console.
pub const CONSOLE_KIND: &str = "ground-console";
const NS: i64 = 1_000_000_000;

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

/// The signed `ground-console` bundle.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleConfig {
    /// Bundle version.
    pub version: String,
    /// UDP address on which downlinked packets arrive (from Yamcs or the agency MCS).
    pub receive: std::net::SocketAddr,
    /// UDP address to which CFDP acknowledgements go back toward the onboard instance.
    pub ack_to: std::net::SocketAddr,
    /// HTTP and WebSocket gateway for Open MCT (`/history`, `/ws`).
    pub gateway: std::net::SocketAddr,
    /// The mission's APID plan (as onboard).
    pub apids: ApidPlan,
    /// CFDP entity IDs (as onboard: `local_entity_id` is the onboard instance).
    pub cfdp: CfdpEntities,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsoleFile {
    console: ConsoleConfig,
}

impl ConsoleConfig {
    /// Parses and checks the bundle content.
    pub fn parse(text: &str) -> Result<Self, String> {
        let f: ConsoleFile = toml::from_str(text).map_err(|e| e.to_string())?;
        f.console.apids.validate().map_err(|e| e.to_string())?;
        if f.console.cfdp.local_entity_id == f.console.cfdp.ground_entity_id {
            return Err("CFDP entity IDs must differ".into());
        }
        Ok(f.console)
    }
}

fn quality_name(q: Quality) -> &'static str {
    match q {
        Quality::Good => "good",
        Quality::Degraded => "degraded",
        Quality::Poor => "poor",
        Quality::Unusable => "unusable",
    }
}

fn iso(leap: &LeapSeconds, t: TaiNs) -> Option<String> {
    leap.tai_to_utc(t).ok().map(|(u, _)| format_utc(u))
}

/// One OPS record as the Open MCT plugin's message (plugins/openmct/src/messages.ts).
fn ops_message(
    r: &OpsRecord,
    apid: u16,
    seq: u16,
    leap: &LeapSeconds,
) -> Option<(&'static str, i64, serde_json::Value)> {
    Some(match r {
        OpsRecord::Alarm(a) => (
            "ops_alarm",
            a.at.0,
            json!({
                "type": "ops_alarm", "crewId": a.crew_id.to_string(), "priority": a.priority.name(),
                "category": a.category.name(), "state": a.state.name(),
                "escalationStage": a.escalation_stage, "createdAt": iso(leap, a.created_at)?,
                "at": iso(leap, a.at)?, "apid": apid, "seq": seq,
            }),
        ),
        OpsRecord::CrewStatus(c) => (
            "crew_status",
            c.at.0,
            json!({
                "type": "crew_status", "crewId": c.crew_id.to_string(),
                "highestPriority": c.highest.map(|h| h.priority.name()),
                "category": c.highest.map(|h| h.category.name()),
                "state": c.highest.map(|h| h.state.name()),
                "at": iso(leap, c.at)?, "apid": apid, "seq": seq,
            }),
        ),
        OpsRecord::InstanceStatus(i) => (
            "instance_status",
            i.at.0,
            json!({
                "type": "instance_status", "health": i.health.name(), "mdsLink": i.mds_link.name(),
                "at": iso(leap, i.at)?, "apid": apid, "seq": seq,
            }),
        ),
    })
}

impl Station {
    /// `config_apply_console`: makes this installation a ground console (CR-025). Audited.
    pub fn config_apply_console(
        &mut self,
        bundle: &[u8],
        signature: &str,
    ) -> Result<String, StationError> {
        let user = self.admin_for(Command::ConfigApplyConsole)?;
        let verified = verify_config(
            CONSOLE_KIND,
            bundle,
            signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| {
            rejected(format!(
                "the ground-console configuration signature is not accepted: {e}"
            ))
        })?;
        let text = std::str::from_utf8(bundle).map_err(|_| rejected("not UTF-8 text"))?;
        let c = ConsoleConfig::parse(text)
            .map_err(|e| rejected(format!("the ground-console configuration is invalid: {e}")))?;
        if c.version != verified.version {
            return Err(rejected(
                "the configuration version is not the signed version",
            ));
        }
        let now = self.now()?;
        self.db.put_config_bundle(
            &format!("{CONSOLE_KIND}-{}", verified.sha256),
            CONSOLE_KIND,
            &verified.version,
            signature,
            text,
            now.0,
        )?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some(CONSOLE_KIND),
            &json!({ "kind": CONSOLE_KIND, "version": verified.version, "sha256": verified.sha256 }),
        )?;
        self.console_cfdp = None;
        Ok(verified.version)
    }

    /// The ground-console configuration in force (re-verified, SR-009), if this installation is one.
    pub fn console_config(&self) -> Result<Option<ConsoleConfig>, StationError> {
        let Some((version, signature, content)) = self.db.latest_config_bundle(CONSOLE_KIND)?
        else {
            return Ok(None);
        };
        verify_config(
            CONSOLE_KIND,
            content.as_bytes(),
            &signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| {
            rejected(format!(
                "the stored ground-console configuration no longer verifies: {e}"
            ))
        })?;
        let c = ConsoleConfig::parse(&content).map_err(rejected)?;
        Ok((c.version == version).then_some(c))
    }

    fn console_cfdp(&mut self, c: &ConsoleConfig) -> Result<&mut CfdpEntity, StationError> {
        if self.console_cfdp.is_none() {
            // Ground side: this entity is the ground one, the remote is the onboard instance.
            let cfg = CfdpConfig {
                local_entity: c.cfdp.ground_entity_id,
                remote_entity: c.cfdp.local_entity_id,
                segment_len: 1024,
                ack_timer_ns: 10 * NS,
                ack_limit: 3,
                nak_timer_ns: 10 * NS,
                nak_limit: 3,
            };
            self.console_cfdp = Some(CfdpEntity::new(cfg, 0).map_err(|e| rejected(e.to_string()))?);
        }
        self.console_cfdp.as_mut().ok_or_else(|| rejected("CFDP"))
    }

    /// One downlinked datagram (system intake, no user): Space Packets are parsed and refused if
    /// they are not telemetry. Summaries and OPS records are stored once, and CFDP files are
    /// assembled. Returns the new OPS messages for the Open MCT stream.
    pub fn console_ingest(
        &mut self,
        datagram: &[u8],
    ) -> Result<Vec<serde_json::Value>, StationError> {
        let Some(c) = self.console_config()? else {
            return Ok(Vec::new());
        };
        let now = self.now()?;
        let leap = LeapSeconds::bundled().map_err(|e| rejected(e.to_string()))?;
        let mut out = Vec::new();
        let mut rest = datagram;
        while !rest.is_empty() {
            let Ok((p, tail)) = ParsedTm::parse(rest) else {
                break;
            };
            rest = tail;
            self.console_packet(&c, &p, now, &leap, &mut out)?;
        }
        Ok(out)
    }

    fn console_packet(
        &mut self,
        c: &ConsoleConfig,
        p: &ParsedTm,
        now: TaiNs,
        leap: &LeapSeconds,
        out: &mut Vec<serde_json::Value>,
    ) -> Result<(), StationError> {
        if p.apid == c.apids.medical_summary {
            let Ok(s) = MedicalSummary::decode(p.time, &p.user_data) else {
                return Ok(());
            };
            let spec = s.parameter.spec();
            let (min, mean, max, state, count) = match s.value {
                SummaryValue::Numeric {
                    min,
                    mean,
                    max,
                    count,
                } => (
                    Some(f64::from(min)),
                    Some(f64::from(mean)),
                    Some(f64::from(max)),
                    None,
                    count,
                ),
                SummaryValue::State { last, count } => {
                    let name = match spec.value {
                        ValueType::Enum(states) => {
                            states.get(usize::from(last)).map(|s| (*s).to_owned())
                        }
                        _ => None,
                    };
                    (None, None, None, name, count)
                }
            };
            self.db.insert_ground_summary(
                &uuid::Uuid::new_v4().to_string(),
                &GroundSummaryRecord {
                    crew_member_id: s.crew_id.to_string(),
                    parameter: spec.key.to_owned(),
                    window_start: s.window_start.0,
                    window_end: s.window_end.0,
                    quality: quality_name(s.quality).to_owned(),
                    min_value: min,
                    mean_value: mean,
                    max_value: max,
                    state,
                    count: i64::from(count),
                },
                now.0,
            )?;
        } else if p.apid == c.apids.cfdp {
            let cfdp = self.console_cfdp(c)?;
            let _ = cfdp.receive(now, &p.user_data);
            for e in cfdp.take_events() {
                if let CfdpEvent::Received { name, data, .. } = e {
                    let kind = if name.starts_with("event-") {
                        "event_packet"
                    } else {
                        "file"
                    };
                    self.db.insert_ground_file(
                        &uuid::Uuid::new_v4().to_string(),
                        &name,
                        kind,
                        &data,
                        now.0,
                    )?;
                }
            }
            if let Some(cfdp) = self.console_cfdp.as_mut() {
                cfdp.prune();
            }
        } else if let Ok(r) = OpsRecord::decode(&c.apids, p.apid, p.time, &p.user_data) {
            if let Some((kind, at, msg)) = ops_message(&r, p.apid.value(), p.seq_count, leap) {
                let key = format!("{}:{}:{}", p.apid.value(), p.seq_count, p.time.0);
                if self
                    .db
                    .insert_ground_ops(&key, kind, at, &msg.to_string())?
                {
                    out.push(msg);
                }
            }
        }
        Ok(())
    }

    /// CFDP PDUs to send back toward the onboard instance (ACK, NAK, Finished).
    pub fn console_cfdp_poll(&mut self) -> Result<Vec<Vec<u8>>, StationError> {
        let Some(c) = self.console_config()? else {
            return Ok(Vec::new());
        };
        let now = self.now()?;
        let cfdp = self.console_cfdp(&c)?;
        cfdp.poll(now, 64).map_err(|e| rejected(e.to_string()))
    }

    /// OPS history for the Open MCT gateway (`/history`): OPS fields only, no physiology.
    pub fn console_history(
        &self,
        kind: &str,
        from: TaiNs,
        to: TaiNs,
    ) -> Result<Vec<serde_json::Value>, StationError> {
        if !["ops_alarm", "crew_status", "instance_status"].contains(&kind) {
            return Err(rejected("unknown history type"));
        }
        Ok(self
            .db
            .ground_ops(kind, from.0, to.0)?
            .iter()
            .filter_map(|m| serde_json::from_str(m).ok())
            .collect())
    }

    /// `console_summaries` (flight surgeon): downlinked summaries of a crew member over the last
    /// `window_s`.
    pub fn console_summaries(
        &mut self,
        crew: CrewMemberId,
        window_s: u32,
    ) -> Result<Vec<GroundSummaryRecord>, StationError> {
        self.user_for(Command::ConsoleSummaries)?;
        let now = self.now()?;
        Ok(self
            .db
            .ground_summaries(&crew.to_string(), now.0 - i64::from(window_s) * NS)?)
    }

    /// `console_files` (flight surgeon): received files, newest first.
    pub fn console_files(&mut self) -> Result<Vec<GroundFileRecord>, StationError> {
        self.user_for(Command::ConsoleFiles)?;
        Ok(self.db.ground_files()?)
    }

    /// `console_file` (flight surgeon): the content of a received file.
    pub fn console_file(&mut self, id: &str) -> Result<Vec<u8>, StationError> {
        self.user_for(Command::ConsoleFile)?;
        self.db
            .ground_file(id)?
            .ok_or_else(|| rejected("no such file"))
    }

    /// `console_ops` (flight surgeon, BME, MCC): OPS records of the last `window_s`.
    pub fn console_ops(
        &mut self,
        kind: &str,
        window_s: u32,
    ) -> Result<Vec<serde_json::Value>, StationError> {
        self.user_for(Command::ConsoleOps)?;
        let now = self.now()?;
        self.console_history(kind, TaiNs(now.0 - i64::from(window_s) * NS), now)
    }

    /// `pmc_save` (flight surgeon): drafts or edits a private medical conference record. Audited.
    pub fn pmc_save(
        &mut self,
        id: Option<&str>,
        crew: CrewMemberId,
        held_at: TaiNs,
        notes: &str,
    ) -> Result<String, StationError> {
        let user = self.user_for(Command::PmcSave)?;
        if notes.trim().is_empty() {
            return Err(rejected("the record is empty"));
        }
        let now = self.now()?;
        let record = match id {
            Some(id) => {
                let r = self
                    .db
                    .pmc_record(id)?
                    .ok_or_else(|| rejected("no such record"))?;
                if r.signed_at.is_some() {
                    return Err(rejected("a signed record is immutable"));
                }
                if r.author != user.user_id.to_string() {
                    return Err(rejected("only the author edits a draft"));
                }
                PmcRecord {
                    held_at: held_at.0,
                    notes: notes.to_owned(),
                    ..r
                }
            }
            None => PmcRecord {
                id: uuid::Uuid::new_v4().to_string(),
                crew_member_id: crew.to_string(),
                author: user.user_id.to_string(),
                held_at: held_at.0,
                notes: notes.to_owned(),
                created_at: now.0,
                signed_at: None,
            },
        };
        self.db.upsert_pmc(&record)?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::EncounterSaved,
            Some(&record.id),
            &json!({ "kind": "pmc" }),
        )?;
        Ok(record.id)
    }

    /// `pmc_sign` (the author, re-authentication): signs once; the record is then immutable.
    pub fn pmc_sign(&mut self, id: &str) -> Result<(), StationError> {
        let user = self.admin_for(Command::PmcSign)?;
        let mut r = self
            .db
            .pmc_record(id)?
            .ok_or_else(|| rejected("no such record"))?;
        if r.author != user.user_id.to_string() {
            return Err(rejected("only the author signs"));
        }
        if r.signed_at.is_some() {
            return Err(rejected("already signed"));
        }
        r.signed_at = Some(self.now()?.0);
        self.db.upsert_pmc(&r)?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::EncounterSigned,
            Some(id),
            &json!({ "kind": "pmc" }),
        )?;
        Ok(())
    }

    /// `pmc_list` (flight surgeon): a crew member's PMC records.
    pub fn pmc_list(&mut self, crew: CrewMemberId) -> Result<Vec<PmcRecord>, StationError> {
        self.user_for(Command::PmcList)?;
        Ok(self.db.pmc_records(&crew.to_string())?)
    }
}
