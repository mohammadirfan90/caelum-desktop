//! Passwords and PINs: argon2id with fixed parameters (CR-006 §2), and the secret policy.

use std::sync::OnceLock;

use argon2::{Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version};

use crate::keys::random_bytes;
use crate::role::Role;
use crate::SecurityError;

/// argon2id memory cost in KiB (64 MiB).
pub const ARGON2_MEMORY_KIB: u32 = 64 * 1024;
/// argon2id iterations.
pub const ARGON2_ITERATIONS: u32 = 3;
/// argon2id lanes.
pub const ARGON2_LANES: u32 = 1;
/// Salt length in bytes.
pub const SALT_LEN: usize = 16;

/// Minimum PIN length for onboard crew (digits only).
pub const MIN_PIN_LEN: usize = 6;
/// Minimum password length for every other role.
pub const MIN_PASSWORD_LEN: usize = 12;
/// Upper bound on secret length, so a huge input cannot be used to waste memory or time.
pub const MAX_SECRET_LEN: usize = 256;

fn hasher() -> Result<Argon2<'static>, SecurityError> {
    let params = Params::new(ARGON2_MEMORY_KIB, ARGON2_ITERATIONS, ARGON2_LANES, None)
        .map_err(|e| SecurityError::Credential(e.to_string()))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

/// Hashes a secret into an argon2id PHC string with a fresh random salt.
pub fn hash_secret(secret: &[u8]) -> Result<String, SecurityError> {
    let salt: [u8; SALT_LEN] = random_bytes()?;
    let hash = hasher()?
        .hash_password_with_salt(secret, &salt)
        .map_err(|e| SecurityError::Credential(e.to_string()))?;
    Ok(hash.to_string())
}

/// Checks a secret against a stored PHC string. A malformed stored value never matches.
pub fn verify_secret(secret: &[u8], phc: &str) -> bool {
    if secret.len() > MAX_SECRET_LEN {
        return false;
    }
    hasher().is_ok_and(|h| h.verify_password(secret, phc).is_ok())
}

/// Spends the same work as a real check when the account does not exist, so response time does
/// not reveal which sign-in names exist.
pub fn verify_against_dummy(secret: &[u8]) {
    static DUMMY: OnceLock<Option<String>> = OnceLock::new();
    if let Some(phc) = DUMMY.get_or_init(|| hash_secret(b"caelum-no-such-account").ok()) {
        let _ = verify_secret(secret, phc);
    }
}

/// Checks a new secret against the policy for `role` (CR-007 §2): onboard crew use a PIN of at
/// least six digits; every other role a password of at least twelve characters.
pub fn check_policy(role: Role, secret: &str) -> Result<(), SecurityError> {
    if secret.len() > MAX_SECRET_LEN {
        return Err(SecurityError::Policy(format!(
            "secrets are limited to {MAX_SECRET_LEN} bytes"
        )));
    }
    if role.is_onboard_crew() {
        if secret.chars().count() < MIN_PIN_LEN || !secret.chars().all(|c| c.is_ascii_digit()) {
            return Err(SecurityError::Policy(format!(
                "a PIN has at least {MIN_PIN_LEN} digits and nothing else"
            )));
        }
    } else if secret.chars().count() < MIN_PASSWORD_LEN {
        return Err(SecurityError::Policy(format!(
            "a password has at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CR-006 §2: argon2id, version 19, m=64 MiB, t=3, p=1, 16-byte salt, PHC format.
    #[test]
    fn hashes_use_the_fixed_argon2id_parameters() {
        let phc = hash_secret(b"correct horse battery").expect("hash");
        assert!(phc.starts_with("$argon2id$v=19$m=65536,t=3,p=1$"), "{phc}");
        assert!(verify_secret(b"correct horse battery", &phc));
        assert!(!verify_secret(b"correct horse batterz", &phc));
        let again = hash_secret(b"correct horse battery").expect("hash");
        assert_ne!(phc, again, "each hash has its own salt");
    }

    #[test]
    fn malformed_or_oversized_input_never_matches() {
        assert!(!verify_secret(b"x", "not a phc string"));
        let phc = hash_secret(b"123456").expect("hash");
        assert!(!verify_secret(&[b'1'; MAX_SECRET_LEN + 1], &phc));
    }

    /// CR-007 §2: PIN for onboard crew, password for everyone else.
    #[test]
    fn secret_policy_depends_on_role() {
        assert!(check_policy(Role::Astronaut, "123456").is_ok());
        assert!(check_policy(Role::Cmo, "12345").is_err());
        assert!(check_policy(Role::Cmo, "12345a").is_err());
        assert!(check_policy(Role::FlightSurgeon, "short-pass").is_err());
        assert!(check_policy(Role::FlightSurgeon, "a long enough pass").is_ok());
        assert!(check_policy(Role::Administrator, &"x".repeat(MAX_SECRET_LEN + 1)).is_err());
    }
}
