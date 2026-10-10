//! Ruleset signatures (SR-009, CR-009 §2).
//!
//! `ruleset.sig.toml` holds the SHA-256 of the exact ruleset bytes, the ruleset version and
//! Ed25519 signatures over `"CAELUM-RULESET-1" ‖ sha256 ‖ version` by keys compiled into the
//! build. Development builds accept development keys (and show the DEVELOPMENT RULESET banner);
//! release builds accept only flight keys and require a medical approver signature.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ruleset::{Ruleset, RulesetError};

/// Domain separation for ruleset signatures.
const TAG: &[u8] = b"CAELUM-RULESET-1";

/// Development public keys (hex, one per line, `#` comments).
const DEV_KEYS: &str = include_str!("../keys/dev.pub");
/// Flight public keys. None exist until a licensed clinician advisor countersigns (CR-003).
const FLIGHT_KEYS: &str = include_str!("../keys/flight.pub");

/// Who signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignerRole {
    /// Engineering author.
    Author,
    /// Medical approver.
    MedicalApprover,
}

/// One signature in the envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureEntry {
    /// Role.
    pub role: SignerRole,
    /// Key ID: first 8 bytes of SHA-256 of the public key, hex.
    pub key_id: String,
    /// Ed25519 signature, hex.
    pub signature: String,
}

/// The `ruleset.sig.toml` envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    /// Ruleset version the signatures cover.
    pub ruleset_version: String,
    /// SHA-256 of the ruleset bytes, hex.
    pub sha256: String,
    /// Signatures.
    #[serde(rename = "signature", default)]
    pub signatures: Vec<SignatureEntry>,
}

/// Which keys a build trusts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustPolicy {
    /// Development builds: development or flight keys; DEVELOPMENT RULESET banner.
    Development,
    /// Release builds: flight keys only, medical approver required.
    Flight,
}

impl TrustPolicy {
    /// The policy of this build (`debug_assertions` = development build).
    pub const fn for_this_build() -> Self {
        if cfg!(debug_assertions) {
            Self::Development
        } else {
            Self::Flight
        }
    }
}

/// Class of a trusted key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyClass {
    /// Development key.
    Development,
    /// Flight key.
    Flight,
}

/// A trusted public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedKey {
    /// Class.
    pub class: KeyClass,
    /// The key.
    pub key: VerifyingKey,
}

/// A ruleset whose signatures verified.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedRuleset {
    /// The rules.
    pub ruleset: Ruleset,
    /// Signed with development keys only: show the DEVELOPMENT RULESET banner.
    pub development: bool,
    /// SHA-256 of the ruleset bytes (hex).
    pub sha256: String,
}

/// Why a ruleset was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    /// The envelope does not parse.
    #[error("signature file does not parse: {0}")]
    Envelope(String),
    /// The ruleset bytes are not the signed ones.
    #[error("ruleset digest does not match its signatures (tampered or not re-signed)")]
    DigestMismatch,
    /// The ruleset's version is not the signed one.
    #[error("ruleset version {found} is not the signed version {signed}")]
    VersionMismatch {
        /// In the ruleset.
        found: String,
        /// In the envelope.
        signed: String,
    },
    /// No signatures.
    #[error("ruleset is not signed")]
    Unsigned,
    /// A signature names a key this build does not trust.
    #[error("signature by untrusted key {0}")]
    UntrustedKey(String),
    /// A signature does not verify.
    #[error("signature by key {0} does not verify")]
    BadSignature(String),
    /// Release builds need a medical approver signature by a flight key.
    #[error("release builds need a medical approver signature by a flight key")]
    NoMedicalApproval,
    /// The rules themselves are invalid.
    #[error(transparent)]
    Ruleset(#[from] RulesetError),
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != N * 2
        || !text
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return None;
    }
    let mut out = [0u8; N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Key ID of a public key.
pub fn key_id(key: &VerifyingKey) -> String {
    hex(&Sha256::digest(key.to_bytes())[..8])
}

/// Parses a key list (hex public keys, one per line; `#` starts a comment).
pub fn parse_keys(text: &str, class: KeyClass) -> Vec<TrustedKey> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .filter_map(unhex::<32>)
        .filter_map(|b| VerifyingKey::from_bytes(&b).ok())
        .map(|key| TrustedKey { class, key })
        .collect()
}

