//! ## ⚠️ Experimental — not wired into the production server
//!
//! `FederationTransport` defines the WebRTC peer-to-peer transport for
//! federation traffic, gated by an Ed25519-signed signaling envelope
//! through `api/signal.js`. The struct is fully typed and the
//! cryptographic verifier callback is part of the constructor, BUT
//! **no caller in the workspace currently instantiates it**.
//!
//! Until a caller is wired in, the production server does NOT use this
//! transport — federation traffic continues to flow over the existing
//! HTTP federation routes (`/api/federation/*`), which have their own
//! authentication path. Treat this module as the staging ground for the
//! future relay-based transport.
//!
//! Anyone adding the first caller MUST also:
//!
//! 1. Configure `ANNEX_SIGNAL_TRUSTED_PEERS` on the relay
//!    (`api/signal.js`) so the slug↔pubkey binding is enforced before
//!    a signed envelope reaches us.
//! 2. Provide a `signal_verifier` closure that consults the
//!    `instances` table (slug → public_key) and the active
//!    `federation_agreements` to authorise the sender — not just check
//!    that the signature matches some pubkey.
//! 3. Add an integration test exercising send/receive end-to-end with
//!    the real relay surface.
//!
//! Until those land, do not flip a feature flag to "enable the
//! transport" in production. The relay accepts signed envelopes
//! addressed at trusted slugs but the receiving server has no
//! verifier wired up, so an authorised peer could still inject SDP
//! into a session this server never initiated. The defence-in-depth
//! check at the receiver is the missing piece.
//!
//! See `crates/annex-federation/src/signal.rs::SignalingPayload` for
//! the wire format and `api/signal.js` for the relay-side gates.

use crate::seal::{open as seal_open, seal as seal_seal, SealError};
use crate::signal::{SignalClient, SignalError, SignalingPayload};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{SigningKey, VerifyingKey};
use futures_util::Future;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{timeout, Duration};
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

pub type InboundHandler = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> + Send + Sync + 'static,
>;
pub type SignalSigner = Arc<dyn Fn(&SignalingPayload) -> Option<String> + Send + Sync + 'static>;
pub type SignalVerifier =
    Arc<dyn Fn(&SignalingPayload) -> Result<(), String> + Send + Sync + 'static>;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("signal error: {0}")]
    Signal(#[from] SignalError),
    #[error("webrtc error: {0}")]
    WebRtc(String),
    #[error("transport not connected for remote slug: {0}")]
    NotConnected(String),
    #[error("peer did not answer in time; entering wait-for-peer state for: {0}")]
    WaitForPeer(String),
    #[error("signaling payload rejected: {0}")]
    SignalingRejected(String),
    #[error("seal error: {0}")]
    Seal(String),
}

impl From<SealError> for TransportError {
    fn from(e: SealError) -> Self {
        TransportError::Seal(e.to_string())
    }
}

/// Parse a 32-byte Ed25519 public key from a 64-char hex string.
fn verifying_key_from_hex(hex: &str) -> Result<VerifyingKey, TransportError> {
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2).unwrap_or(""), 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| TransportError::Seal("invalid peer pubkey hex".to_string()))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| TransportError::Seal("peer pubkey must be 32 bytes".to_string()))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| TransportError::Seal(e.to_string()))
}

/// Seal an SDP for `recipient` and base64-encode it for the `sdp` wire field.
/// The relay only ever sees this opaque blob — never the ICE candidates / IPs.
fn seal_sdp(sdp: &str, recipient: &VerifyingKey) -> Result<String, TransportError> {
    Ok(BASE64.encode(seal_seal(sdp.as_bytes(), recipient)?))
}

/// Reverse of [`seal_sdp`]: base64-decode then open with our own key.
fn open_sdp(sealed_b64: &str, me: &SigningKey) -> Result<String, TransportError> {
    let blob = BASE64
        .decode(sealed_b64.as_bytes())
        .map_err(|_| TransportError::Seal("sealed sdp is not valid base64".to_string()))?;
    let plain = seal_open(&blob, me)?;
    String::from_utf8(plain)
        .map_err(|_| TransportError::Seal("sealed sdp was not utf-8".to_string()))
}

