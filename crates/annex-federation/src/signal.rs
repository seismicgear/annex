use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

const DEFAULT_SIGNAL_BASE_URL: &str = "https://router.monolithannex.com";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalingPayload {
    pub from_server_slug: String,
    pub to_server_slug: String,
    /// Correlation id for one offer/answer exchange.
    pub session_id: String,
    /// SDP type: "offer" | "answer"
    pub sdp_type: String,
    pub sdp: String,
    /// Unix timestamp in milliseconds when this signal was created.
    pub sent_at_ms: i64,
    /// Ed25519 signature over the canonical signaling payload.
    pub vrp_signature: String,
}

#[derive(Debug, Error)]
pub enum SignalError {
    #[error("signal network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("signal endpoint returned {status}: {body}")]
    Http { status: StatusCode, body: String },
}

#[derive(Clone)]
pub struct SignalClient {
    client: reqwest::Client,
    base_url: String,
}

impl SignalClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        Self::with_base_url(DEFAULT_SIGNAL_BASE_URL)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(70))
            .build()?;

        Ok(Self {
            client,
            base_url: base_url.into(),
        })
    }

    pub async fn post_signal(&self, payload: &SignalingPayload) -> Result<(), SignalError> {
        let url = format!("{}/api/signal", self.base_url.trim_end_matches('/'));
        let resp = self.client.post(url).json(payload).send().await?;
        if resp.status().is_success() {
            return Ok(());
        }

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(SignalError::Http { status, body })
    }

    /// Long-polls for the next signal addressed to `local_server_slug`.
    pub async fn poll_signal(
        &self,
        local_server_slug: &str,
        wait_seconds: u64,
    ) -> Result<Option<SignalingPayload>, SignalError> {
        let url = format!("{}/api/signal", self.base_url.trim_end_matches('/'));
        let resp = self
            .client
            .get(url)
            .query(&[
                ("slug", local_server_slug),
                ("wait", &wait_seconds.to_string()),
            ])
            .send()
            .await?;

        if resp.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SignalError::Http { status, body });
        }

        Ok(Some(resp.json::<SignalingPayload>().await?))
    }
}
