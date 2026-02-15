//! Server policy configuration.

use serde::{Deserialize, Serialize};

/// Defines the operational policy of an Annex server.
///
/// This struct is serialized to JSON and stored in the `servers` and `server_policy_versions` tables.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerPolicy {
    /// Minimum VRP alignment score required for agents to join.
    pub agent_min_alignment_score: f32,
    /// Capabilities required for agents to join.
    pub agent_required_capabilities: Vec<String>,
    /// Whether federation with other servers is enabled.
    pub federation_enabled: bool,
    /// Default message retention period in days.
    pub default_retention_days: u32,
    /// Whether voice channels are enabled.
    pub voice_enabled: bool,
    /// Maximum number of members allowed on the server.
    pub max_members: u32,
    /// Rate limiting configuration.
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
}

/// Configuration for API rate limiting.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RateLimitConfig {
    /// Max requests per minute for registration endpoint.
    pub registration_limit: u32,
    /// Max requests per minute for verification endpoint.
    pub verification_limit: u32,
    /// Max requests per minute for other endpoints.
    pub default_limit: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            registration_limit: 5,
            verification_limit: 5,
            default_limit: 60,
        }
    }
}

impl Default for ServerPolicy {
    fn default() -> Self {
        Self {
            agent_min_alignment_score: 0.8,
            agent_required_capabilities: Vec::new(),
            federation_enabled: true,
            default_retention_days: 30,
            voice_enabled: true,
            max_members: 1000,
            rate_limit: RateLimitConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_values() {
        let policy = ServerPolicy::default();
        assert_eq!(policy.agent_min_alignment_score, 0.8);
        assert!(policy.agent_required_capabilities.is_empty());
        assert!(policy.federation_enabled);
        assert_eq!(policy.default_retention_days, 30);
        assert!(policy.voice_enabled);
        assert_eq!(policy.max_members, 1000);
        assert_eq!(policy.rate_limit.registration_limit, 5);
        assert_eq!(policy.rate_limit.verification_limit, 5);
        assert_eq!(policy.rate_limit.default_limit, 60);
    }

    #[test]
    fn serialization_round_trip() {
        let policy = ServerPolicy::default();
        let json = serde_json::to_string(&policy).expect("should serialize");
        let decoded: ServerPolicy = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(policy, decoded);
    }
}
