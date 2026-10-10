//! The remaining M8 clinical-file items (CR-022): the DICOM folder watch (F-ACQ-06), ECG events
//! and segment export (F-CLN-03), and the check schedule (F-CLN-06, F-CLN-07).

use std::path::{Path, PathBuf};

use caelum_alarms::signing::{built_in_keys, verify_config, TrustPolicy};
use caelum_devices::files::edf::{
    self, EdfPlusPatient, EdfPlusRecordingId, EdfPlusWrite, EdfWriteSignal,
};
use caelum_devices::files::{dicom, CivilDateTime};
use caelum_domain::ids::CrewMemberId;
use caelum_domain::time::{LeapSeconds, TaiNs};
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::Command;
use serde::Deserialize;
use serde_json::json;

use super::{Station, StationError};

/// Configuration kind of the folder watch.
pub const IMPORT_KIND: &str = "import";
/// Configuration kind of the check schedule.
pub const SCHEDULE_KIND: &str = "schedule";
/// Longest exported ECG segment, s.
pub const MAX_SEGMENT_S: f64 = 600.0;
const NS: i64 = 1_000_000_000;
const DAY_NS: i64 = 86_400 * NS;

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

/// The signed `import` bundle: one watched folder and the crew member its files belong to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportConfig {
    /// Bundle version.
    pub version: String,
    /// The folder to watch: absolute, or relative to the station's data directory.
    pub watch_dir: String,
    /// The crew member every file in it belongs to (never guessed from file contents).
    pub crew_member_id: String,
}

/// The signed `schedule` bundle: daily check times, `HH:MM` UTC.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleConfig {
    /// Bundle version.
    pub version: String,
    /// Psychomotor vigilance test times.
    pub pvt: Vec<String>,
    /// Daily check-in times.
    pub checkin: Vec<String>,
}

/// The next due check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextCheck {
    /// `pvt` or `checkin`.
    pub kind: &'static str,
    /// When, TAI.
    pub at: TaiNs,
}

/// What one folder scan did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderScan {
    /// Files imported (moved to `imported/`).
    pub imported: Vec<String>,
    /// Files refused (moved to `refused/`), with the reason.
    pub refused: Vec<(String, String)>,
}

fn minutes_of(t: &str) -> Result<i64, String> {
    let (h, m) = t
        .split_once(':')
        .ok_or_else(|| format!("{t}: expected HH:MM"))?;
    let (h, m): (i64, i64) = (
        h.parse().map_err(|_| format!("{t}: hour"))?,
        m.parse().map_err(|_| format!("{t}: minute"))?,
    );
    if !(0..24).contains(&h) || !(0..60).contains(&m) || t.len() != 5 {
        return Err(format!("{t}: expected HH:MM"));
    }
    Ok(h * 60 + m)
}

impl ScheduleConfig {
    /// Parses and checks the bundle content.
    pub fn parse(text: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            schedule: ScheduleConfig,
        }
        let f: File = toml::from_str(text).map_err(|e| e.to_string())?;
        for t in f.schedule.pvt.iter().chain(&f.schedule.checkin) {
            minutes_of(t)?;
        }
        if f.schedule.pvt.is_empty() && f.schedule.checkin.is_empty() {
            return Err("at least one check time".into());
        }
        Ok(f.schedule)
    }

    /// The first check due after `now` (UTC times converted with the leap table).
    pub fn next(&self, now: TaiNs, leap: &LeapSeconds) -> Option<NextCheck> {
        let (utc, _) = leap.tai_to_utc(now).ok()?;
        let day = utc.unix_ns.div_euclid(DAY_NS) * DAY_NS;
        let mut best: Option<NextCheck> = None;
        for (kind, times) in [("pvt", &self.pvt), ("checkin", &self.checkin)] {
            for t in times {
                let Ok(m) = minutes_of(t) else { continue };
                for d in 0..2 {
                    let unix = day + d * DAY_NS + m * 60 * NS;
                    let Ok((tai, _)) = leap.utc_to_tai(unix) else {
                        continue;
                    };
                    if tai.0 > now.0 && best.as_ref().is_none_or(|b| tai.0 < b.at.0) {
                        best = Some(NextCheck { kind, at: tai });
                    }
                }
            }
        }
        best
    }
}

impl ImportConfig {
    /// Parses and checks the bundle content.
    pub fn parse(text: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            import: ImportConfig,
        }
        let f: File = toml::from_str(text).map_err(|e| e.to_string())?;
        if f.import.watch_dir.trim().is_empty() || f.import.watch_dir.contains("..") {
            return Err("watch_dir must be a path without `..`".into());
        }
        f.import
            .crew_member_id
            .parse::<CrewMemberId>()
            .map_err(|_| "crew_member_id is not a crew member ID".to_owned())?;
        Ok(f.import)
    }
}

