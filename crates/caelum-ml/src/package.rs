//! Model package verification (F-ML-01, CR-018 §3).
//!
//! A package is a directory holding `manifest.toml`, its signature envelope `manifest.sig.toml`
//! and the ONNX file the manifest names. Verification, in order:
//! 1. the envelope verifies over the exact manifest bytes as a configuration bundle of kind
//!    [`MODEL_KIND`] (`caelum_alarms::signing::verify_config`: domain-separated Ed25519, author
//!    signature required, release builds accept flight keys only);
//! 2. the manifest parses and validates, and its version is the signed version;
//! 3. the SHA-256 of the model bytes equals the hash pinned in the signed manifest;
//! 4. release builds refuse `RESEARCH` packages, and a `VALIDATED` package needs a medical
//!    approver signature by a flight key (development builds show it as development-signed).
//!
//! Any failure rejects the package: [`VerifiedPackage`] can only be built here, and the registry
//! and runtimes accept nothing else, so an unverified model is never loaded.

use std::path::Path;

use caelum_alarms::signing::{
    self, ConfigEnvelope, ConfigVerifyError, SignerRole, TrustPolicy, TrustedKey,
};

use crate::manifest::{Manifest, ManifestError, ValidationStatus};

/// Signature kind of model packages (configuration-bundle envelope, CR-018 §3).
pub const MODEL_KIND: &str = "model";
/// Manifest file name in a package directory.
pub const MANIFEST_FILE: &str = "manifest.toml";
/// Signature envelope file name in a package directory.
pub const SIGNATURE_FILE: &str = "manifest.sig.toml";
/// Largest model file accepted, bytes.
pub const MAX_MODEL_BYTES: u64 = 256 * 1024 * 1024;

/// Why a package was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackageError {
    /// A file could not be read.
    #[error("cannot read {file}: {reason}")]
    Io {
        /// File.
        file: String,
        /// Reason.
        reason: String,
    },
    /// The signature envelope does not verify.
    #[error(transparent)]
    Signature(#[from] ConfigVerifyError),
    /// The manifest is invalid.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// The signed version is not the manifest's version.
    #[error("manifest version {found} is not the signed version {signed}")]
    VersionMismatch {
        /// In the manifest.
        found: String,
        /// In the envelope.
        signed: String,
    },
    /// The model bytes are not the pinned ones.
    #[error("model file digest {found} does not match the pinned {pinned} (tampered or replaced)")]
    ModelDigestMismatch {
        /// Pinned in the manifest.
        pinned: String,
        /// Of the bytes supplied.
        found: String,
    },
    /// The model file is too large.
    #[error("model file is larger than {MAX_MODEL_BYTES} bytes")]
    TooLarge,
    /// Release builds never load research models.
    #[error("research packages are not loaded by release builds")]
    ResearchInRelease,
    /// A VALIDATED package lacks a medical approver signature by a flight key.
    #[error("a VALIDATED package needs a medical approver signature by a flight key")]
    NoMedicalApproval,
}

/// A package whose signature, manifest and model hash verified. Only [`verify`] builds one.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedPackage {
    manifest: Manifest,
    model: Vec<u8>,
    manifest_sha256: String,
    development: bool,
}

impl VerifiedPackage {
    /// The verified manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The verified model bytes (their SHA-256 is the pinned one).
    pub fn model_bytes(&self) -> &[u8] {
        &self.model
    }

    /// SHA-256 of the signed manifest bytes (hex), for audit.
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    /// Signed with development keys only (shown in the UI).
    pub fn development(&self) -> bool {
        self.development
    }
}

/// Verifies a package from its bytes (sans-I/O). Any failure rejects it.
pub fn verify(
    manifest_bytes: &[u8],
    envelope_text: &str,
    model_bytes: &[u8],
    keys: &[TrustedKey],
    policy: TrustPolicy,
) -> Result<VerifiedPackage, PackageError> {
    let signed = signing::verify_config(MODEL_KIND, manifest_bytes, envelope_text, keys, policy)?;
    let text = std::str::from_utf8(manifest_bytes)
        .map_err(|e| ManifestError::Parse(format!("not UTF-8: {e}")))?;
    let manifest = Manifest::parse(text)?;
    if manifest.model.version != signed.version {
        return Err(PackageError::VersionMismatch {
            found: manifest.model.version,
            signed: signed.version,
        });
    }
    if model_bytes.len() as u64 > MAX_MODEL_BYTES {
        return Err(PackageError::TooLarge);
    }
    let found = signing::digest(model_bytes);
    if found != manifest.model.sha256 {
        return Err(PackageError::ModelDigestMismatch {
            pinned: manifest.model.sha256,
            found,
        });
    }
    match manifest.validation.status {
        ValidationStatus::Research if policy == TrustPolicy::Flight => {
            return Err(PackageError::ResearchInRelease)
        }
        ValidationStatus::Validated if policy == TrustPolicy::Flight => {
            // verify_config verified every signature in the envelope against flight keys; a
            // VALIDATED model additionally needs one by the medical approver.
            let envelope: ConfigEnvelope = toml::from_str(envelope_text)
                .map_err(|e| ConfigVerifyError::Envelope(e.to_string()))?;
            if !envelope
                .signatures
                .iter()
                .any(|s| s.role == SignerRole::MedicalApprover)
            {
                return Err(PackageError::NoMedicalApproval);
            }
        }
        _ => {}
    }
    Ok(VerifiedPackage {
        manifest,
        model: model_bytes.to_vec(),
        manifest_sha256: signed.sha256,
        development: signed.development,
    })
}

fn read(path: &Path) -> Result<Vec<u8>, PackageError> {
    std::fs::read(path).map_err(|e| PackageError::Io {
        file: path.display().to_string(),
        reason: e.to_string(),
    })
}

/// Reads and verifies a package directory.
pub fn load_dir(
    dir: &Path,
    keys: &[TrustedKey],
    policy: TrustPolicy,
) -> Result<VerifiedPackage, PackageError> {
    let manifest_bytes = read(&dir.join(MANIFEST_FILE))?;
    let envelope_path = dir.join(SIGNATURE_FILE);
    let envelope = String::from_utf8(read(&envelope_path)?).map_err(|e| PackageError::Io {
        file: envelope_path.display().to_string(),
        reason: e.to_string(),
    })?;
    // The file name comes from the (not yet verified) manifest; `Manifest::parse` refuses path
    // separators, and the signature is checked before the name is trusted for anything else.
    let text = std::str::from_utf8(&manifest_bytes)
        .map_err(|e| ManifestError::Parse(format!("not UTF-8: {e}")))?;
    let unverified = Manifest::parse(text)?;
    let model_path = dir.join(&unverified.model.file);
    let size = std::fs::metadata(&model_path)
        .map_err(|e| PackageError::Io {
            file: model_path.display().to_string(),
            reason: e.to_string(),
        })?
        .len();
    if size > MAX_MODEL_BYTES {
        return Err(PackageError::TooLarge);
    }
    let model = read(&model_path)?;
    verify(&manifest_bytes, &envelope, &model, keys, policy)
}
