//! Uplink of approved items only (F-GND-07, SR-009, R-SEC-04): rulesets, the protocol library,
//! model packages and configuration bundles arrive by CFDP with an Ed25519 signature envelope and
//! are verified with the same keys, envelopes and trust policy as the bundled ruleset
//! (`caelum_alarms::signing`, CR-009 §2, CR-012 §2).
//!
//! - Rulesets: `signing::verify` (release builds: flight keys with a medical approver).
//! - Protocol library and model packages: configuration envelope of their kind; release builds
//!   additionally require a medical approver signature (clinical content, CR-003).
//! - Configuration bundles (`ground`, `contact-schedule`, `crew`, ...): configuration envelope.
//! - No downgrade: a version must be newer than the installed one; the same version with other
//!   content is refused. The [`UplinkLedger`] records what is applied and keeps the previous
//!   version for rollback (docs/03 §7). Applying the content is the core's job.

use std::collections::BTreeMap;

use caelum_alarms::signing::{
    self, ConfigEnvelope, SignerRole, TrustPolicy, TrustedKey, VerifiedRuleset,
};
use serde::{Deserialize, Serialize};

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    signing::hex(bytes)
}

/// Parses lower-case or upper-case hex.
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| {
            text.get(i..i + 2)
                .and_then(|b| u8::from_str_radix(b, 16).ok())
        })
        .collect()
}

/// What an uplinked file is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum UplinkKind {
    /// Alarm ruleset.
    Ruleset,
    /// Protocol library (checklists).
    Protocols,
    /// Model package.
    Model,
    /// Configuration bundle of a named kind.
    Config(String),
}

impl UplinkKind {
    /// From the CFDP destination path `uplink/<kind>/<file>` (`<kind>` = `ruleset`, `protocols`,
    /// `model` or `config-<bundle kind>`).
    pub fn from_path(path: &str) -> Option<Self> {
        let mut parts = path.split('/');
        if parts.next()? != "uplink" {
            return None;
        }
        let kind = parts.next()?;
        parts.next().filter(|f| !f.is_empty())?;
        if parts.next().is_some() {
            return None;
        }
        match kind {
            "ruleset" => Some(Self::Ruleset),
            "protocols" => Some(Self::Protocols),
            "model" => Some(Self::Model),
            k => k
                .strip_prefix("config-")
                .filter(|b| !b.is_empty())
                .map(|b| Self::Config(b.to_owned())),
        }
    }

    fn bundle_kind(&self) -> String {
        match self {
            Self::Ruleset => "ruleset".into(),
            Self::Protocols => "protocols".into(),
            Self::Model => "model".into(),
            Self::Config(k) => k.clone(),
        }
    }
}

/// Uplink refusals.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UplinkError {
    /// Ruleset verification failed.
    #[error(transparent)]
    Ruleset(#[from] signing::VerifyError),
    /// Bundle verification failed.
    #[error(transparent)]
    Bundle(#[from] signing::ConfigVerifyError),
    /// Clinical content without a medical approver (release builds).
    #[error("{0:?} needs a medical approver signature by a flight key")]
    NoMedicalApproval(UplinkKind),
    /// Version not newer than the installed one.
    #[error("version {offered} is not newer than installed {installed}")]
    Downgrade {
        /// Offered.
        offered: String,
        /// Installed.
        installed: String,
    },
    /// Same version, other content.
    #[error("version {0} is installed with different content")]
    SameVersionDifferentContent(String),
    /// Nothing to roll back to.
    #[error("no previous version of {0:?}")]
    NoPrevious(UplinkKind),
}

/// A verified uplink item, ready for the core to apply.
#[derive(Debug, Clone, PartialEq)]
pub enum VerifiedUplink {
    /// A ruleset (parsed).
    Ruleset(VerifiedRuleset),
    /// Any other bundle: content for its owner to parse.
    Bundle {
        /// Kind.
        kind: UplinkKind,
        /// Signed version.
        version: String,
        /// SHA-256 (hex).
        sha256: String,
        /// Development-signed.
        development: bool,
        /// Content.
        content: Vec<u8>,
    },
}

impl VerifiedUplink {
    /// Kind.
    pub fn kind(&self) -> UplinkKind {
        match self {
            Self::Ruleset(_) => UplinkKind::Ruleset,
            Self::Bundle { kind, .. } => kind.clone(),
        }
    }

    /// Version.
    pub fn version(&self) -> &str {
        match self {
            Self::Ruleset(r) => &r.ruleset.version,
            Self::Bundle { version, .. } => version,
        }
    }

    /// Digest.
    pub fn sha256(&self) -> &str {
        match self {
            Self::Ruleset(r) => &r.sha256,
            Self::Bundle { sha256, .. } => sha256,
        }
    }
}

/// Verifies an uplinked file and its envelope.
pub fn verify_uplink(
    kind: &UplinkKind,
    content: &[u8],
    envelope: &str,
    keys: &[TrustedKey],
    policy: TrustPolicy,
) -> Result<VerifiedUplink, UplinkError> {
    if *kind == UplinkKind::Ruleset {
        return Ok(VerifiedUplink::Ruleset(signing::verify(
            content, envelope, keys, policy,
        )?));
    }
    let bundle = kind.bundle_kind();
    let v = signing::verify_config(&bundle, content, envelope, keys, policy)?;
    if policy == TrustPolicy::Flight && matches!(kind, UplinkKind::Protocols | UplinkKind::Model) {
        // verify_config checked every signature; a medical approver entry is therefore genuine.
        let env: ConfigEnvelope = toml::from_str(envelope)
            .map_err(|e| signing::ConfigVerifyError::Envelope(e.to_string()))?;
        if !env
            .signatures
            .iter()
            .any(|s| s.role == SignerRole::MedicalApprover)
        {
            return Err(UplinkError::NoMedicalApproval(kind.clone()));
        }
    }
    Ok(VerifiedUplink::Bundle {
        kind: kind.clone(),
        version: v.version,
        sha256: v.sha256,
        development: v.development,
        content: content.to_vec(),
    })
}

/// Compares dotted numeric versions; `None` when either is not dotted numeric.
fn newer(offered: &str, installed: &str) -> Option<bool> {
    let parse = |v: &str| -> Option<Vec<u64>> { v.split('.').map(|p| p.parse().ok()).collect() };
    let (a, b) = (parse(offered)?, parse(installed)?);
    Some(a > b)
}

/// One installed item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    /// Version.
    pub version: String,
    /// SHA-256 (hex).
    pub sha256: String,
}

