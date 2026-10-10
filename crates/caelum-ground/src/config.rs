//! Ground configuration from a signed bundle (F-SYS-07 channels and APIDs part, F-GND-01,
//! ICD-VEH §3/§5): the APID plan, the path B UDP endpoint (until ICD-VEH OI-1 is agreed), the
//! CFDP entity IDs and the contact-window schedule.
//!
//! Bundles use the configuration envelope of `caelum_alarms::signing` (CR-012 §2) with kind
//! [`GROUND_KIND`] or [`CONTACT_KIND`]. Nothing has a default: a missing field refuses the bundle.

use std::collections::BTreeSet;
use std::net::SocketAddr;

use caelum_alarms::signing::{verify_config, ConfigVerifyError, TrustPolicy, TrustedKey};
use caelum_domain::time::TaiNs;
use serde::Deserialize;

use crate::packet::Apid;

/// Configuration bundle kind of the ground configuration.
pub const GROUND_KIND: &str = "ground";
/// Configuration bundle kind of the contact-window schedule.
pub const CONTACT_KIND: &str = "contact-schedule";

/// Configuration errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// Signature verification failed.
    #[error(transparent)]
    Signature(#[from] ConfigVerifyError),
    /// The content does not parse or is incomplete.
    #[error("configuration does not parse: {0}")]
    Parse(String),
    /// The content is inconsistent.
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

/// The APID plan (F-GND-01). Every APID is distinct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApidPlan {
    /// OPS: CAELUM instance status (technical health).
    pub ops_instance_status: Apid,
    /// OPS: per-crew status.
    pub ops_crew_status: Apid,
    /// OPS: alarm state changes and ground escalations.
    pub ops_alarm: Apid,
    /// MEDICAL: parameter summaries.
    pub medical_summary: Apid,
    /// CFDP PDUs encapsulated in Space Packets.
    pub cfdp: Apid,
}

impl ApidPlan {
    /// Every APID of the plan.
    pub fn all(&self) -> [Apid; 5] {
        [
            self.ops_instance_status,
            self.ops_crew_status,
            self.ops_alarm,
            self.medical_summary,
            self.cfdp,
        ]
    }

    /// Checks that the APIDs are distinct.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let set: BTreeSet<Apid> = self.all().into_iter().collect();
        if set.len() != self.all().len() {
            return Err(ConfigError::Invalid("APIDs must be distinct".into()));
        }
        Ok(())
    }
}

/// Path B endpoint (ICD-VEH §3, OI-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathBConfig {
    /// UDP address and port that receive the OPS telemetry packets.
    pub endpoint: SocketAddr,
}

/// CFDP entity IDs (CCSDS 727.0-B-5 MIB).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CfdpEntities {
    /// This CAELUM instance.
    pub local_entity_id: u16,
    /// The ground CFDP entity.
    pub ground_entity_id: u16,
}

/// The ground downlink (MEDICAL channel and CFDP files) toward ground through the vehicle
/// (ICD-VEH OI-1: a configurable UDP endpoint until the vehicle interface is agreed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundLinkConfig {
    /// UDP address and port that receive the downlink packets.
    pub endpoint: SocketAddr,
    /// Local UDP address on which CFDP acknowledgements from ground arrive.
    pub receive: SocketAddr,
    /// MEDICAL summary window, s (F-GND-14), ≥ 60.
    pub summary_window_s: u32,
}

/// The ground configuration bundle content.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroundConfig {
    /// Bundle version.
    pub version: String,
    /// APID plan.
    pub apids: ApidPlan,
    /// Path B endpoint.
    pub path_b: PathBConfig,
    /// CFDP entities.
    pub cfdp: CfdpEntities,
    /// Ground downlink; absent when only path B is used.
    #[serde(default)]
    pub ground_link: Option<GroundLinkConfig>,
}

