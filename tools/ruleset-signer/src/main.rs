//! `ruleset-signer`: keys, signatures and verification for CAELUM alarm rulesets (CR-009, SR-009).
//!
//! ```text
//! ruleset-signer keygen --out <private-key-file>      # outside the repository
//! ruleset-signer pubkey --key <private-key-file>
//! ruleset-signer sign   --ruleset <ruleset.toml> --key <private-key-file> --role author|medical_approver
//! ruleset-signer verify --ruleset <ruleset.toml> [--policy development|flight]
//! ```
//!
//! `sign` writes `<ruleset>.sig.toml` next to the ruleset (`ruleset.sig.toml`), adding the
//! signature to an existing envelope for the same bytes, or starting a new one when the bytes
//! changed. Private keys are refused inside the repository.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use caelum_alarms::ruleset::Ruleset;
use caelum_alarms::signing::{
    built_in_keys, digest, hex, key_id, sign, verify, Envelope, SignerRole, TrustPolicy,
};
use ed25519_dalek::SigningKey;
use zeroize::Zeroizing;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn need(args: &[String], name: &str) -> Result<String, String> {
    arg(args, name).ok_or_else(|| format!("missing {name}"))
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn inside_repo(path: &Path) -> bool {
    let (Ok(repo), Some(parent)) = (repo_root().canonicalize(), path.parent()) else {
        return false;
    };
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    parent.canonicalize().is_ok_and(|p| p.starts_with(repo))
}

fn read_key(path: &str) -> Result<SigningKey, String> {
    let text = Zeroizing::new(std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?);
    let hexed = text.trim();
    if hexed.len() != 64 {
        return Err(format!("{path}: not a 32-byte hex key"));
    }
    let mut seed = Zeroizing::new([0u8; 32]);
    for (i, slot) in seed.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hexed[i * 2..i * 2 + 2], 16)
            .map_err(|_| format!("{path}: not hex"))?;
    }
    Ok(SigningKey::from_bytes(&seed))
}

fn sig_path(ruleset: &str) -> PathBuf {
    Path::new(ruleset).with_extension("sig.toml")
}

fn run(args: &[String]) -> Result<String, String> {
    match args.first().map(String::as_str) {
        Some("keygen") => {
            let out = PathBuf::from(need(args, "--out")?);
            if inside_repo(&out) {
                return Err("refusing to write a private key inside the repository".into());
            }
            if out.exists() {
                return Err(format!("{} exists; not overwriting a key", out.display()));
            }
            let mut seed = Zeroizing::new([0u8; 32]);
            getrandom::fill(seed.as_mut()).map_err(|e| e.to_string())?;
            let key = SigningKey::from_bytes(&seed);
            std::fs::write(&out, Zeroizing::new(hex(seed.as_ref()))).map_err(|e| e.to_string())?;
            Ok(format!(
                "private key written to {}\npublic key {}\nkey id {}",
                out.display(),
                hex(&key.verifying_key().to_bytes()),
                key_id(&key.verifying_key())
            ))
        }
        Some("pubkey") => {
            let key = read_key(&need(args, "--key")?)?;
            Ok(format!(
                "{}  # key id {}",
                hex(&key.verifying_key().to_bytes()),
                key_id(&key.verifying_key())
            ))
        }
        Some("sign") => {
            let ruleset_path = need(args, "--ruleset")?;
            let key = read_key(&need(args, "--key")?)?;
            let role = match need(args, "--role")?.as_str() {
                "author" => SignerRole::Author,
                "medical_approver" => SignerRole::MedicalApprover,
                other => return Err(format!("unknown role {other}")),
            };
            let bytes = std::fs::read(&ruleset_path).map_err(|e| e.to_string())?;
            let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
            let ruleset = Ruleset::parse(text).map_err(|e| e.to_string())?;
            let sha = digest(&bytes);
            let out = sig_path(&ruleset_path);
            let mut envelope = std::fs::read_to_string(&out)
                .ok()
                .and_then(|t| toml::from_str::<Envelope>(&t).ok())
                .filter(|e| e.sha256 == sha && e.ruleset_version == ruleset.version)
                .unwrap_or(Envelope {
                    ruleset_version: ruleset.version.clone(),
                    sha256: sha,
                    signatures: Vec::new(),
                });
            let entry = sign(&bytes, &ruleset.version, role, &key);
            envelope
                .signatures
                .retain(|s| s.key_id != entry.key_id || s.role != role);
            envelope.signatures.push(entry);
            let header = "# Signatures of ruleset.toml (CR-009). Written by tools/ruleset-signer; do not edit.\n";
            let body = toml::to_string(&envelope).map_err(|e| e.to_string())?;
            std::fs::write(&out, format!("{header}{body}")).map_err(|e| e.to_string())?;
            Ok(format!(
                "signed {} v{} → {}",
                ruleset_path,
                ruleset.version,
                out.display()
            ))
        }
        Some("verify") => {
            let ruleset_path = need(args, "--ruleset")?;
            let policy = match arg(args, "--policy").as_deref() {
                None | Some("development") => TrustPolicy::Development,
                Some("flight") => TrustPolicy::Flight,
                Some(other) => return Err(format!("unknown policy {other}")),
            };
            let bytes = std::fs::read(&ruleset_path).map_err(|e| e.to_string())?;
            let envelope =
                std::fs::read_to_string(sig_path(&ruleset_path)).map_err(|e| e.to_string())?;
            let v =
                verify(&bytes, &envelope, &built_in_keys(), policy).map_err(|e| e.to_string())?;
            Ok(format!(
                "verified v{} ({} rules){}",
                v.ruleset.version,
                v.ruleset.rules.len(),
                if v.development {
                    ", DEVELOPMENT RULESET"
                } else {
                    ""
                }
            ))
        }
        _ => Err("usage: ruleset-signer keygen|pubkey|sign|verify (see source header)".into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("ruleset-signer: {e}");
            ExitCode::FAILURE
        }
    }
}
