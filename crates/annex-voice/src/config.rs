use serde::{Deserialize, Serialize};
use std::fmt;

fn default_token_ttl_seconds() -> u64 {
    3600
}

/// Default URL used when no LiveKit config is provided. Matches the
/// `docker-compose.yml` and `--dev` mode defaults.
pub const DEV_LIVEKIT_URL: &str = "ws://localhost:7880";
/// Default API key used for LiveKit `--dev` mode.
pub const DEV_LIVEKIT_API_KEY: &str = "devkey";
/// Default API secret used for LiveKit `--dev` mode.
pub const DEV_LIVEKIT_API_SECRET: &str = "secret";

#[derive(Clone, Serialize, Deserialize)]
pub struct LiveKitConfig {
    pub url: String,
    /// Browser-facing LiveKit URL. Falls back to `url` when empty.
    /// In Docker, `url` points to the internal hostname (e.g. `ws://livekit:7880`)
    /// while `public_url` should point to the host-reachable address
    /// (e.g. `ws://localhost:7880`).
    #[serde(default)]
    pub public_url: String,
    pub api_key: String,
    #[serde(skip_serializing)]
    pub api_secret: String,
    /// JWT token TTL in seconds for LiveKit join tokens. Default: 3600 (1 hour).
    #[serde(default = "default_token_ttl_seconds")]
    pub token_ttl_seconds: u64,
}

impl Default for LiveKitConfig {
    fn default() -> Self {
        Self {
            url: DEV_LIVEKIT_URL.to_string(),
            public_url: String::new(),
            api_key: DEV_LIVEKIT_API_KEY.to_string(),
            api_secret: DEV_LIVEKIT_API_SECRET.to_string(),
            token_ttl_seconds: default_token_ttl_seconds(),
        }
    }
}

impl fmt::Debug for LiveKitConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveKitConfig")
            .field("url", &self.url)
            .field("public_url", &self.public_url)
            .field("api_key", &self.api_key)
            .field("api_secret", &"[REDACTED]")
            .field("token_ttl_seconds", &self.token_ttl_seconds)
            .finish()
    }
}

impl LiveKitConfig {
    pub fn new(
        url: impl Into<String>,
        api_key: impl Into<String>,
        api_secret: impl Into<String>,
    ) -> Self {
        Self {
            url: url.into(),
            public_url: String::new(),
            api_key: api_key.into(),
            api_secret: api_secret.into(),
            token_ttl_seconds: default_token_ttl_seconds(),
        }
    }
}
