//! The sans-I/O fail-over state machine (F-SYS-04, R-OPS-02, `docs/03_ARCHITECTURE.md` §6).
//!
//! The caller feeds it time (monotonic milliseconds) and what it hears from the peer, and executes
//! the returned [`Action`]s in order. It does no I/O, so tests drive it with a deterministic clock.
//!
//! Rules:
//! - Every node starts as a standby and listens for one detection window before it may promote,
//!   so a restarted node never displaces a running primary.
//! - A standby that has heard nothing valid from its primary for the detection window promotes
//!   itself under a fence one epoch above the highest it has seen. The fence is persisted
//!   ([`Action::PersistFence`]) before the node acts as primary ([`Self::fence_persisted`]), so an
//!   epoch is never reused after a crash.
//! - A message carrying a lower fence than the one a node follows or holds is stale and refused;
//!   the caller answers it with the current fence, and a primary that learns of a higher fence
//!   demotes itself at once ([`Action::Demote`]). A stale primary therefore can never act on a
//!   node that has seen the newer epoch, and never gets an entry acknowledged.

use crate::fence::{Fence, NodeId};
use crate::timing::Timing;

/// The instance role, as shown in the status footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceRole {
    /// Follows a primary (or is waiting to hear one); read-only.
    Standby,
    /// Has decided to take over and is persisting its new fence.
    Promoting,
    /// Acts: runs alarms, owns devices, serves crew stations, replicates to the standby.
    Primary,
}

/// What the caller must do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Persist this fence as the highest seen. For the node's own promotion fence, call
    /// [`Failover::fence_persisted`] once it is durable; the node does not act before that.
    PersistFence(Fence),
    /// This node is now primary under `fence`: resume alarm escalation from the replicated
    /// record, start replicating to the peer and serving crew stations.
    BecomePrimary {
        /// The fence the node acts under.
        fence: Fence,
        /// When the last valid message from the previous primary arrived (ms), if any.
        silent_since_ms: u64,
        /// When the node became primary (ms).
        promoted_at_ms: u64,
    },
    /// Hand-off: reconnect the devices to this node by `deadline_ms`.
    ReconnectDevices {
        /// The fence the node acts under.
        fence: Fence,
        /// Device reconnection deadline (ms).
        deadline_ms: u64,
    },
    /// Stop acting at once: a higher fence exists. Release devices, stop escalating, stop serving
    /// crew stations; continue as a standby of `to`.
    Demote {
        /// The fence this node acted under.
        from: Fence,
        /// The newer fence.
        to: Fence,
    },
    /// Send a heartbeat under this fence to the peer.
    SendHeartbeat(Fence),
}

/// A message refused because its fence is older than the one this node holds or follows.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("stale fence {stale}; current is {current}")]
pub struct StaleFence {
    /// The refused fence.
    pub stale: Fence,
    /// The fence the sender must learn.
    pub current: Fence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Standby { primary: Option<Fence> },
    Promoting { fence: Fence, requested_ms: u64 },
    Primary { fence: Fence },
}

/// One node's fail-over state.
#[derive(Debug, Clone)]
pub struct Failover {
    me: NodeId,
    timing: Timing,
    state: State,
    highest_seen: Option<Fence>,
    last_heard_ms: u64,
    last_heartbeat_ms: Option<u64>,
}

impl Failover {
    /// A node starting at `now_ms` as a standby, with the highest fence it persisted earlier.
    pub fn new(me: NodeId, timing: Timing, persisted: Option<Fence>, now_ms: u64) -> Self {
        Self {
            me,
            timing,
            state: State::Standby { primary: None },
            highest_seen: persisted,
            last_heard_ms: now_ms,
            last_heartbeat_ms: None,
        }
    }

    /// This node.
    pub fn node(&self) -> &NodeId {
        &self.me
    }

    /// The timing in use.
    pub fn timing(&self) -> &Timing {
        &self.timing
    }

    /// The role.
    pub fn role(&self) -> InstanceRole {
        match self.state {
            State::Standby { .. } => InstanceRole::Standby,
            State::Promoting { .. } => InstanceRole::Promoting,
            State::Primary { .. } => InstanceRole::Primary,
        }
    }

    /// Whether this node may act as primary (F-SYS-04 fencing): only a primary whose fence has
    /// been persisted and who has seen no higher fence.
    pub fn may_act(&self) -> bool {
        matches!(self.state, State::Primary { .. })
    }

    /// The fence this node acts under, if primary.
    pub fn acting_fence(&self) -> Option<&Fence> {
        match &self.state {
            State::Primary { fence } => Some(fence),
            _ => None,
        }
    }

    /// The primary this node believes current: itself if primary, else the one it follows.
    pub fn current_primary(&self) -> Option<&Fence> {
        match &self.state {
            State::Primary { fence } => Some(fence),
            State::Standby { primary } => primary.as_ref(),
            State::Promoting { .. } => None,
        }
    }

    /// The highest fence this node has seen or taken.
    pub fn highest_seen(&self) -> Option<&Fence> {
        self.highest_seen.as_ref()
    }

    /// The fence a message must at least carry to be accepted.
    fn current(&self) -> Option<&Fence> {
        match &self.state {
            State::Primary { fence } | State::Promoting { fence, .. } => Some(fence),
            State::Standby { primary } => primary.as_ref().or(self.highest_seen.as_ref()),
        }
    }

    /// Checks a peer's fence without changing state: `Err` if it is stale.
    pub fn check(&self, fence: &Fence) -> Result<(), StaleFence> {
        match self.current() {
            Some(current) if fence < current => Err(StaleFence {
                stale: fence.clone(),
                current: current.clone(),
            }),
            _ => Ok(()),
        }
    }

