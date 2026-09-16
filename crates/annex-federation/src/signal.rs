use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

const DEFAULT_SIGNAL_BASE_URL: &str = "https://router.monolithannex.com";

/// Legacy slug addressing. Canonical string:
/// `from|to|session_id|sdp_type|sdp|sent_at_ms|from_pubkey_hex`.
pub const FORMAT_V1: u8 = 1;

/// Rendezvous-tag addressing. Canonical string:
/// `v2|tag|session_id|sdp_type|sdp|sent_at_ms|from_pubkey_hex`.
pub const FORMAT_V2: u8 = 2;

/// Envelope format default for deserialisation: an envelope with no
/// `format_version` is v1, which is what every peer built before rendezvous
/// addressing sends.
fn default_format_version() -> u8 {
    FORMAT_V1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalingPayload {
    /// Which wire contract this envelope follows.
    ///
    /// Carried on the wire AND inside the signed canonical string. Both
    /// halves are load-bearing. On the wire it lets a relay and a peer that
    /// each understand both versions interoperate without a flag day —
    /// `api/signal.js` dispatches on it. Inside the signature it stops a
    /// downgrade: without it, an attacker could strip `rendezvous_tag` from a
    /// v2 envelope, present it as v1, and the signature would still verify
    /// over a string that no longer says where the envelope was addressed.
    #[serde(default = "default_format_version")]
    pub format_version: u8,
    pub from_server_slug: String,
    pub to_server_slug: String,
    /// Rotating, metadata-hardened rendezvous address (see
    /// [`crate::metadata::rendezvous_tag`]). Under [`FORMAT_V2`] it is the
    /// relay's queue key and the slugs are empty, so the relay never observes
    /// the stable slug graph and cannot link a recipient across time buckets.
    /// Empty under [`FORMAT_V1`].
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
    /// Base64 Ed25519 signature over [`SignalingPayload::canonical_signing_input`].
    pub vrp_signature: String,
}

impl SignalingPayload {
    /// The exact byte string that is signed and verified.
    ///
    /// This function and `canonicalEnvelope` in `api/signal.js` are one
    /// contract with two implementations, and they disagreed for the whole
    /// life of the relay transport: Rust interpolated `rendezvous_tag` as a
    /// fourth field, JavaScript did not, so no signature a Rust peer produced
    /// could ever verify at the relay. Nothing caught it because nothing
    /// instantiated the transport. `api/canonical-vectors.json` pins both
    /// sides now — `signal.rs`'s own tests read it, and so does
    /// `api/signal.test.mjs`.
    ///
    /// The `v2` literal leads the v2 string rather than a bare version number
    /// so the two shapes cannot collide: without it, a v1 envelope whose
    /// `from_server_slug` happened to be the string `v2` would canonicalise
    /// identically to a v2 envelope, and a signature over one would verify
    /// as the other.
    pub fn canonical_signing_input(&self) -> String {
        if self.format_version == FORMAT_V2 {
            format!(
                "v2|{}|{}|{}|{}|{}|{}",
                self.rendezvous_tag,
                self.session_id,
                self.sdp_type,
                self.sdp,
                self.sent_at_ms,
                self.from_pubkey_hex
            )
        } else {
            format!(
                "{}|{}|{}|{}|{}|{}|{}",
                self.from_server_slug,
                self.to_server_slug,
                self.session_id,
                self.sdp_type,
                self.sdp,
                self.sent_at_ms,
                self.from_pubkey_hex
            )
        }
    }

    /// The relay queue this envelope is addressed to.
    pub fn queue_key(&self) -> &str {
        if self.format_version == FORMAT_V2 {
            &self.rendezvous_tag
        } else {
            &self.to_server_slug
        }
    }
}

/// Headers proving the caller owns the queue it is draining.
///
/// Knowing a tag is not authorisation to drain it: a tag travels in every
/// envelope addressed to its owner, so any relay operator or on-path observer
/// learns it. `api/signal.js` recomputes the tag forwards from
/// `drain_pubkey_hex` and refuses a drain whose tag is not that key's own
/// address.
#[derive(Debug, Clone)]
pub struct DrainAuth {
    pub drain_pubkey_hex: String,
    pub timestamp_ms: i64,
    pub signature_b64: String,
}

impl DrainAuth {
    /// The string a drainer signs: `drain|v2|<tag>|<timestamp_ms>`.
    pub fn canonical_signing_input(tag: &str, timestamp_ms: i64) -> String {
        format!("drain|v2|{tag}|{timestamp_ms}")
    }
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
        self.poll(&[("slug", local_server_slug)], wait_seconds, None)
            .await
    }

    /// Long-polls the rendezvous queue `tag`, presenting `auth` so a
    /// production relay can confirm we own it.
    ///
    /// `auth` is `Option` only because a dev relay does not check it; a
    /// production relay answers 401 without it, which is the correct and
    /// visible failure rather than a silent empty queue.
    pub async fn poll_tag(
        &self,
        tag: &str,
        wait_seconds: u64,
        auth: Option<&DrainAuth>,
    ) -> Result<Option<SignalingPayload>, SignalError> {
        self.poll(&[("tag", tag)], wait_seconds, auth.map(|a| (tag, a)))
            .await
    }

    async fn poll(
        &self,
        address: &[(&str, &str)],
        wait_seconds: u64,
        auth: Option<(&str, &DrainAuth)>,
    ) -> Result<Option<SignalingPayload>, SignalError> {
        let url = format!("{}/api/signal", self.base_url.trim_end_matches('/'));
        let wait = wait_seconds.to_string();
        let mut req = self.client.get(url);
        for (k, v) in address {
            req = req.query(&[(*k, *v)]);
        }
        req = req.query(&[("wait", wait.as_str())]);
        if let Some((tag, auth)) = auth {
            req = req
                .header("x-annex-drain-tag", tag)
                .header("x-annex-drain-pubkey", &auth.drain_pubkey_hex)
                .header("x-annex-drain-timestamp", auth.timestamp_ms.to_string())
                .header("x-annex-drain-signature", &auth.signature_b64);
        }
        let resp = req.send().await?;

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