impl GroundConfig {
    /// Parses and validates unsigned content (tests and the signing tool).
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let c: Self = toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        c.apids.validate()?;
        if c.cfdp.local_entity_id == c.cfdp.ground_entity_id {
            return Err(ConfigError::Invalid("CFDP entity IDs must differ".into()));
        }
        if c.ground_link.is_some_and(|g| g.summary_window_s < 60) {
            return Err(ConfigError::Invalid(
                "summary_window_s must be at least 60".into(),
            ));
        }
        Ok(c)
    }

    /// Verifies the signature (kind [`GROUND_KIND`]) and parses the content.
    pub fn load_signed(
        bytes: &[u8],
        envelope: &str,
        keys: &[TrustedKey],
        policy: TrustPolicy,
    ) -> Result<Self, ConfigError> {
        let verified = verify_config(GROUND_KIND, bytes, envelope, keys, policy)?;
        let text = std::str::from_utf8(bytes).map_err(|e| ConfigError::Parse(e.to_string()))?;
        let c = Self::parse(text)?;
        if c.version != verified.version {
            return Err(ConfigError::Invalid(format!(
                "content version {} is not the signed version {}",
                c.version, verified.version
            )));
        }
        Ok(c)
    }
}

/// One contact window (acquisition to loss of signal), TAI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContactWindow {
    /// Acquisition of signal (TAI ns since 1958).
    pub start_tai_ns: i64,
    /// Loss of signal (TAI ns since 1958), exclusive.
    pub end_tai_ns: i64,
}

/// The contact-window schedule (F-GND-05, F-GND-06). Outside every window the link is down.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactSchedule {
    windows: Vec<ContactWindow>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScheduleFile {
    version: String,
    #[serde(rename = "window", default)]
    windows: Vec<ContactWindow>,
}

impl ContactSchedule {
    /// A schedule from windows; they must be non-empty intervals that do not overlap.
    pub fn new(mut windows: Vec<ContactWindow>) -> Result<Self, ConfigError> {
        windows.sort_by_key(|w| w.start_tai_ns);
        if windows.iter().any(|w| w.end_tai_ns <= w.start_tai_ns) {
            return Err(ConfigError::Invalid("empty contact window".into()));
        }
        if windows
            .windows(2)
            .any(|p| p[1].start_tai_ns < p[0].end_tai_ns)
        {
            return Err(ConfigError::Invalid("overlapping contact windows".into()));
        }
        Ok(Self { windows })
    }

    /// Verifies a signed schedule (kind [`CONTACT_KIND`]) and parses it.
    pub fn load_signed(
        bytes: &[u8],
        envelope: &str,
        keys: &[TrustedKey],
        policy: TrustPolicy,
    ) -> Result<Self, ConfigError> {
        let verified = verify_config(CONTACT_KIND, bytes, envelope, keys, policy)?;
        let text = std::str::from_utf8(bytes).map_err(|e| ConfigError::Parse(e.to_string()))?;
        let f: ScheduleFile =
            toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        if f.version != verified.version {
            return Err(ConfigError::Invalid(
                "schedule version is not the signed one".into(),
            ));
        }
        Self::new(f.windows)
    }

    /// Whether `t` is inside a contact window.
    pub fn in_contact(&self, t: TaiNs) -> bool {
        self.windows
            .iter()
            .any(|w| t.0 >= w.start_tai_ns && t.0 < w.end_tai_ns)
    }

