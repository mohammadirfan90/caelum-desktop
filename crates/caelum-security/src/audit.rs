//! The signed, hash-chained audit log (F-SYS-03, R-SEC-03, docs/03 §5).
//!
//! Each entry's hash is `SHA-256(domain tag ‖ every field, length-prefixed ‖ previous hash)`, so
//! changing any field (time included), reordering, inserting or removing an entry breaks the chain.
//! The hash is signed with the instance Ed25519 key, so the chain cannot be rebuilt by someone who
//! can write the database but does not hold the key. The first entry links to [`GENESIS_HASH`].
//!
//! Removing entries from the end leaves a shorter valid chain; that is detected when the chain
//! head is compared with an earlier downlinked copy (M10), not by this check alone.

use caelum_domain::time::TaiNs;
use caelum_store::{AuditRecord, Database};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::clock::Clock;
use crate::keys::{hex, unhex, InstanceKey, InstanceVerifier};
use crate::role::{Role, UserIdentity};
use crate::SecurityError;

/// `prev_hash` of the first entry.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Domain separation for the entry hash (format version 1).
const HASH_TAG: &[u8] = b"CAELUM-AUDIT-1";

/// What an entry is about; decides who may read it (`docs/ROLE_MATRIX.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditCategory {
    /// Clinical actions (encounters, alarms, notes).
    Clinical,
    /// Configuration (bundles, rulesets, devices).
    Configuration,
    /// Security (sign-in, sessions, users, keys, break-glass).
    Security,
    /// Technical (devices, data quality, storage).
    Technical,
}

impl AuditCategory {
    /// Every category.
    pub const ALL: [AuditCategory; 4] = [
        AuditCategory::Clinical,
        AuditCategory::Configuration,
        AuditCategory::Security,
        AuditCategory::Technical,
    ];
}

/// Audited actions. Later milestones add clinical, configuration and technical actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditAction {
    /// Successful sign-in.
    Login,
    /// Refused sign-in (wrong secret, unknown name, disabled, rate-limited).
    LoginFailed,
    /// Sign-out (including a session replaced by another sign-in).
    Logout,
    /// Session locked after the idle timeout.
    AutoLock,
    /// Successful re-authentication (also unlocks a locked session).
    Reauth,
    /// Refused re-authentication.
    ReauthFailed,
    /// Emergency view opened without sign-in (SR-014).
    BreakGlassOpened,
    /// User account created.
    UserCreated,
    /// Key created in the keystore (new installation).
    KeyCreated,
    /// Alarm acknowledged onboard (F-ALM-05).
    AlarmAcknowledged,
    /// Ground escalation stage acknowledged by the flight surgeon.
    AlarmGroundAcknowledged,
    /// Alarm silenced (SR-008).
    AlarmSilenced,
    /// Alarm escalated by hand (F-ALM-05).
    AlarmEscalated,
    /// User account disabled (CR-012 §3).
    UserDisabled,
    /// Signed configuration bundle applied (F-SYS-07, CR-012 §2).
    ConfigApplied,
    /// Device paired and registered (F-ACQ-01).
    DevicePaired,
    /// Device assigned to a crew member (F-ACQ-09).
    DeviceAssigned,
    /// Device assignment removed (F-ACQ-09).
    DeviceUnassigned,
    /// Encounter created or updated before sign-off (F-CLN-05).
    EncounterSaved,
    /// Encounter signed (F-CLN-05).
    EncounterSigned,
    /// Daily check-in submitted; content never audited (F-CLN-07).
    CheckinSubmitted,
    /// PVT result stored (F-CLN-06).
    PvtRecorded,
    /// Medication or kit entry (F-CLN-08).
    MedicationLogged,
    /// Protocol checklist started (F-CLN-02).
    ChecklistStarted,
    /// Checklist step marked (F-CLN-02).
    ChecklistStep,
    /// Checklist completed (F-CLN-02).
    ChecklistCompleted,
    /// Attachment imported (F-CLN-04).
    AttachmentImported,
    /// Clinical data exported (F-SYS-08).
    ExportCreated,
}

impl AuditAction {
    /// Every action.
    pub const ALL: [AuditAction; 28] = [
        AuditAction::Login,
        AuditAction::LoginFailed,
        AuditAction::Logout,
        AuditAction::AutoLock,
        AuditAction::Reauth,
        AuditAction::ReauthFailed,
        AuditAction::BreakGlassOpened,
        AuditAction::UserCreated,
        AuditAction::KeyCreated,
        AuditAction::AlarmAcknowledged,
        AuditAction::AlarmGroundAcknowledged,
        AuditAction::AlarmSilenced,
        AuditAction::AlarmEscalated,
        AuditAction::UserDisabled,
        AuditAction::ConfigApplied,
        AuditAction::DevicePaired,
        AuditAction::DeviceAssigned,
        AuditAction::DeviceUnassigned,
        AuditAction::EncounterSaved,
        AuditAction::EncounterSigned,
        AuditAction::CheckinSubmitted,
        AuditAction::PvtRecorded,
        AuditAction::MedicationLogged,
        AuditAction::ChecklistStarted,
        AuditAction::ChecklistStep,
        AuditAction::ChecklistCompleted,
        AuditAction::AttachmentImported,
        AuditAction::ExportCreated,
    ];