/// The keys compiled into this build.
pub fn built_in_keys() -> Vec<TrustedKey> {
    let mut keys = parse_keys(DEV_KEYS, KeyClass::Development);
    keys.extend(parse_keys(FLIGHT_KEYS, KeyClass::Flight));
    keys
}

fn message(sha256: &str, version: &str) -> Vec<u8> {
    let mut m = TAG.to_vec();
    m.extend((sha256.len() as u64).to_be_bytes());
    m.extend(sha256.as_bytes());
    m.extend((version.len() as u64).to_be_bytes());
    m.extend(version.as_bytes());
    m
}

/// SHA-256 of ruleset bytes, hex.
pub fn digest(ruleset_bytes: &[u8]) -> String {
    hex(&Sha256::digest(ruleset_bytes))
}

/// Signs ruleset bytes (used by `tools/ruleset-signer`).
pub fn sign(
    ruleset_bytes: &[u8],
    version: &str,
    role: SignerRole,
    key: &SigningKey,
) -> SignatureEntry {
    let sha = digest(ruleset_bytes);
    SignatureEntry {
        role,
        key_id: key_id(&key.verifying_key()),
        signature: hex(&key.sign(&message(&sha, version)).to_bytes()),
    }
}

/// Verifies and parses a ruleset against `keys` under `policy`. Any failure rejects it.
pub fn verify(
    ruleset_bytes: &[u8],
    envelope_text: &str,
    keys: &[TrustedKey],
    policy: TrustPolicy,
) -> Result<VerifiedRuleset, VerifyError> {
    let envelope: Envelope =
        toml::from_str(envelope_text).map_err(|e| VerifyError::Envelope(e.to_string()))?;
    let sha = digest(ruleset_bytes);
    if sha != envelope.sha256 {
        return Err(VerifyError::DigestMismatch);
    }
    if envelope.signatures.is_empty() {
        return Err(VerifyError::Unsigned);
    }
    let msg = message(&envelope.sha256, &envelope.ruleset_version);
    let mut all_flight = true;
    let mut flight_medical = false;
    for s in &envelope.signatures {
        let key = keys
            .iter()
            .find(|k| key_id(&k.key) == s.key_id)
            .filter(|k| policy == TrustPolicy::Development || k.class == KeyClass::Flight)
            .ok_or_else(|| VerifyError::UntrustedKey(s.key_id.clone()))?;
        let sig =
            unhex::<64>(&s.signature).ok_or_else(|| VerifyError::BadSignature(s.key_id.clone()))?;
        key.key
            .verify_strict(&msg, &Signature::from_bytes(&sig))
            .map_err(|_| VerifyError::BadSignature(s.key_id.clone()))?;
        all_flight &= key.class == KeyClass::Flight;
        flight_medical |= key.class == KeyClass::Flight && s.role == SignerRole::MedicalApprover;
    }
    if policy == TrustPolicy::Flight && !flight_medical {
        return Err(VerifyError::NoMedicalApproval);
    }
    let text =
        std::str::from_utf8(ruleset_bytes).map_err(|e| RulesetError::Parse(e.to_string()))?;
    let ruleset = Ruleset::parse(text)?;
    if ruleset.version != envelope.ruleset_version {
        return Err(VerifyError::VersionMismatch {
            found: ruleset.version,
            signed: envelope.ruleset_version,
        });
    }
    Ok(VerifiedRuleset {
        ruleset,
        development: !(all_flight && flight_medical),
        sha256: sha,
    })
}
