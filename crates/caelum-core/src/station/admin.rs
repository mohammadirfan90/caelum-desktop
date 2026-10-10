//! Crew roster and user administration (F-SYS-07, F-SYS-01; CR-012 §2–3, CR-007 §4). Every
//! change needs an administrator who re-authenticated within the last 2 minutes
//! (`docs/ROLE_MATRIX.md` §4) and is audited.

use caelum_alarms::signing::{built_in_keys, verify_config, TrustPolicy};
use caelum_domain::ids::{CrewMemberId, UserId};
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::{Command, Principal, Role, UserIdentity};
use caelum_security::session::AuthError;
use caelum_security::users::{create_user, NewUser};
use caelum_store::{CrewMemberRecord, UserRecord};
use serde::Deserialize;
use serde_json::json;

use super::{Station, StationError};

/// Configuration kind of the crew roster.
pub const CREW_KIND: &str = "crew";

/// The signed roster file (`crew.toml`, CR-012 §2).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RosterFile {
    roster: RosterHead,
    #[serde(default)]
    crew: Vec<RosterEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RosterHead {
    version: String,
    mission: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RosterEntry {
    id: String,
    name: String,
    position: String,
}

/// A roster that was applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterApplied {
    /// Roster version.
    pub version: String,
    /// Active crew members.
    pub members: usize,
    /// Signed with development keys only.
    pub development: bool,
}

/// An account as administrators and supervisors see it (never the credential).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSummary {
    /// Account ID.
    pub id: String,
    /// Sign-in name.
    pub username: String,
    /// Role.
    pub role: Role,
    /// Crew member (crew roles).
    pub crew_member_id: Option<String>,
    /// Cannot sign in.
    pub disabled: bool,
}

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

/// Parses and checks a roster's content against its signed version.
fn parse_roster(bytes: &[u8], signed_version: &str) -> Result<Vec<CrewMemberRecord>, StationError> {
    let text = std::str::from_utf8(bytes).map_err(|_| rejected("the roster is not UTF-8 text"))?;
    let file: RosterFile =
        toml::from_str(text).map_err(|e| rejected(format!("the roster does not parse: {e}")))?;
    if file.roster.version != signed_version {
        return Err(rejected(format!(
            "roster version {} is not the signed version {signed_version}",
            file.roster.version
        )));
    }
    if file.roster.mission.trim().is_empty() {
        return Err(rejected("the roster names no mission"));
    }
    if file.crew.is_empty() {
        return Err(rejected("the roster lists no crew member"));
    }
    let mut members: Vec<CrewMemberRecord> = Vec::with_capacity(file.crew.len());
    for entry in file.crew {
        let id = uuid::Uuid::parse_str(&entry.id)
            .map_err(|_| rejected(format!("crew ID {} is not a UUID", entry.id)))?
            .hyphenated()
            .to_string();
        let name = entry.name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(rejected(format!(
                "crew member {id} needs a name of 1 to 100 characters"
            )));
        }
        if members.iter().any(|m| m.id == id) {
            return Err(rejected(format!("crew ID {id} is listed twice")));
        }
        members.push(CrewMemberRecord {
            id,
            name: name.to_owned(),
            position: entry.position.trim().to_owned(),
            mission: file.roster.mission.trim().to_owned(),
            active: true,
        });
    }
    Ok(members)
}

impl Station {
    /// The signed-in, unlocked user allowed to run `command`.
    pub(super) fn user_for(&mut self, command: Command) -> Result<UserIdentity, StationError> {
        match self.authorize(command)? {
            Principal::User(user) => Ok(user),
            _ => Err(StationError::Auth(AuthError::NotSignedIn)),
        }
    }

    /// The user for an administrative change: authorised and re-authenticated (ROLE_MATRIX §4).
    pub(super) fn admin_for(&mut self, command: Command) -> Result<UserIdentity, StationError> {
        let user = self.user_for(command)?;
        let (sessions, log) = self.parts();
        sessions.require_recent_reauth(&log)?;
        Ok(user)
    }

    /// `crew_list`: the roster; an astronaut sees only their own entry. No health data.
    pub fn crew_list(&mut self) -> Result<Vec<CrewMemberRecord>, StationError> {
        let user = self.user_for(Command::CrewList)?;
        let all = self.db.crew_members()?;
        Ok(match user.role {
            Role::Astronaut => {
                let own = user.crew_member_id.map(|c| c.to_string());
                all.into_iter()
                    .filter(|c| Some(&c.id) == own.as_ref())
                    .collect()
            }
            _ => all,
        })
    }

