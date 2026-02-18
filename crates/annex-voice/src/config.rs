use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct LiveKitConfig {
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
}

impl fmt::Debug for LiveKitConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveKitConfig")
            .field("url", &self.url)
            .field("api_key", &self.api_key)
            .field("api_secret", &"[REDACTED]")
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
        }
    }
}
