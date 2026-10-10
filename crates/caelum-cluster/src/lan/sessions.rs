//! User sessions of the crew LAN service (F-SYS-05, CLAUDE.md rule 6, CR-017 §5).
//!
//! A user signs in from a secondary station; the service issues a random 256-bit token bound to
//! that station's certificate identity. Only the SHA-256 of the token is kept. A token is refused
//! from any other station, after the idle timeout (default 10 min, the onboard auto-lock), after
//! its lifetime (default 12 h), after sign-out, and when the account is no longer active.
//! Sessions are not replicated: after a takeover users sign in again on the new primary.

use std::collections::HashMap;

use caelum_domain::ids::UserId;
use caelum_security::keys::{hex, random_bytes};
use caelum_security::role::UserIdentity;
use sha2::{Digest, Sha256};

use super::StationIdentity;
use crate::fence::NodeId;

/// Session timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionPolicy {
    /// Unused for this long, a session ends (ms). Default 10 min.
    pub idle_ms: u64,
    /// A session ends this long after sign-in regardless of use (ms). Default 12 h.
    pub lifetime_ms: u64,
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            idle_ms: 10 * 60 * 1000,
            lifetime_ms: 12 * 60 * 60 * 1000,
        }
    }
}

/// The operating system could not supply random bytes for a token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no random source for a session token")]
pub struct NoRandom;

#[derive(Debug, Clone)]
struct Session {
    station: NodeId,
    user: UserIdentity,
    created_ms: u64,
    last_used_ms: u64,
}

/// The live LAN sessions.
#[derive(Debug, Default)]
pub struct LanSessions {
    policy: SessionPolicy,
    by_hash: HashMap<[u8; 32], Session>,
}

fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

impl LanSessions {
    /// No sessions.
    pub fn new(policy: SessionPolicy) -> Self {
        Self {
            policy,
            by_hash: HashMap::new(),
        }
    }

    /// Opens a session for `user` on `station`; returns the token (64 hex characters).
    pub fn issue(
        &mut self,
        station: &StationIdentity,
        user: UserIdentity,
        now_ms: u64,
    ) -> Result<String, NoRandom> {
        let token = hex(&random_bytes::<32>().map_err(|_| NoRandom)?);
        self.by_hash.insert(
            token_hash(&token),
            Session {
                station: station.station_id.clone(),
                user,
                created_ms: now_ms,
                last_used_ms: now_ms,
            },
        );
        Ok(token)
    }

    /// The user of `token` presented by `station`, if the session is valid; refreshes its idle
    /// timer. Expired sessions are removed.
    pub fn resolve(
        &mut self,
        token: &str,
        station: &StationIdentity,
        now_ms: u64,
    ) -> Option<UserIdentity> {
        let key = token_hash(token);
        let session = self.by_hash.get_mut(&key)?;
        let expired = now_ms.saturating_sub(session.last_used_ms) >= self.policy.idle_ms
            || now_ms.saturating_sub(session.created_ms) >= self.policy.lifetime_ms;
        if expired {
            self.by_hash.remove(&key);
            return None;
        }
        if session.station != station.station_id {
            return None;
        }
        session.last_used_ms = now_ms;
        Some(session.user.clone())
    }

    /// Ends the session of `token`. Returns whether one existed.
    pub fn revoke(&mut self, token: &str) -> bool {
        self.by_hash.remove(&token_hash(token)).is_some()
    }

    /// Ends every session of `user` (account disabled). Returns how many.
    pub fn revoke_user(&mut self, user: UserId) -> usize {
        let before = self.by_hash.len();
        self.by_hash.retain(|_, s| s.user.user_id != user);
        before - self.by_hash.len()
    }

    /// Ends every session (this node demoted).
    pub fn clear(&mut self) {
        self.by_hash.clear();
    }

    /// Number of live sessions.
    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lan::StationKind;
    use caelum_security::role::Role;

    fn station(id: &str) -> StationIdentity {
        StationIdentity {
            station_id: NodeId::new(id).unwrap(),
            kind: StationKind::Secondary,
        }
    }

    fn user() -> UserIdentity {
        UserIdentity {
            user_id: "00000000-0000-4000-8000-000000000001".parse().unwrap(),
            username: "cmo".into(),
            role: Role::Cmo,
            crew_member_id: None,
        }
    }

    #[test]
    fn f_sys_05_token_is_bound_to_its_station_and_expires() {
        let mut s = LanSessions::new(SessionPolicy {
            idle_ms: 1_000,
            lifetime_ms: 5_000,
        });
        let token = s.issue(&station("tab-1"), user(), 0).unwrap();
        assert_eq!(token.len(), 64);
        assert!(
            s.resolve(&token, &station("tab-2"), 10).is_none(),
            "other station"
        );
        assert!(s.resolve(&token, &station("tab-1"), 900).is_some());
        assert!(
            s.resolve(&token, &station("tab-1"), 1_800).is_some(),
            "idle timer refreshed"
        );
        assert!(s
            .resolve("0".repeat(64).as_str(), &station("tab-1"), 1_900)
            .is_none());
        assert!(
            s.resolve(&token, &station("tab-1"), 2_800).is_none(),
            "idle timeout"
        );
        assert!(s.is_empty());

        let token = s.issue(&station("tab-1"), user(), 0).unwrap();
        for t in (500..5_000).step_by(500) {
            assert!(s.resolve(&token, &station("tab-1"), t).is_some());
        }
        assert!(
            s.resolve(&token, &station("tab-1"), 5_000).is_none(),
            "lifetime"
        );
    }

    #[test]
    fn f_sys_05_sign_out_and_account_disable_end_sessions() {
        let mut s = LanSessions::new(SessionPolicy::default());
        let a = s.issue(&station("tab-1"), user(), 0).unwrap();
        let b = s.issue(&station("tab-2"), user(), 0).unwrap();
        assert!(s.revoke(&a));
        assert!(!s.revoke(&a));
        assert!(s.resolve(&a, &station("tab-1"), 1).is_none());
        assert_eq!(s.revoke_user(user().user_id), 1);
        assert!(s.resolve(&b, &station("tab-2"), 1).is_none());
    }
}