    /// Stored name.
    pub const fn as_str(self) -> &'static str {
        match self {
            AuditAction::Login => "auth.login",
            AuditAction::LoginFailed => "auth.login_failed",
            AuditAction::Logout => "auth.logout",
            AuditAction::AutoLock => "auth.auto_lock",
            AuditAction::Reauth => "auth.reauth",
            AuditAction::ReauthFailed => "auth.reauth_failed",
            AuditAction::BreakGlassOpened => "break_glass.open",
            AuditAction::UserCreated => "user.create",
            AuditAction::KeyCreated => "key.create",
            AuditAction::AlarmAcknowledged => "alarm.acknowledge",
            AuditAction::AlarmGroundAcknowledged => "alarm.ground_acknowledge",
            AuditAction::AlarmSilenced => "alarm.silence",
            AuditAction::AlarmEscalated => "alarm.escalate",
            AuditAction::UserDisabled => "user.disable",
            AuditAction::ConfigApplied => "config.apply",
            AuditAction::DevicePaired => "device.pair",
            AuditAction::DeviceAssigned => "device.assign",
            AuditAction::DeviceUnassigned => "device.unassign",
            AuditAction::EncounterSaved => "encounter.save",
            AuditAction::EncounterSigned => "encounter.sign",
            AuditAction::CheckinSubmitted => "checkin.submit",
            AuditAction::PvtRecorded => "pvt.record",
            AuditAction::MedicationLogged => "medication.log",
            AuditAction::ChecklistStarted => "checklist.start",
            AuditAction::ChecklistStep => "checklist.step",
            AuditAction::ChecklistCompleted => "checklist.complete",
            AuditAction::AttachmentImported => "attachment.import",
            AuditAction::ExportCreated => "export.create",
        }
    }

    /// Parses a stored name.
    pub fn parse(name: &str) -> Option<AuditAction> {
        AuditAction::ALL.into_iter().find(|a| a.as_str() == name)
    }

    /// The category deciding who may read the entry.
    pub const fn category(self) -> AuditCategory {
        match self {
            AuditAction::Login
            | AuditAction::LoginFailed
            | AuditAction::Logout
            | AuditAction::AutoLock
            | AuditAction::Reauth
            | AuditAction::ReauthFailed
            | AuditAction::BreakGlassOpened
            | AuditAction::UserCreated
            | AuditAction::UserDisabled
            | AuditAction::KeyCreated => AuditCategory::Security,
            AuditAction::ConfigApplied => AuditCategory::Configuration,
            // Device work is technical: BME reads it (ROLE_MATRIX §2, "Audit log").
            AuditAction::DevicePaired
            | AuditAction::DeviceAssigned
            | AuditAction::DeviceUnassigned => AuditCategory::Technical,
            AuditAction::AlarmAcknowledged
            | AuditAction::AlarmGroundAcknowledged
            | AuditAction::AlarmSilenced
            | AuditAction::AlarmEscalated
            | AuditAction::EncounterSaved
            | AuditAction::EncounterSigned
            | AuditAction::CheckinSubmitted
            | AuditAction::PvtRecorded
            | AuditAction::MedicationLogged
            | AuditAction::ChecklistStarted
            | AuditAction::ChecklistStep
            | AuditAction::ChecklistCompleted
            | AuditAction::AttachmentImported
            | AuditAction::ExportCreated => AuditCategory::Clinical,
        }
    }
}

/// Who performed an audited action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// User ID, or a fixed name for actions without a user.
    pub id: String,
    /// Role name, or a fixed name for actions without a user.
    pub role: String,
}

impl Actor {
    /// A signed-in user.
    pub fn user(identity: &UserIdentity) -> Self {
        Self {
            id: identity.user_id.to_string(),
            role: identity.role.as_str().to_owned(),
        }
    }

    /// Someone attempting to sign in: the name typed (truncated) and the account's role if known.
    pub fn claimed(username: &str, role: Option<Role>) -> Self {
        Self {
            id: format!("name:{}", username.chars().take(64).collect::<String>()),
            role: role.map_or("unknown", Role::as_str).to_owned(),
        }
    }