    /// The next acquisition of signal after `t`, if one is scheduled.
    pub fn next_aos(&self, t: TaiNs) -> Option<TaiNs> {
        self.windows
            .iter()
            .find(|w| w.start_tai_ns > t.0)
            .map(|w| TaiNs(w.start_tai_ns))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use caelum_alarms::signing::{sign_config, KeyClass};
    use ed25519_dalek::SigningKey;

    pub(crate) const GROUND_TOML: &str = r#"
version = "1"
[apids]
ops_instance_status = 0x300
ops_crew_status = 0x301
ops_alarm = 0x302
medical_summary = 0x310
cfdp = 0x320
[path_b]
endpoint = "127.0.0.1:50300"
[cfdp]
local_entity_id = 10
ground_entity_id = 1
"#;

    fn envelope(kind: &str, bytes: &[u8], version: &str, key: &SigningKey) -> String {
        let sig = sign_config(kind, bytes, version, key);
        format!(
            "kind = \"{kind}\"\nversion = \"{version}\"\nsha256 = \"{}\"\n[[signature]]\nrole = \"author\"\nkey_id = \"{}\"\nsignature = \"{}\"\n",
            caelum_alarms::signing::digest(bytes),
            sig.key_id,
            sig.signature
        )
    }

    fn keys(key: &SigningKey) -> Vec<TrustedKey> {
        vec![TrustedKey {
            class: KeyClass::Development,
            key: key.verifying_key(),
        }]
    }

    /// F-GND-01, F-SYS-07: the APID plan comes from a signed bundle and is checked.
    #[test]
    fn f_gnd_01_signed_ground_config_loads() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let env = envelope(GROUND_KIND, GROUND_TOML.as_bytes(), "1", &key);
        let c = GroundConfig::load_signed(
            GROUND_TOML.as_bytes(),
            &env,
            &keys(&key),
            TrustPolicy::Development,
        )
        .expect("loads");
        assert_eq!(c.apids.ops_alarm.value(), 0x302);
        assert_eq!(c.path_b.endpoint.port(), 50300);
        // A development key is refused under the flight policy.
        assert!(GroundConfig::load_signed(
            GROUND_TOML.as_bytes(),
            &env,
            &keys(&key),
            TrustPolicy::Flight
        )
        .is_err());
        // Tampering is detected.
        let tampered = GROUND_TOML.replace("0x302", "0x303");
        assert!(GroundConfig::load_signed(
            tampered.as_bytes(),
            &env,
            &keys(&key),
            TrustPolicy::Development
        )
        .is_err());
    }

    #[test]
    fn duplicate_or_missing_apids_are_refused() {
        let dup = GROUND_TOML.replace("ops_alarm = 0x302", "ops_alarm = 0x301");
        assert!(matches!(
            GroundConfig::parse(&dup),
            Err(ConfigError::Invalid(_))
        ));
        let missing = GROUND_TOML.replace("cfdp = 0x320\n", "");
        assert!(matches!(
            GroundConfig::parse(&missing),
            Err(ConfigError::Parse(_))
        ));
        let idle = GROUND_TOML.replace("0x320", "0x7FF");
        assert!(GroundConfig::parse(&idle).is_err());
        let same_entity = GROUND_TOML.replace("ground_entity_id = 1", "ground_entity_id = 10");
        assert!(GroundConfig::parse(&same_entity).is_err());
    }

    #[test]
    fn contact_schedule_validates_and_answers() {
        let w = |a, b| ContactWindow {
            start_tai_ns: a,
            end_tai_ns: b,
        };
        assert!(ContactSchedule::new(vec![w(0, 10), w(5, 20)]).is_err());
        assert!(ContactSchedule::new(vec![w(10, 10)]).is_err());
        let s = ContactSchedule::new(vec![w(100, 200), w(0, 10)]).expect("valid");
        assert!(s.in_contact(TaiNs(0)));
        assert!(!s.in_contact(TaiNs(10)));
        assert!(s.in_contact(TaiNs(150)));
        assert_eq!(s.next_aos(TaiNs(10)), Some(TaiNs(100)));
        assert_eq!(s.next_aos(TaiNs(150)), None);
    }

    #[test]
    fn signed_contact_schedule_loads() {
        let key = SigningKey::from_bytes(&[9; 32]);
        let text = "version = \"3\"\n[[window]]\nstart_tai_ns = 0\nend_tai_ns = 10\n";
        let env = envelope(CONTACT_KIND, text.as_bytes(), "3", &key);
        let s = ContactSchedule::load_signed(
            text.as_bytes(),
            &env,
            &keys(&key),
            TrustPolicy::Development,
        )
        .expect("loads");
        assert!(s.in_contact(TaiNs(5)));
        // A ground-config signature does not verify as a schedule.
        let wrong = envelope(GROUND_KIND, text.as_bytes(), "3", &key);
        assert!(ContactSchedule::load_signed(
            text.as_bytes(),
            &wrong,
            &keys(&key),
            TrustPolicy::Development
        )
        .is_err());
    }
}