/// Installed versions with the previous one kept for rollback (persisted by the core).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UplinkLedger {
    current: BTreeMap<UplinkKind, Installed>,
    previous: BTreeMap<UplinkKind, Installed>,
}

impl UplinkLedger {
    /// The installed version of a kind.
    pub fn installed(&self, kind: &UplinkKind) -> Option<&Installed> {
        self.current.get(kind)
    }

    /// Records a verified item as applied, refusing downgrades. Returns `false` when the same
    /// version and content are already installed (idempotent re-delivery).
    pub fn apply(&mut self, item: &VerifiedUplink) -> Result<bool, UplinkError> {
        let kind = item.kind();
        if let Some(cur) = self.current.get(&kind) {
            if cur.version == item.version() {
                return if cur.sha256 == item.sha256() {
                    Ok(false)
                } else {
                    Err(UplinkError::SameVersionDifferentContent(
                        cur.version.clone(),
                    ))
                };
            }
            if newer(item.version(), &cur.version) != Some(true) {
                return Err(UplinkError::Downgrade {
                    offered: item.version().to_owned(),
                    installed: cur.version.clone(),
                });
            }
        }
        let new = Installed {
            version: item.version().to_owned(),
            sha256: item.sha256().to_owned(),
        };
        if let Some(old) = self.current.insert(kind.clone(), new) {
            self.previous.insert(kind, old);
        }
        Ok(true)
    }