    /// Break-glass use at a station (no sign-in).
    pub fn break_glass() -> Self {
        Self {
            id: "break-glass".to_owned(),
            role: "break_glass".to_owned(),
        }
    }

    /// CAELUM itself (installation, automatic locks).
    pub fn system() -> Self {
        Self {
            id: "system".to_owned(),
            role: "system".to_owned(),
        }
    }
}

/// One audit entry as read back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditEntry {
    /// Chain position.
    pub seq: i64,
    /// Entry ID.
    pub id: String,
    /// TAI nanoseconds.
    pub at_tai: TaiNs,
    /// Actor.
    pub actor: String,
    /// Actor's role.
    pub role: String,
    /// Action name.
    pub action: String,
    /// Target, if any.
    pub target: Option<String>,
    /// JSON details.
    pub details: String,
}

/// Result of checking the whole chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainStatus {
    /// Entries checked.
    pub entries: u64,
    /// Problems found, in chain order (empty: intact).
    pub problems: Vec<ChainProblem>,
}

impl ChainStatus {
    /// True if every link, hash and signature checked out.
    pub fn is_intact(&self) -> bool {
        self.problems.is_empty()
    }
}

/// A problem at one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainProblem {
    /// Sequence number of the entry.
    pub seq: i64,
    /// What is wrong.
    pub kind: ProblemKind,
}

/// Kinds of chain problems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind {
    /// Sequence numbers are not 1, 2, 3, … (an entry was removed or inserted).
    SequenceGap,
    /// `prev_hash` is not the previous entry's hash.
    BrokenLink,
    /// The stored hash does not match the entry's fields.
    HashMismatch,
    /// The signature is missing, malformed or not by the instance key.
    BadSignature,
}

/// Field input for the entry hash; also what verification recomputes.
struct HashInput<'a> {
    seq: i64,
    id: &'a str,
    at_tai: i64,
    actor: &'a str,
    role: &'a str,
    action: &'a str,
    target: Option<&'a str>,
    details: &'a str,
    prev_hash: &'a str,
}

impl HashInput<'_> {
    fn digest(&self) -> [u8; 32] {
        fn field(h: &mut Sha256, bytes: &[u8]) {
            h.update((bytes.len() as u64).to_be_bytes());
            h.update(bytes);
        }
        let mut h = Sha256::new();
        h.update(HASH_TAG);
        field(&mut h, &self.seq.to_be_bytes());
        field(&mut h, self.id.as_bytes());
        field(&mut h, &self.at_tai.to_be_bytes());
        field(&mut h, self.actor.as_bytes());
        field(&mut h, self.role.as_bytes());
        field(&mut h, self.action.as_bytes());
        match self.target {
            None => field(&mut h, &[0]),
            Some(t) => {
                field(&mut h, &[1]);
                field(&mut h, t.as_bytes());
            }
        }
        field(&mut h, self.details.as_bytes());
        field(&mut h, self.prev_hash.as_bytes());
        h.finalize().into()
    }

    fn of(r: &AuditRecord) -> HashInput<'_> {
        HashInput {
            seq: r.seq,
            id: &r.id,
            at_tai: r.at_tai,
            actor: &r.actor,
            role: &r.role,
            action: &r.action,
            target: r.target.as_deref(),
            details: &r.details,
            prev_hash: &r.prev_hash,
        }
    }
}

/// Writes and reads the audit log of one database.
pub struct AuditLog<'a> {
    db: &'a Database,
    key: &'a InstanceKey,
    clock: &'a dyn Clock,
}

impl<'a> AuditLog<'a> {
    /// The audit log of `db`, signed with `key`, timestamped by `clock`.
    pub fn new(db: &'a Database, key: &'a InstanceKey, clock: &'a dyn Clock) -> Self {
        Self { db, key, clock }
    }

