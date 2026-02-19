use crate::{VrpAnchorSnapshot, VrpError};
use annex_types::ServerPolicy;
use serde::{Deserialize, Serialize};

/// A server's policy root, used for VRP alignment.
///
/// This structure represents the raw ethical/policy stance of a server
/// before it is hashed into a `VrpAnchorSnapshot`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerPolicyRoot {
    /// The server's core operating principles.
    pub principles: Vec<String>,
    /// Actions prohibited by the server.
    pub prohibited_actions: Vec<String>,
}

impl ServerPolicyRoot {
    /// Creates a new policy root.
    pub fn new(principles: Vec<String>, prohibited_actions: Vec<String>) -> Self {
        Self {
            principles,
            prohibited_actions,
        }
    }

    /// Derives a policy root from a server policy configuration.
    pub fn from_policy(policy: &ServerPolicy) -> Self {
        Self {
            principles: policy.principles.clone(),
            prohibited_actions: policy.prohibited_actions.clone(),
        }
    }

    /// Converts the policy root into an anchor snapshot for VRP comparison.
    ///
    /// Returns `VrpError::SystemClockInvalid` if the system clock is misconfigured.
    pub fn to_anchor_snapshot(&self) -> Result<VrpAnchorSnapshot, VrpError> {
        VrpAnchorSnapshot::new(&self.principles, &self.prohibited_actions)
    }
}

impl From<&ServerPolicy> for ServerPolicyRoot {
    fn from(policy: &ServerPolicy) -> Self {
        Self::from_policy(policy)
    }
}