    /// `config_apply_crew` (F-SYS-07): verifies a signed crew roster and makes it the roster in
    /// force. Members missing from it become inactive, never deleted. Audited.
    pub fn config_apply_crew(
        &mut self,
        bundle: &[u8],
        signature: &str,
    ) -> Result<RosterApplied, StationError> {
        let user = self.admin_for(Command::ConfigApplyCrew)?;
        let verified = verify_config(
            CREW_KIND,
            bundle,
            signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| rejected(format!("the roster signature is not accepted: {e}")))?;
        let members = parse_roster(bundle, &verified.version)?;
        if let Some((version, _, content)) = self.db.latest_config_bundle(CREW_KIND)? {
            if version == verified.version && content.as_bytes() != bundle {
                return Err(rejected(format!(
                    "roster version {version} is already in force with different content"
                )));
            }
        }
        let now = self.now()?;
        let content =
            std::str::from_utf8(bundle).map_err(|_| rejected("the roster is not UTF-8 text"))?;
        self.db.put_config_bundle(
            &format!("{CREW_KIND}-{}", verified.sha256),
            CREW_KIND,
            &verified.version,
            signature,
            content,
            now.0,
        )?;
        self.db.apply_crew_roster(&members)?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some(CREW_KIND),
            &json!({
                "kind": CREW_KIND,
                "version": verified.version,
                "sha256": verified.sha256,
                "members": members.len(),
                "development": verified.development,
            }),
        )?;
        Ok(RosterApplied {
            version: verified.version,
            members: members.len(),
            development: verified.development,
        })
    }

    /// `user_list`: every account for supervisors and administrators; only themselves for an
    /// astronaut or MCC (ROLE_MATRIX §2 "Users and roles"). Never credentials.
    pub fn user_list(&mut self) -> Result<Vec<UserSummary>, StationError> {
        let user = self.user_for(Command::UserList)?;
        let own_only = matches!(user.role, Role::Astronaut | Role::Mcc);
        let me = user.user_id.to_string();
        Ok(self
            .db
            .users()?
            .into_iter()
            .filter(|u| !own_only || u.id == me)
            .filter_map(summary)
            .collect())
    }

    /// `user_create` (CR-007 §4): a crew account must belong to an active roster member. Audited.
    pub fn user_create(
        &mut self,
        username: &str,
        role: Role,
        crew_member_id: Option<CrewMemberId>,
        secret: &str,
    ) -> Result<UserId, StationError> {
        let admin = self.admin_for(Command::UserCreate)?;
        if role.is_machine() {
            return Err(rejected(
                "machine identities are enrolled with keys, not here",
            ));
        }
        if let Some(crew) = crew_member_id {
            let active = self
                .db
                .crew_member(&crew.to_string())?
                .is_some_and(|c| c.active);
            if !active {
                return Err(rejected("the crew member is not on the roster in force"));
            }
        }
        if self
            .db
            .user_by_username(&username.to_lowercase())?
            .is_some()
        {
            return Err(rejected("this sign-in name is taken"));
        }
        let id = create_user(
            &self.log(),
            &Actor::user(&admin),
            NewUser {
                username,
                role,
                crew_member_id,
                secret,
            },
        )?;
        Ok(id)
    }

    /// `user_disable`: never the caller's own account. The caller is an active administrator, so
    /// this also means the last active administrator can never be disabled. Audited.
    pub fn user_disable(&mut self, id: UserId) -> Result<(), StationError> {
        let admin = self.admin_for(Command::UserDisable)?;
        if id == admin.user_id {
            return Err(rejected("you cannot disable your own account"));
        }
        let users = self.db.users()?;
        let target = users
            .iter()
            .find(|u| u.id == id.to_string())
            .ok_or_else(|| rejected("no such account"))?;
        self.db.set_user_disabled(&target.id, true)?;
        self.log().append(
            &Actor::user(&admin),
            AuditAction::UserDisabled,
            Some(&target.id),
            &json!({ "username": target.username }),
        )?;
        Ok(())
    }
}

fn summary(u: UserRecord) -> Option<UserSummary> {
    Some(UserSummary {
        role: Role::parse(&u.role)?,
        id: u.id,
        username: u.username,
        crew_member_id: u.crew_member_id,
        disabled: u.disabled,
    })
}