    /// The database.
    pub fn db(&self) -> &'a Database {
        self.db
    }

    /// The clock.
    pub fn clock(&self) -> &'a dyn Clock {
        self.clock
    }

    /// Appends a signed entry. `details` is serialised as JSON (object keys in sorted order).
    pub fn append(
        &self,
        actor: &Actor,
        action: AuditAction,
        target: Option<&str>,
        details: &serde_json::Value,
    ) -> Result<AuditEntry, SecurityError> {
        let at_tai = self.clock.now_tai()?.0;
        let id = uuid::Uuid::new_v4().hyphenated().to_string();
        let details = serde_json::to_string(&sorted(details))
            .map_err(|e| SecurityError::Audit(e.to_string()))?;
        let record = self.db.append_audit(|head| {
            let (seq, prev_hash) = head.map_or((1, GENESIS_HASH.to_owned()), |h| {
                (h.seq + 1, h.hash.clone())
            });
            let digest = HashInput {
                seq,
                id: &id,
                at_tai,
                actor: &actor.id,
                role: &actor.role,
                action: action.as_str(),
                target,
                details: &details,
                prev_hash: &prev_hash,
            }
            .digest();
            AuditRecord {
                seq,
                id: id.clone(),
                at_tai,
                actor: actor.id.clone(),
                role: actor.role.clone(),
                action: action.as_str().to_owned(),
                target: target.map(str::to_owned),
                details: details.clone(),
                prev_hash,
                hash: hex(&digest),
                signature: hex(&self.key.sign(&digest)),
            }
        })?;
        Ok(entry_of(record))
    }

    /// Entries in the given categories, in chain order. Entries with an unknown action are shown
    /// only to readers of every category.
    pub fn list(&self, categories: &[AuditCategory]) -> Result<Vec<AuditEntry>, SecurityError> {
        let all = AuditCategory::ALL.iter().all(|c| categories.contains(c));
        Ok(self
            .db
            .audit_records()?
            .into_iter()
            .filter(|r| {
                AuditAction::parse(&r.action).map_or(all, |a| categories.contains(&a.category()))
            })
            .map(entry_of)
            .collect())
    }

    /// Checks every link, hash and signature against the instance key.
    pub fn verify(&self) -> Result<ChainStatus, SecurityError> {
        Ok(verify_records(
            &self.db.audit_records()?,
            &self.key.verifying_key(),
        ))
    }
}

/// The hash of a stored record, computed from its fields (what its `hash` and signature must
/// match). Also used to check downlinked copies of the chain on the ground.
pub fn record_hash(record: &AuditRecord) -> [u8; 32] {
    HashInput::of(record).digest()
}

/// Checks a sequence of records (chain order) against `verifier`.
pub fn verify_records(records: &[AuditRecord], verifier: &InstanceVerifier) -> ChainStatus {
    let mut problems = Vec::new();
    let mut prev_hash = GENESIS_HASH;
    for (i, r) in records.iter().enumerate() {
        let mut report = |kind| problems.push(ChainProblem { seq: r.seq, kind });
        if i64::try_from(i + 1).ok() != Some(r.seq) {
            report(ProblemKind::SequenceGap);
        }
        if r.prev_hash != prev_hash {
            report(ProblemKind::BrokenLink);
        }
        let digest = HashInput::of(r).digest();
        if r.hash != hex(&digest) {
            report(ProblemKind::HashMismatch);
        }
        let signed = unhex::<64>(&r.signature).is_some_and(|sig| verifier.verify(&digest, &sig));
        if !signed {
            report(ProblemKind::BadSignature);
        }
        prev_hash = &r.hash;
    }
    ChainStatus {
        entries: records.len() as u64,
        problems,
    }
}

fn entry_of(r: AuditRecord) -> AuditEntry {
    AuditEntry {
        seq: r.seq,
        id: r.id,
        at_tai: TaiNs(r.at_tai),
        actor: r.actor,
        role: r.role,
        action: r.action,
        target: r.target,
        details: r.details,
    }
}

/// A copy of `value` with object keys sorted, so equal details always serialise identically
/// regardless of how serde_json orders maps in this build.
fn sorted(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .filter_map(|k| map.get(k).map(|v| (k.clone(), sorted(v))))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_names_round_trip_and_are_unique() {
        for a in AuditAction::ALL {
            assert_eq!(AuditAction::parse(a.as_str()), Some(a));
        }
        let mut names: Vec<_> = AuditAction::ALL.iter().map(|a| a.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), AuditAction::ALL.len());
    }

    #[test]
    fn details_are_serialised_with_sorted_keys() {
        let v = serde_json::json!({"b": 1, "a": {"d": 2, "c": [ {"f": 3, "e": 4} ]}});
        let text = serde_json::to_string(&sorted(&v)).expect("json");
        assert_eq!(text, r#"{"a":{"c":[{"e":4,"f":3}],"d":2},"b":1}"#);
    }

    /// Length prefixes keep field boundaries: moving a character between fields changes the hash.
    #[test]
    fn hash_input_keeps_field_boundaries() {
        let base = HashInput {
            seq: 1,
            id: "id",
            at_tai: 5,
            actor: "ab",
            role: "c",
            action: "x",
            target: None,
            details: "{}",
            prev_hash: GENESIS_HASH,
        };
        let shifted = HashInput {
            actor: "a",
            role: "bc",
            ..base
        };
        let empty_target = HashInput {
            target: Some(""),
            ..base
        };
        assert_ne!(base.digest(), shifted.digest());
        assert_ne!(base.digest(), empty_target.digest());
    }
}