impl Station {
    fn apply_bundle(
        &mut self,
        command: Command,
        kind: &str,
        bundle: &[u8],
        signature: &str,
        check: impl FnOnce(&str) -> Result<String, String>,
    ) -> Result<String, StationError> {
        let user = self.admin_for(command)?;
        let verified = verify_config(
            kind,
            bundle,
            signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| {
            rejected(format!(
                "the {kind} configuration signature is not accepted: {e}"
            ))
        })?;
        let text = std::str::from_utf8(bundle).map_err(|_| rejected("not UTF-8 text"))?;
        let version = check(text)
            .map_err(|e| rejected(format!("the {kind} configuration is invalid: {e}")))?;
        if version != verified.version {
            return Err(rejected(
                "the configuration version is not the signed version",
            ));
        }
        let now = self.now()?;
        self.db.put_config_bundle(
            &format!("{kind}-{}", verified.sha256),
            kind,
            &verified.version,
            signature,
            text,
            now.0,
        )?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some(kind),
            &json!({ "kind": kind, "version": verified.version, "sha256": verified.sha256 }),
        )?;
        Ok(verified.version)
    }

    fn bundle_in_force(&self, kind: &str) -> Result<Option<String>, StationError> {
        let Some((version, signature, content)) = self.db.latest_config_bundle(kind)? else {
            return Ok(None);
        };
        let v = verify_config(
            kind,
            content.as_bytes(),
            &signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| {
            rejected(format!(
                "the stored {kind} configuration no longer verifies: {e}"
            ))
        })?;
        Ok((v.version == version).then_some(content))
    }

    /// `config_apply_import`: the signed folder-watch configuration (F-ACQ-06). Audited.
    pub fn config_apply_import(
        &mut self,
        bundle: &[u8],
        signature: &str,
    ) -> Result<String, StationError> {
        self.apply_bundle(
            Command::ConfigApplyImport,
            IMPORT_KIND,
            bundle,
            signature,
            |t| ImportConfig::parse(t).map(|c| c.version),
        )
    }

    /// `config_apply_schedule`: the signed check schedule (F-CLN-06, F-CLN-07). Audited.
    pub fn config_apply_schedule(
        &mut self,
        bundle: &[u8],
        signature: &str,
    ) -> Result<String, StationError> {
        self.apply_bundle(
            Command::ConfigApplySchedule,
            SCHEDULE_KIND,
            bundle,
            signature,
            |t| ScheduleConfig::parse(t).map(|c| c.version),
        )
    }

    /// `schedule_next`: the next due check, or `None` without a signed schedule (NO DATA).
    pub fn schedule_next(&mut self) -> Result<Option<NextCheck>, StationError> {
        self.user_for(Command::ScheduleNext)?;
        let Some(text) = self.bundle_in_force(SCHEDULE_KIND)? else {
            return Ok(None);
        };
        let c = ScheduleConfig::parse(&text).map_err(rejected)?;
        let leap = LeapSeconds::bundled().map_err(|e| rejected(e.to_string()))?;
        Ok(c.next(self.now()?, &leap))
    }

    /// Folder watch (F-ACQ-06, CR-022 §2): every new `*.dcm` in the configured folder is imported
    /// through the normal path (it must open), filed to the configured crew member, audited as
    /// the system, then moved to `imported/` or `refused/`. Nothing is deleted.
    pub fn import_folder_scan(&mut self) -> Result<FolderScan, StationError> {
        let mut scan = FolderScan::default();
        let Some(text) = self.bundle_in_force(IMPORT_KIND)? else {
            return Ok(scan);
        };
        let c = ImportConfig::parse(&text).map_err(rejected)?;
        let crew: CrewMemberId = c
            .crew_member_id
            .parse()
            .map_err(|_| rejected("crew member"))?;
        let dir = if Path::new(&c.watch_dir).is_absolute() {
            PathBuf::from(&c.watch_dir)
        } else {
            self.data_dir.join(&c.watch_dir)
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(scan);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_dcm = path.is_file()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("dcm"));
            if !is_dcm {
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let outcome = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    dicom::read_file(&bytes)
                        .map_err(|e| format!("the DICOM file does not open: {e}"))?;
                    Ok(bytes)
                });
            let target = match outcome {
                Ok(bytes) => {
                    let metadata = json!({ "source": "folder_watch", "size": bytes.len() });
                    match self.attachment_store(
                        &Actor::system(),
                        crew,
                        "dicom",
                        &name,
                        &bytes,
                        metadata,
                    ) {
                        Ok(_) => {
                            scan.imported.push(name.clone());
                            "imported"
                        }
                        Err(e) => {
                            scan.refused.push((name.clone(), e.to_string()));
                            "refused"
                        }
                    }
                }
                Err(e) => {
                    scan.refused.push((name.clone(), e));
                    "refused"
                }
            };
            let to = dir.join(target);
            if std::fs::create_dir_all(&to).is_ok() {
                let _ = std::fs::rename(&path, to.join(&name));
            }
        }
        Ok(scan)
    }

    /// `ecg_export_segment` (F-CLN-03): `from_s`..`to_s` of an imported ECG recording as EDF+, with
    /// the original lead labels and units. Audited.
    pub fn ecg_export_segment(
        &mut self,
        id: &str,
        from_s: f64,
        to_s: f64,
    ) -> Result<Vec<u8>, StationError> {
        let user = self.user_for(Command::EcgExportSegment)?;
        let rec = self.ecg_view(id)?;
        if !(from_s.is_finite()
            && to_s.is_finite()
            && from_s >= 0.0
            && to_s > from_s
            && to_s - from_s <= MAX_SEGMENT_S)
        {
            return Err(rejected(
                "the segment must lie within the recording and last at most 10 min",
            ));
        }
        // EDF+ needs a start time; one the file does not state is never made up.
        let start = rec.start.ok_or_else(|| {
            rejected("the recording does not state its start time, which EDF+ needs")
        })?;
        let shifted = shift(start, from_s).map_err(rejected)?;
        let mut signals = Vec::new();
        for lead in &rec.leads {
            let fs = lead.sample_rate_hz;
            if fs.fract() != 0.0 {
                return Err(rejected("EDF+ export needs whole-number sample rates"));
            }
            let (a, b) = ((from_s * fs).round() as usize, (to_s * fs).round() as usize);
            if b > lead.samples.len() {
                return Err(rejected("the segment ends after the recording"));
            }
            signals.push(
                EdfWriteSignal::fitted(
                    &lead.label,
                    &lead.unit,
                    lead.samples[a..b].to_vec(),
                    fs as usize,
                )
                .map_err(|e| rejected(e.to_string()))?,
            );
        }
        let bytes = edf::write_edf_plus(&EdfPlusWrite {
            patient: EdfPlusPatient {
                code: None,
                sex: None,
                birthdate: None,
                name: None,
            },
            recording: EdfPlusRecordingId::default(),
            start: shifted,
            record_duration_s: 1.0,
            signals,
            annotations: Vec::new(),
        })
        .map_err(|e| rejected(e.to_string()))?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::ExportCreated,
            Some(id),
            &json!({ "format": "edf+", "from_s": from_s, "to_s": to_s }),
        )?;
        Ok(bytes)
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant, "chrono-compatible low-level
/// date algorithms").
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date of a day count since 1970-01-01 (inverse of [`days_from_civil`]).
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