    /// Advances time: the standby promotes after the detection window; the primary heartbeats.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Action> {
        match &self.state {
            State::Standby { .. } => {
                if now_ms.saturating_sub(self.last_heard_ms) >= self.timing.detection_ms() {
                    let epoch = self
                        .highest_seen
                        .as_ref()
                        .map_or(1, |f| f.epoch.saturating_add(1));
                    let fence = Fence::new(epoch, self.me.clone());
                    self.highest_seen = Some(fence.clone());
                    self.state = State::Promoting {
                        fence: fence.clone(),
                        requested_ms: now_ms,
                    };
                    vec![Action::PersistFence(fence)]
                } else {
                    Vec::new()
                }
            }
            State::Promoting {
                fence,
                requested_ms,
            } => {
                // The caller has not confirmed persistence yet; ask again once per heartbeat.
                if now_ms.saturating_sub(*requested_ms) >= self.timing.heartbeat_ms() {
                    let fence = fence.clone();
                    self.state = State::Promoting {
                        fence: fence.clone(),
                        requested_ms: now_ms,
                    };
                    vec![Action::PersistFence(fence)]
                } else {
                    Vec::new()
                }
            }
            State::Primary { fence } => {
                let due = self
                    .last_heartbeat_ms
                    .is_none_or(|t| now_ms.saturating_sub(t) >= self.timing.heartbeat_ms());
                if due {
                    let fence = fence.clone();
                    self.last_heartbeat_ms = Some(now_ms);
                    vec![Action::SendHeartbeat(fence)]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// The caller made `fence` durable. Completes a pending promotion under that fence.
    pub fn fence_persisted(&mut self, fence: &Fence, now_ms: u64) -> Vec<Action> {
        match &self.state {
            State::Promoting { fence: pending, .. } if pending == fence => {
                let fence = fence.clone();
                let silent_since_ms = self.last_heard_ms;
                self.state = State::Primary {
                    fence: fence.clone(),
                };
                self.last_heartbeat_ms = Some(now_ms);
                vec![
                    Action::BecomePrimary {
                        fence: fence.clone(),
                        silent_since_ms,
                        promoted_at_ms: now_ms,
                    },
                    Action::ReconnectDevices {
                        fence: fence.clone(),
                        deadline_ms: now_ms.saturating_add(self.timing.reconnect_ms()),
                    },
                    Action::SendHeartbeat(fence),
                ]
            }
            _ => Vec::new(),
        }
    }

    /// A valid heartbeat (or log entry, or hello) from a primary arrived: proves it is alive.
    pub fn on_heartbeat(&mut self, fence: &Fence, now_ms: u64) -> Result<Vec<Action>, StaleFence> {
        self.observe(fence, now_ms, true)
    }

    /// Learned of a fence from any message (e.g. a peer's refusal naming the current fence).
    /// Never counts as proof that a primary is alive.
    pub fn on_peer_fence(&mut self, fence: &Fence, now_ms: u64) -> Result<Vec<Action>, StaleFence> {
        self.observe(fence, now_ms, false)
    }

    fn observe(
        &mut self,
        fence: &Fence,
        now_ms: u64,
        liveness: bool,
    ) -> Result<Vec<Action>, StaleFence> {
        self.check(fence)?;
        let mut actions = Vec::new();
        if self.highest_seen.as_ref().is_none_or(|h| fence > h) {
            self.highest_seen = Some(fence.clone());
            actions.push(Action::PersistFence(fence.clone()));
        }
        // A fence naming this node that it does not hold is a leftover of an earlier run of this
        // node: record the epoch, but it is nobody's liveness and nobody to follow.
        if fence.node == self.me {
            return Ok(actions);
        }
        match &self.state {
            State::Primary { fence: mine } => {
                if fence > mine {
                    actions.push(Action::Demote {
                        from: mine.clone(),
                        to: fence.clone(),
                    });
                    self.follow(fence, now_ms);
                }
            }
            State::Promoting { fence: mine, .. } => {
                if fence > mine {
                    // Never acted under the pending fence: just follow the newer primary.
                    self.follow(fence, now_ms);
                }
            }
            State::Standby { .. } => {
                self.state = State::Standby {
                    primary: Some(fence.clone()),
                };
                if liveness {
                    self.last_heard_ms = now_ms;
                }
            }
        }
        Ok(actions)
    }

    fn follow(&mut self, fence: &Fence, now_ms: u64) {
        self.state = State::Standby {
            primary: Some(fence.clone()),
        };
        self.last_heard_ms = now_ms;
        self.last_heartbeat_ms = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> NodeId {
        NodeId::new(id).unwrap()
    }

    #[test]
    fn f_sys_04_new_node_waits_a_detection_window_before_promoting() {
        let mut f = Failover::new(node("a"), Timing::default(), None, 0);
        assert!(f.tick(4_999).is_empty());
        assert_eq!(f.role(), InstanceRole::Standby);
        let actions = f.tick(5_000);
        assert_eq!(
            actions,
            vec![Action::PersistFence(Fence::new(1, node("a")))]
        );
        assert_eq!(f.role(), InstanceRole::Promoting);
        assert!(!f.may_act(), "never acts before the fence is persisted");
    }

    #[test]
    fn f_sys_04_promotion_re_requests_persistence_until_confirmed() {
        let mut f = Failover::new(node("a"), Timing::default(), None, 0);
        f.tick(5_000);
        assert!(f.tick(5_500).is_empty());
        assert_eq!(
            f.tick(6_000),
            vec![Action::PersistFence(Fence::new(1, node("a")))]
        );
        assert!(f
            .fence_persisted(&Fence::new(9, node("a")), 6_100)
            .is_empty());
        assert!(!f.may_act());
    }
}
