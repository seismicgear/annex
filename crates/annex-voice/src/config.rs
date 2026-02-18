use serde::{Deserialize, Serialize};
use std::fmt;

fn default_token_ttl_seconds() -> u64 {
    3600
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LiveKitConfig {
    pub url: String,
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
            url: String::new(),
            api_key: String::new(),
            api_secret: String::new(),
            token_ttl_seconds: default_token_ttl_seconds(),
        }
    }
}

impl fmt::Debug for LiveKitConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveKitConfig")
            .field("url", &self.url)
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
            api_key: api_key.into(),
            api_secret: api_secret.into(),
            token_ttl_seconds: default_token_ttl_seconds(),
        }
    }
}
