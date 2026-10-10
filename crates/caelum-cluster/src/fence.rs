//! Node identities and fencing tokens (F-SYS-04, `docs/03_ARCHITECTURE.md` §6).
//!
//! A [`Fence`] is the pair (epoch, node). Every promotion takes a fence with a higher epoch than
//! any the node has seen, and fences are totally ordered (epoch first, then node ID as a tie-break),
//! so two primaries can always tell which one is current. Everything a primary sends carries its
//! fence; a node that has seen a higher fence refuses it, and a primary that sees a higher fence
//! demotes itself.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Longest node ID.
pub const MAX_NODE_ID_LEN: usize = 64;

/// A cluster node (one CAELUM instance), e.g. `laptop-a`.
///
/// ASCII letters, digits, `.`, `_` and `-`, 1 to 64 characters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NodeId(String);

/// A malformed node ID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("node id must be 1 to 64 characters of A-Z, a-z, 0-9, '.', '_' or '-'")]
pub struct InvalidNodeId;

impl NodeId {
    /// Validates and wraps a node ID.
    pub fn new(id: &str) -> Result<Self, InvalidNodeId> {
        let ok = !id.is_empty()
            && id.len() <= MAX_NODE_ID_LEN
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if ok {
            Ok(Self(id.to_owned()))
        } else {
            Err(InvalidNodeId)
        }
    }

    /// The ID text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NodeId {
    type Error = InvalidNodeId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        NodeId::new(&value)
    }
}

impl From<NodeId> for String {
    fn from(value: NodeId) -> Self {
        value.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A fencing token: the epoch a primary acts under, and the node that took it.
///
/// Ordered by epoch, then node ID; the greater fence is current (F-SYS-04).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Fence {
    /// Promotion counter; never reused by the cluster.
    pub epoch: u64,
    /// The node that promoted itself under this epoch.
    pub node: NodeId,
}

impl Fence {
    /// A fence.
    pub fn new(epoch: u64, node: NodeId) -> Self {
        Self { epoch, node }
    }
}

impl fmt::Display for Fence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.epoch, self.node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f_sys_04_node_ids_are_validated() {
        assert!(NodeId::new("laptop-a").is_ok());
        assert!(NodeId::new("").is_err());
        assert!(NodeId::new("has space").is_err());
        assert!(NodeId::new(&"x".repeat(65)).is_err());
        assert!(serde_json::from_str::<NodeId>("\"bad id\"").is_err());
    }

    #[test]
    fn f_sys_04_fences_order_by_epoch_then_node() {
        let a = NodeId::new("a").unwrap();
        let b = NodeId::new("b").unwrap();
        assert!(Fence::new(2, a.clone()) > Fence::new(1, b.clone()));
        assert!(Fence::new(1, b) > Fence::new(1, a));
    }
}
