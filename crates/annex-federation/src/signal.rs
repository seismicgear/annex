use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

const DEFAULT_SIGNAL_BASE_URL: &str = "https://router.monolithannex.com";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalingPayload {
    pub from_server_slug: String,
    pub to_server_slug: String,
    /// Rotating, metadata-hardened rendezvous address (see
    /// [`crate::metadata::rendezvous_tag`]). When non-empty it is the relay's
    /// queue key instead of `to_server_slug`, so the relay never observes the
    /// stable slug graph and cannot link a recipient across time buckets. It is
    /// part of the signed canonical string, so the relay cannot re-address a
    /// signed envelope. Empty string = legacy slug addressing.
    #[serde(default)]
    pub rendezvous_tag: String,
    /// Correlation id for one offer/answer exchange.
    pub session_id: String,
    /// SDP type: "offer" | "answer"
    pub sdp_type: String,
    pub sdp: String,
    /// Unix timestamp in milliseconds when this signal was created.
    pub sent_at_ms: i64,
    /// Sender's Ed25519 public key, 32 raw bytes hex-encoded (64 hex chars).
    ///
    /// Production deployments of `api/signal.js` REJECT payloads whose
    /// `vrp_signature` does not verify against this key. The receiving
    /// server still owns the slug→pubkey binding check (via its
    /// `SignalVerifier` callback) — the relay's job is only to refuse
    /// unsigned traffic. `#[serde(default)]` keeps the field optional
    /// on the wire so older clients can be parsed; the relay-level
    /// production gate is what enforces presence.
    #[serde(default)]
    pub from_pubkey_hex: String,
    /// Ed25519 signature over the canonical signaling payload
    ///   `from_server_slug|to_server_slug|rendezvous_tag|session_id|sdp_type|sdp|sent_at_ms|from_pubkey_hex`
    /// base64-encoded (`rendezvous_tag` is the empty string when absent). This
    /// MUST match `api/signal.js` in the monolith-annex repo.
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
