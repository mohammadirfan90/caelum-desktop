//! User accounts (F-SYS-01). Who may call this is decided at the command layer (administrator with
//! re-authentication, `docs/ROLE_MATRIX.md` §2); provisioning follows CR-007.

use caelum_domain::ids::{CrewMemberId, UserId};
use caelum_store::UserRecord;
use serde_json::json;

use crate::audit::{Actor, AuditAction, AuditLog};
use crate::credential::{check_policy, hash_secret};
use crate::role::Role;
use crate::SecurityError;

/// A new account.
#[derive(Debug, Clone, Copy)]
pub struct NewUser<'a> {
    /// Sign-in name: 1 to 64 characters `a-z 0-9 . _ -`, stored in lower case.
    pub username: &'a str,
    /// Role.
    pub role: Role,
    /// Crew member, required for onboard crew roles and refused for all others.
    pub crew_member_id: Option<CrewMemberId>,
    /// Initial PIN or password (checked against the policy for the role).
    pub secret: &'a str,
}

/// Creates an account and audits it. The secret is stored only as an argon2id hash.
pub fn create_user(
    log: &AuditLog<'_>,
    actor: &Actor,
    new: NewUser<'_>,
) -> Result<UserId, SecurityError> {
    let username = new.username.to_lowercase();
    let valid_name = (1..=64).contains(&username.len())
        && username
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
    if !valid_name {
        return Err(SecurityError::Policy(
            "a sign-in name has 1 to 64 characters: a-z, 0-9, '.', '_' or '-'".to_owned(),
        ));
    }
    match (new.role.is_onboard_crew(), new.crew_member_id) {
        (true, None) => {
            return Err(SecurityError::Policy(format!(
                "a {} account must belong to a crew member",
                new.role
            )))
        }
        (false, Some(_)) => {
            return Err(SecurityError::Policy(format!(
                "a {} account cannot belong to a crew member",
                new.role
            )))
        }
        _ => {}
    }
    check_policy(new.role, new.secret)?;
    let id = UserId::from_uuid(uuid::Uuid::new_v4());
    log.db().insert_user(&UserRecord {
        id: id.to_string(),
        username: username.clone(),
        role: new.role.as_str().to_owned(),
        crew_member_id: new.crew_member_id.map(|c| c.to_string()),
        credential: hash_secret(new.secret.as_bytes())?,
        key_id: None,
        disabled: false,
    })?;
    log.append(
        actor,
        AuditAction::UserCreated,
        Some(&id.to_string()),
        &json!({"username": username, "role": new.role.as_str()}),
    )?;
    Ok(id)
}
