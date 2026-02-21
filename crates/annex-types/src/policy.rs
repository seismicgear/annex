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
    /// The server's core operating principles (for VRP alignment).
    #[serde(default)]
    pub principles: Vec<String>,
    /// Actions prohibited by the server (for VRP alignment).
    #[serde(default)]
    pub prohibited_actions: Vec<String>,
    /// Server access mode: "public", "invite_only", or "password".
    #[serde(default = "default_access_mode")]
    pub access_mode: String,
    /// Password required to join when access_mode is "password".
    #[serde(default)]
    pub access_password: String,
    /// Whether image uploads are enabled.
    #[serde(default = "default_true")]
    pub images_enabled: bool,
    /// Whether video uploads are enabled.
    #[serde(default = "default_true")]
    pub videos_enabled: bool,
    /// Whether generic file uploads are enabled.
    #[serde(default = "default_true")]
    pub files_enabled: bool,
    /// Maximum image upload size in megabytes.
    #[serde(default = "default_max_upload_mb")]
    pub max_image_size_mb: u32,
    /// Maximum video upload size in megabytes.
    #[serde(default = "default_max_upload_mb")]
    pub max_video_size_mb: u32,
    /// Maximum file upload size in megabytes.
    #[serde(default = "default_max_upload_mb")]
    pub max_file_size_mb: u32,
    /// Whether server-scoped usernames are enabled.
    #[serde(default)]
    pub usernames_enabled: bool,
}

fn default_access_mode() -> String {
    "public".to_string()
}

fn default_true() -> bool {
    true
}

fn default_max_upload_mb() -> u32 {
    5
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
            registration_limit: 10,
            verification_limit: 10,
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
            principles: Vec::new(),
            prohibited_actions: Vec::new(),
            access_mode: "public".to_string(),
            access_password: String::new(),
            images_enabled: true,
            videos_enabled: true,
            files_enabled: true,
            max_image_size_mb: 5,
            max_video_size_mb: 5,
            max_file_size_mb: 5,
            usernames_enabled: false,
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
        assert_eq!(policy.rate_limit.registration_limit, 10);
        assert_eq!(policy.rate_limit.verification_limit, 10);
        assert_eq!(policy.rate_limit.default_limit, 60);
        assert!(policy.principles.is_empty());
        assert!(policy.prohibited_actions.is_empty());
        assert_eq!(policy.access_mode, "public");
        assert!(policy.access_password.is_empty());
        assert!(policy.images_enabled);
        assert!(policy.videos_enabled);
        assert!(policy.files_enabled);
        assert_eq!(policy.max_image_size_mb, 5);
        assert_eq!(policy.max_video_size_mb, 5);
        assert_eq!(policy.max_file_size_mb, 5);
        assert!(!policy.usernames_enabled);
    }

    #[test]
    fn serialization_round_trip() {
        let policy = ServerPolicy::default();
        let json = serde_json::to_string(&policy).expect("should serialize");
        let decoded: ServerPolicy = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(policy, decoded);
    }
}