/// `t` moved by `seconds` on its own clock (the UTC offset, if any, is kept).
fn shift(t: CivilDateTime, seconds: f64) -> Result<CivilDateTime, String> {
    let base = days_from_civil(i64::from(t.year), i64::from(t.month), i64::from(t.day)) * 86_400
        + i64::from(t.hour) * 3_600
        + i64::from(t.minute) * 60
        + i64::from(t.second);
    let total_ns = i128::from(base) * 1_000_000_000
        + i128::from(t.nanosecond)
        + (seconds * 1e9).round() as i128;
    let secs = i64::try_from(total_ns.div_euclid(1_000_000_000)).map_err(|e| e.to_string())?;
    let ns = u32::try_from(total_ns.rem_euclid(1_000_000_000)).map_err(|e| e.to_string())?;
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let sod = secs.rem_euclid(86_400);
    let n = |v: i64| u8::try_from(v).map_err(|e| e.to_string());
    CivilDateTime::new(
        i32::try_from(y).map_err(|e| e.to_string())?,
        n(mo)?,
        n(d)?,
        n(sod / 3_600)?,
        n(sod % 3_600 / 60)?,
        n(sod % 60)?,
        ns,
        t.utc_offset_minutes,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn f_cln_03_segment_start_moves_across_midnight_and_month_ends() {
        let t = CivilDateTime::new(2026, 2, 28, 23, 59, 30, 0, Some(0)).unwrap();
        let s = shift(t, 45.5).unwrap();
        assert_eq!(
            (
                s.year,
                s.month,
                s.day,
                s.hour,
                s.minute,
                s.second,
                s.nanosecond
            ),
            (2026, 3, 1, 0, 0, 15, 500_000_000)
        );
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
    }

    #[test]
    fn f_cln_06_the_next_check_is_the_first_time_after_now() {
        let leap = LeapSeconds::bundled().unwrap();
        let c = ScheduleConfig::parse(
            "[schedule]\nversion = \"1\"\npvt = [\"08:00\"]\ncheckin = [\"20:00\"]\n",
        )
        .unwrap();
        // 2026-10-10 12:00 UTC.
        let (now, _) = leap.utc_to_tai(1_791_633_600 * NS).unwrap();
        let next = c.next(now, &leap).unwrap();
        assert_eq!(next.kind, "checkin");
        let (utc, _) = leap.tai_to_utc(next.at).unwrap();
        assert_eq!(utc.unix_ns, (1_791_633_600 + 8 * 3_600) * NS);
        assert!(ScheduleConfig::parse(
            "[schedule]\nversion = \"1\"\npvt = [\"8:00\"]\ncheckin = []\n"
        )
        .is_err());
    }
}