    /// Rolls back to the previous version (the core re-applies that content).
    pub fn rollback(&mut self, kind: &UplinkKind) -> Result<Installed, UplinkError> {
        let prev = self
            .previous
            .remove(kind)
            .ok_or_else(|| UplinkError::NoPrevious(kind.clone()))?;
        self.current.insert(kind.clone(), prev.clone());
        Ok(prev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelum_alarms::signing::{digest, sign, sign_config, KeyClass};
    use ed25519_dalek::SigningKey;

    fn trusted(keys: &[(&SigningKey, KeyClass)]) -> Vec<TrustedKey> {
        keys.iter()
            .map(|(k, class)| TrustedKey {
                class: *class,
                key: k.verifying_key(),
            })
            .collect()
    }

    fn config_env(
        kind: &str,
        content: &[u8],
        version: &str,
        signers: &[(&SigningKey, &str)],
    ) -> String {
        let mut env = format!(
            "kind = \"{kind}\"\nversion = \"{version}\"\nsha256 = \"{}\"\n",
            digest(content)
        );
        for (k, role) in signers {
            let s = sign_config(kind, content, version, k);
            env.push_str(&format!(
                "[[signature]]\nrole = \"{role}\"\nkey_id = \"{}\"\nsignature = \"{}\"\n",
                s.key_id, s.signature
            ));
        }
        env
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(
            unhex(&hex(&[0, 1, 0xAB, 0xFF])),
            Some(vec![0, 1, 0xAB, 0xFF])
        );
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
    }

    #[test]
    fn f_gnd_07_paths_map_to_kinds() {
        assert_eq!(
            UplinkKind::from_path("uplink/ruleset/ruleset.toml"),
            Some(UplinkKind::Ruleset)
        );
        assert_eq!(
            UplinkKind::from_path("uplink/config-contact-schedule/s.toml"),
            Some(UplinkKind::Config("contact-schedule".into()))
        );
        assert_eq!(UplinkKind::from_path("uplink/firmware/x.bin"), None);
        assert_eq!(UplinkKind::from_path("downlink/ruleset/x"), None);
        assert_eq!(UplinkKind::from_path("uplink/model/"), None);
        assert_eq!(UplinkKind::from_path("uplink/config-/x"), None);
    }

    /// F-GND-07, SR-009: the bundled ruleset verifies as an uplink with the built-in development
    /// key in development builds; a tampered copy is refused.
    #[test]
    fn f_gnd_07_ruleset_uplink_reuses_ruleset_verification() {
        let keys = signing::built_in_keys();
        let ok = verify_uplink(
            &UplinkKind::Ruleset,
            caelum_alarms::BUNDLED_RULESET.as_bytes(),
            caelum_alarms::BUNDLED_SIGNATURES,
            &keys,
            TrustPolicy::Development,
        )
        .expect("bundled ruleset verifies");
        assert_eq!(ok.kind(), UplinkKind::Ruleset);
        let tampered = caelum_alarms::BUNDLED_RULESET.replacen('9', "8", 1);
        assert!(verify_uplink(
            &UplinkKind::Ruleset,
            tampered.as_bytes(),
            caelum_alarms::BUNDLED_SIGNATURES,
            &keys,
            TrustPolicy::Development
        )
        .is_err());
        // Release builds refuse a development-signed ruleset.
        assert!(verify_uplink(
            &UplinkKind::Ruleset,
            caelum_alarms::BUNDLED_RULESET.as_bytes(),
            caelum_alarms::BUNDLED_SIGNATURES,
            &keys,
            TrustPolicy::Flight
        )
        .is_err());
        let _ = sign; // ruleset signing is exercised by caelum-alarms
    }

    /// F-GND-07: model packages need a flight medical approver in release builds.
    #[test]
    fn f_gnd_07_models_need_medical_approval_in_flight() {
        let author = SigningKey::from_bytes(&[3; 32]);
        let medic = SigningKey::from_bytes(&[4; 32]);
        let keys = trusted(&[(&author, KeyClass::Flight), (&medic, KeyClass::Flight)]);
        let content = b"model package bytes";
        let author_only = config_env("model", content, "2", &[(&author, "author")]);
        assert!(matches!(
            verify_uplink(
                &UplinkKind::Model,
                content,
                &author_only,
                &keys,
                TrustPolicy::Flight
            ),
            Err(UplinkError::NoMedicalApproval(_))
        ));
        let both = config_env(
            "model",
            content,
            "2",
            &[(&author, "author"), (&medic, "medical_approver")],
        );
        let v = verify_uplink(
            &UplinkKind::Model,
            content,
            &both,
            &keys,
            TrustPolicy::Flight,
        )
        .expect("approved");
        assert_eq!(v.version(), "2");
        // A model envelope does not verify as protocols.
        assert!(verify_uplink(
            &UplinkKind::Protocols,
            content,
            &both,
            &keys,
            TrustPolicy::Flight
        )
        .is_err());
        // Untrusted signer.
        let stranger = SigningKey::from_bytes(&[5; 32]);
        let bad = config_env("model", content, "2", &[(&stranger, "author")]);
        assert!(verify_uplink(
            &UplinkKind::Model,
            content,
            &bad,
            &keys,
            TrustPolicy::Development
        )
        .is_err());
    }

    /// F-GND-07: no downgrades, idempotent re-delivery, rollback to the previous version.
    #[test]
    fn f_gnd_07_ledger_refuses_downgrades_and_rolls_back() {
        let author = SigningKey::from_bytes(&[6; 32]);
        let keys = trusted(&[(&author, KeyClass::Development)]);
        let kind = UplinkKind::Config("ground".into());
        let item = |v: &str, body: &[u8]| {
            let env = config_env("ground", body, v, &[(&author, "author")]);
            verify_uplink(&kind, body, &env, &keys, TrustPolicy::Development).expect("verifies")
        };
        let mut l = UplinkLedger::default();
        assert_eq!(l.apply(&item("1.2", b"a")), Ok(true));
        assert_eq!(l.apply(&item("1.2", b"a")), Ok(false));
        assert!(matches!(
            l.apply(&item("1.2", b"b")),
            Err(UplinkError::SameVersionDifferentContent(_))
        ));
        assert!(matches!(
            l.apply(&item("1.1", b"c")),
            Err(UplinkError::Downgrade { .. })
        ));
        assert!(matches!(
            l.apply(&item("beta", b"c")),
            Err(UplinkError::Downgrade { .. })
        ));
        assert_eq!(l.apply(&item("1.10", b"d")), Ok(true));
        assert_eq!(l.installed(&kind).map(|i| i.version.as_str()), Some("1.10"));
        assert_eq!(l.rollback(&kind).map(|i| i.version), Ok("1.2".into()));
        assert!(l.rollback(&kind).is_err());
    }
}