#[derive(Clone)]
pub struct FederationTransport {
    local_server_slug: String,
    /// Local Ed25519 public key, hex-encoded (64 chars). Stamped onto
    /// every outbound `SignalingPayload` so the relay can verify the
    /// `vrp_signature` against this key under a production profile
    /// (see `api/signal.js`). The receiving server independently checks
    /// that the slug↔pubkey binding matches what it knows about the
    /// sender via the `signal_verifier` callback.
    local_public_key_hex: String,
    signal: SignalClient,
    peers: Arc<RwLock<HashMap<String, Arc<RTCPeerConnection>>>>,
    channels: Arc<RwLock<HashMap<String, Arc<RTCDataChannel>>>>,
    pending_answers: Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>>,
    inbound_handler: InboundHandler,
    signal_signer: SignalSigner,
    signal_verifier: SignalVerifier,
    /// Our Ed25519 signing key, used to OPEN SDPs sealed to us (the recipient
    /// X25519 key is derived from it). The plaintext SDP — and the ICE
    /// candidates / IP addresses inside — never leaves this process unsealed.
    local_signing_key: Arc<SigningKey>,
}

impl FederationTransport {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        local_server_slug: impl Into<String>,
        local_public_key_hex: impl Into<String>,
        local_signing_key: Arc<SigningKey>,
        signal: SignalClient,
        inbound_handler: InboundHandler,
        signal_signer: SignalSigner,
        signal_verifier: SignalVerifier,
    ) -> Self {
        Self {
            local_server_slug: local_server_slug.into(),
            local_public_key_hex: local_public_key_hex.into(),
            signal,
            peers: Arc::new(RwLock::new(HashMap::new())),
            channels: Arc::new(RwLock::new(HashMap::new())),
            pending_answers: Arc::new(Mutex::new(HashMap::new())),
            inbound_handler,
            signal_signer,
            signal_verifier,
            local_signing_key,
        }
    }

    pub fn spawn_signal_listener(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                match self.signal.poll_signal(&self.local_server_slug, 55).await {
                    Ok(Some(payload)) => {
                        if let Err(err) = self.handle_signal_payload(payload).await {
                            tracing::warn!(error = %err, "failed to process inbound federation signal");
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        tracing::warn!(error = %err, "signal long-poll failed");
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
            }
        });
    }

    pub async fn establish_peer(
        &self,
        remote_server_slug: &str,
        remote_pubkey_hex: &str,
    ) -> Result<(), TransportError> {
        let recipient = verifying_key_from_hex(remote_pubkey_hex)?;
        let pc = self
            .build_peer_connection(remote_server_slug.to_string())
            .await?;

        let dc = pc
            .create_data_channel(
                "annex-federation",
                Some(RTCDataChannelInit {
                    ordered: Some(true),
                    ..Default::default()
                }),
            )
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        self.bind_data_channel(remote_server_slug.to_string(), dc.clone())
            .await;

        let offer = pc
            .create_offer(None)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        pc.set_local_description(offer.clone())
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        let session_id = uuid::Uuid::new_v4().to_string();
        self.pending_answers
            .lock()
            .await
            .insert(session_id.clone(), tx);

        let mut offer_payload = SignalingPayload {
            from_server_slug: self.local_server_slug.clone(),
            to_server_slug: remote_server_slug.to_string(),
            session_id: session_id.clone(),
            sdp_type: "offer".to_string(),
            // Sealed to the recipient: the relay forwards opaque ciphertext.
            sdp: seal_sdp(&offer.sdp, &recipient)?,
            sent_at_ms: chrono::Utc::now().timestamp_millis(),
            from_pubkey_hex: self.local_public_key_hex.clone(),
            vrp_signature: String::new(),
        };
        offer_payload.vrp_signature =
            self.signal_signer.as_ref()(&offer_payload).ok_or_else(|| {
                TransportError::SignalingRejected("missing signaling signature".to_string())
            })?;
        self.signal.post_signal(&offer_payload).await?;

        let answer_sdp = match timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => {
                self.pending_answers.lock().await.remove(&session_id);
                return Err(TransportError::WebRtc("answer channel closed".to_string()));
            }
            Err(_) => {
                self.pending_answers.lock().await.remove(&session_id);
                return Err(TransportError::WaitForPeer(remote_server_slug.to_string()));
            }
        };
        let answer = RTCSessionDescription::answer(answer_sdp)
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        pc.set_remote_description(answer)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        self.peers
            .write()
            .await
            .insert(remote_server_slug.to_string(), pc);

        Ok(())
    }

    pub async fn relay_message(
        &self,
        remote_server_slug: &str,
        envelope_json: &str,
    ) -> Result<(), TransportError> {
        let channels = self.channels.read().await;
        let Some(dc) = channels.get(remote_server_slug) else {
            return Err(TransportError::NotConnected(remote_server_slug.to_string()));
        };

        dc.send_text(envelope_json)
            .await
            .map(|_| ())
            .map_err(|e| TransportError::WebRtc(e.to_string()))
    }

    async fn handle_signal_payload(&self, payload: SignalingPayload) -> Result<(), TransportError> {
        if payload.to_server_slug != self.local_server_slug {
            return Err(TransportError::WebRtc(
                "signaling payload addressed to a different server".to_string(),
            ));
        }
        let age_ms = chrono::Utc::now().timestamp_millis() - payload.sent_at_ms;
        if age_ms.abs() > 60_000 {
            return Err(TransportError::SignalingRejected(
                "expired or clock-skewed signaling payload".to_string(),
            ));
        }
        self.signal_verifier.as_ref()(&payload).map_err(TransportError::SignalingRejected)?;
        match payload.sdp_type.as_str() {
            "offer" => self.handle_offer(payload).await,
            "answer" => {
                let answer_sdp = open_sdp(&payload.sdp, &self.local_signing_key)?;
                if let Some(tx) = self
                    .pending_answers
                    .lock()
                    .await
                    .remove(&payload.session_id)
                {
                    let _ = tx.send(answer_sdp);
                } else {
                    tracing::debug!(
                        from = %payload.from_server_slug,
                        session_id = %payload.session_id,
                        "dropping unmatched federation answer"
                    );
                }
                Ok(())
            }
            other => Err(TransportError::WebRtc(format!(
                "unsupported signaling payload type: {other}"
            ))),
        }
    }

    async fn handle_offer(&self, payload: SignalingPayload) -> Result<(), TransportError> {
        // The offerer's identity key — used to seal our answer back to them.
        let offerer = verifying_key_from_hex(&payload.from_pubkey_hex)?;
        // Open the sealed offer locally; the relay never saw this plaintext.
        let offer_sdp = open_sdp(&payload.sdp, &self.local_signing_key)?;

        let pc = self
            .build_peer_connection(payload.from_server_slug.clone())
            .await?;

        let offer = RTCSessionDescription::offer(offer_sdp)
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        pc.set_remote_description(offer)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        let answer = pc
            .create_answer(None)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        pc.set_local_description(answer.clone())
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;

        let mut answer_payload = SignalingPayload {
            from_server_slug: self.local_server_slug.clone(),
            to_server_slug: payload.from_server_slug.clone(),
            session_id: payload.session_id,
            sdp_type: "answer".to_string(),
            // Sealed to the offerer: the relay forwards opaque ciphertext.
            sdp: seal_sdp(&answer.sdp, &offerer)?,
            sent_at_ms: chrono::Utc::now().timestamp_millis(),
            from_pubkey_hex: self.local_public_key_hex.clone(),
            vrp_signature: String::new(),
        };
        answer_payload.vrp_signature =
            self.signal_signer.as_ref()(&answer_payload).ok_or_else(|| {
                TransportError::SignalingRejected("missing signaling signature".to_string())
            })?;
        self.signal.post_signal(&answer_payload).await?;

        self.peers
            .write()
            .await
            .insert(payload.from_server_slug, pc);
        Ok(())
    }

    async fn build_peer_connection(
        &self,
        remote_slug: String,
    ) -> Result<Arc<RTCPeerConnection>, TransportError> {
        let api = APIBuilder::new().build();
        let pc = Arc::new(
            api.new_peer_connection(RTCConfiguration {
                ice_servers: vec![RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_string()],
                    ..Default::default()
                }],
                ..Default::default()
            })
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?,
        );

        let channels = self.channels.clone();
        let handler = self.inbound_handler.clone();
        let remote_for_dc = remote_slug.clone();
        pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
            let channels = channels.clone();
            let handler = handler.clone();
            let remote_for_dc = remote_for_dc.clone();
            Box::pin(async move {
                channels
                    .write()
                    .await
                    .insert(remote_for_dc.clone(), dc.clone());

                dc.on_message(Box::new(move |msg| {
                    let handler = handler.clone();
                    Box::pin(async move {
                        if let Ok(payload) = String::from_utf8(msg.data.to_vec()) {
                            (handler)(payload).await;
                        }
                    })
                }));
            })
        }));

        Ok(pc)
    }

    async fn bind_data_channel(&self, remote_slug: String, dc: Arc<RTCDataChannel>) {
        self.channels.write().await.insert(remote_slug, dc.clone());
        let handler = self.inbound_handler.clone();
        dc.on_message(Box::new(move |msg| {
            let handler = handler.clone();
            Box::pin(async move {
                if let Ok(payload) = String::from_utf8(msg.data.to_vec()) {
                    (handler)(payload).await;
                }
            })
        }));
    }
}
