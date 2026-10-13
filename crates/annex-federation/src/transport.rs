//! WebRTC peer-to-peer transport for federation traffic.
//!
//! Two servers bootstrap a data channel by exchanging sealed SDP through
//! `api/signal.js`, a relay that is trusted to deliver bytes and trusted with
//! nothing else. `annex-server`'s `startup.rs` builds one of these when
//! `ANNEX_FEDERATION_RELAY_TRANSPORT_ENABLED` is set; the HTTP federation routes
//! (`/api/federation/*`) remain the path when it is not.
//!
//! ### What the relay is not allowed to learn
//!
//! * **Content.** SDPs are sealed to the recipient's identity key and
//!   length-padded ([`crate::metadata::seal_padded_to`]), so the relay sees a
//!   constant-size opaque blob and never an ICE candidate or an IP.
//! * **The graph.** Envelopes are addressed to a rotating rendezvous tag
//!   ([`crate::metadata::rendezvous_tag_for`]) — a one-way function of the
//!   recipient's key and the hour — and carry no server slugs at all. Tags for
//!   the same peer in different hours are unlinkable.
//! * **Who may read a queue.** A tag rides in every envelope addressed to its
//!   owner, so knowing one is not authorisation. A drain presents the
//!   drainer's public key and a signature, and the relay recomputes the tag
//!   forwards from that key: only the addressee can drain.
//!
//! ### What the relay IS relied on for, and what backs it up
//!
//! The relay authenticates senders (Ed25519 over the canonical envelope) and
//! authorises them against `ANNEX_SIGNAL_TRUSTED_PEERS`. Because v2 envelopes
//! carry no slug, that map degenerates to an allowlist of authorised keys —
//! the relay cannot bind a key to a server it is not allowed to know about.
//! The peer↔key binding lives here instead, in the `PeerAuthorizer` callback,
//! which consults `instances` and the active `federation_agreements`. Both
//! checks are required: the relay's stops unauthorised traffic reaching us at
//! all, and ours stops an authorised peer injecting SDP into a session this
//! server never initiated.
//!
//! [`FederationTransport::handle_signal_payload`] also re-derives our own tag
//! and refuses an envelope addressed elsewhere. "It came out of our queue, so
//! it is for us" is an argument about the relay's behaviour, and the relay is
//! the untrusted party.
//!
//! ### The wire format is a two-implementation contract
//!
//! [`crate::signal::SignalingPayload::canonical_signing_input`] and
//! `canonicalEnvelope` in `api/signal.js` must agree byte for byte. They did
//! not, for the whole time this module had no caller: Rust interpolated
//! `rendezvous_tag` as a fourth field and JavaScript did not, so every
//! signature this transport produced would have been rejected with a 401 and
//! the only symptom would have been federation never connecting.
//! `api/canonical-vectors.json` and `api/rendezvous-vectors.json` are read by
//! `crates/annex-federation/tests/canonical_vectors.rs` and by
//! `api/signal.test.mjs`, so the two sides now fail together or not at all.
//!
//! The envelope carries an explicit `format_version`, inside the signature as
//! well as on the wire, so a relay and a peer can be upgraded independently
//! and neither can be talked into verifying a v2 envelope under v1 rules.

use crate::metadata::{
    current_bucket, open_padded_from, rendezvous_tag_for, rendezvous_tag_for_now, seal_padded_to,
};
use crate::seal::SealError;
use crate::signal::{DrainAuth, SignalClient, SignalError, SignalingPayload, FORMAT_V2};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use futures_util::Future;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use thiserror::Error;
use tokio::sync::{oneshot, Mutex, Notify, RwLock, Semaphore};
use tokio::time::{timeout, Duration};
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::data_channel_state::RTCDataChannelState;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

mod rpc;
use rpc::{Chunk, Packet, CHUNK_BYTES, MAX_BODY, MAX_PACKET, MAX_REQUEST};

// The ordered authority protocol admits a 16 MiB event or intent. Leave room
// for its signed enclosing request/receipt and use the same limits on HTTP and
// DataChannel paths, so acceptance cannot exceed durable delivery capacity.
pub const MAX_APPLICATION_REQUEST_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_APPLICATION_RESPONSE_BYTES: usize = 20 * 1024 * 1024;

/// A receiver's application refusal, carried inside the signed, correlated
/// negative receipt. Transport failures never manufacture a semantic status.
#[derive(Clone, Debug, Error, serde::Serialize, serde::Deserialize)]
#[error("HTTP {status}: {message}")]
#[serde(deny_unknown_fields)]
pub struct ApplicationError {
    pub status: u16,
    pub message: String,
}
impl ApplicationError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status: if (400..=599).contains(&status) {
                status
            } else {
                500
            },
            message: message.into().chars().take(4096).collect(),
        }
    }
    fn wire(&self) -> String {
        serde_json::to_string(&("annex-federation-refusal-v1", self))
            .expect("fixed refusal is serializable")
    }
    fn from_wire(body: String) -> Self {
        match serde_json::from_str::<(String, Self)>(&body) {
            Ok((domain, error))
                if domain == "annex-federation-refusal-v1"
                    && (400..=599).contains(&error.status)
                    && error.message.len() <= 16384 =>
            {
                error
            }
            // Older peers returned untyped text. It cannot establish a
            // permanent refusal, regardless of text resembling an HTTP code.
            _ => Self::new(502, body),
        }
    }
}
impl From<String> for ApplicationError {
    fn from(message: String) -> Self {
        Self::new(500, message)
    }
}
impl From<&str> for ApplicationError {
    fn from(message: &str) -> Self {
        Self::new(500, message)
    }
}

pub type InboundHandler = Arc<
    dyn Fn(
            String,
            String,
            String,
        )
            -> Pin<Box<dyn Future<Output = Result<String, ApplicationError>> + Send + 'static>>
        + Send
        + Sync
        + 'static,
>;
pub type PeerAuthorizer = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync + 'static>;

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
    #[error("federation RPC protocol error: {0}")]
    Protocol(String),
    #[error("peer rejected federation operation: {0}")]
    RemoteRejected(ApplicationError),
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

/// Seal an SDP for `recipient` (length-padded) and base64-encode it for the
/// `sdp` wire field. The relay only ever sees this opaque, constant-size blob —
/// never the ICE candidates / IPs, and never the payload's true length.
fn seal_sdp(sdp: &str, recipient: &VerifyingKey) -> Result<String, TransportError> {
    Ok(BASE64.encode(seal_padded_to(sdp.as_bytes(), recipient)?))
}

/// Reverse of [`seal_sdp`]: base64-decode, open with our own key, strip padding.
fn open_sdp(sealed_b64: &str, me: &SigningKey) -> Result<String, TransportError> {
    let blob = BASE64
        .decode(sealed_b64.as_bytes())
        .map_err(|_| TransportError::Seal("sealed sdp is not valid base64".to_string()))?;
    let plain = open_padded_from(&blob, me)?;
    String::from_utf8(plain)
        .map_err(|_| TransportError::Seal("sealed sdp was not utf-8".to_string()))
}

struct Peer {
    generation: uuid::Uuid,
    pc: Arc<RTCPeerConnection>,
    dc: Option<Arc<RTCDataChannel>>,
    outbound: bool,
    writes: Arc<Mutex<()>>,
    created: tokio::time::Instant,
}
struct PendingAnswer {
    peer: String,
    generation: uuid::Uuid,
}
struct PendingRequest {
    peer: String,
    generation: uuid::Uuid,
    digest: String,
    endpoint: String,
    reply: oneshot::Sender<Result<String, ApplicationError>>,
    created: tokio::time::Instant,
}
struct Assembly {
    created: tokio::time::Instant,
    count: usize,
    next: usize,
    data: Vec<u8>,
}

/// Counters expose actual data-channel traffic and application receipts.
#[derive(Default)]
pub struct TransportCounters {
    pub bytes_sent: AtomicU64,
    pub bytes_received: AtomicU64,
    pub requests_committed: AtomicU64,
    pub receipts_verified: AtomicU64,
}

#[derive(Clone)]
pub struct FederationTransport {
    local_public_key_hex: String,
    signal: SignalClient,
    peers: Arc<RwLock<HashMap<String, Peer>>>,
    connect_locks: Arc<Mutex<HashMap<String, std::sync::Weak<Mutex<()>>>>>,
    pending_answers: Arc<Mutex<HashMap<String, PendingAnswer>>>,
    pending_requests: Arc<Mutex<HashMap<String, PendingRequest>>>,
    assemblies: Arc<Mutex<HashMap<(String, String), Assembly>>>,
    seen_signals: Arc<Mutex<HashMap<String, tokio::time::Instant>>>,
    changed: Arc<Notify>,
    establishment_slots: Arc<Semaphore>,
    incoming_establishment_slots: Arc<Semaphore>,
    signal_slots: Arc<Semaphore>,
    incoming_slots: Arc<Semaphore>,
    outgoing_slots: Arc<Semaphore>,
    inbound_handler: InboundHandler,
    authorize: PeerAuthorizer,
    local_signing_key: Arc<SigningKey>,
    pub counters: Arc<TransportCounters>,
}

impl FederationTransport {
    pub fn new(
        _local_server_slug: impl Into<String>,
        local_public_key_hex: impl Into<String>,
        local_signing_key: Arc<SigningKey>,
        signal: SignalClient,
        inbound_handler: InboundHandler,
        authorize: PeerAuthorizer,
    ) -> Self {
        Self {
            local_public_key_hex: local_public_key_hex.into().to_lowercase(),
            signal,
            peers: Default::default(),
            connect_locks: Default::default(),
            pending_answers: Default::default(),
            pending_requests: Default::default(),
            assemblies: Default::default(),
            seen_signals: Default::default(),
            changed: Arc::new(Notify::new()),
            establishment_slots: Arc::new(Semaphore::new(8)),
            incoming_establishment_slots: Arc::new(Semaphore::new(8)),
            signal_slots: Arc::new(Semaphore::new(32)),
            incoming_slots: Arc::new(Semaphore::new(32)),
            outgoing_slots: Arc::new(Semaphore::new(32)),
            inbound_handler,
            authorize,
            local_signing_key,
            counters: Default::default(),
        }
    }

    fn own_tag(&self, bucket: u64) -> String {
        rendezvous_tag_for(&self.local_signing_key.verifying_key(), bucket)
    }

    fn drain_auth(&self, tag: &str) -> DrainAuth {
        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        let input = DrainAuth::canonical_signing_input(tag, timestamp_ms);
        DrainAuth {
            drain_pubkey_hex: self.local_public_key_hex.clone(),
            timestamp_ms,
            signature_b64: BASE64.encode(self.local_signing_key.sign(input.as_bytes()).to_bytes()),
        }
    }

    fn check_peer(&self, peer: &str) -> Result<(), TransportError> {
        if self.establishment_slots.is_closed() {
            return Err(TransportError::Protocol("transport stopped".into()));
        }
        (self.authorize)(peer).map_err(TransportError::SignalingRejected)
    }

    async fn poll_once(&self) -> Option<Duration> {
        let bucket = current_bucket();
        let prev_tag = self.own_tag(bucket.saturating_sub(1));
        // Bound a drain so an attacker cannot indefinitely starve the current queue.
        for _ in 0..128 {
            match self
                .signal
                .poll_tag(&prev_tag, 0, Some(&self.drain_auth(&prev_tag)))
                .await
            {
                Ok(Some(payload)) => self.dispatch_signal(payload),
                Ok(None) => break,
                Err(err) => {
                    tracing::warn!(%err, "previous federation queue drain failed");
                    break;
                }
            }
        }
        let tag = self.own_tag(bucket);
        match self
            .signal
            .poll_tag(&tag, 55, Some(&self.drain_auth(&tag)))
            .await
        {
            Ok(Some(payload)) => {
                self.dispatch_signal(payload);
                None
            }
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(%err, "federation signaling poll failed");
                Some(Duration::from_secs(2))
            }
        }
    }

    fn dispatch_signal(&self, payload: SignalingPayload) {
        // A slow ICE gather must not block answers for unrelated peers.
        let Ok(permit) = self.signal_slots.clone().try_acquire_owned() else {
            tracing::warn!("federation signaling concurrency limit reached");
            return;
        };
        let this = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(error) = this.handle_signal_payload(payload).await {
                tracing::warn!(%error, "federation signaling rejected");
            }
        });
    }

    pub fn spawn_signal_listener(
        self: Arc<Self>,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let maintenance_transport = self.clone();
            let maintenance_stop = shutdown.child_token();
            let maintenance_token = maintenance_stop.clone();
            let maintenance = tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(5));
                loop {
                    tokio::select! {
                        _ = maintenance_token.cancelled() => break,
                        _ = tick.tick() => maintenance_transport.sweep().await,
                    }
                }
            });
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    backoff = self.poll_once() => {
                        if let Some(delay) = backoff {
                            tokio::select! { _ = shutdown.cancelled() => break, _ = tokio::time::sleep(delay) => {} }
                        }
                    }
                }
            }
            maintenance_stop.cancel();
            let _ = maintenance.await;
            self.close().await;
        })
    }

    async fn sweep(&self) {
        self.assemblies
            .lock()
            .await
            .retain(|_, assembly| assembly.created.elapsed() < Duration::from_secs(30));
        self.seen_signals
            .lock()
            .await
            .retain(|_, created| created.elapsed() < Duration::from_secs(90));
        self.pending_requests
            .lock()
            .await
            .retain(|_, request| request.created.elapsed() < Duration::from_secs(90));
        let revoked: Vec<_> = self
            .peers
            .read()
            .await
            .iter()
            .filter(|(key, peer)| {
                self.check_peer(key).is_err()
                    || (peer.created.elapsed() > Duration::from_secs(40)
                        && peer
                            .dc
                            .as_ref()
                            .is_none_or(|dc| dc.ready_state() != RTCDataChannelState::Open))
            })
            .map(|(key, peer)| (key.clone(), peer.generation))
            .collect();
        for (key, generation) in revoked {
            self.remove_peer(&key, generation).await;
        }
    }

    pub async fn close(&self) {
        self.establishment_slots.close();
        self.incoming_establishment_slots.close();
        self.signal_slots.close();
        self.incoming_slots.close();
        self.outgoing_slots.close();
        let peers: Vec<_> = self
            .peers
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.generation))
            .collect();
        for (key, generation) in peers {
            self.remove_peer(&key, generation).await;
        }
        self.pending_answers.lock().await.clear();
        self.pending_requests.lock().await.clear();
        self.assemblies.lock().await.clear();
        self.changed.notify_waiters();
    }

    async fn remove_peer(&self, key: &str, generation: uuid::Uuid) {
        let removed = {
            let mut peers = self.peers.write().await;
            if peers.get(key).is_some_and(|p| p.generation == generation) {
                peers.remove(key)
            } else {
                None
            }
        };
        if let Some(peer) = removed {
            self.pending_answers
                .lock()
                .await
                .retain(|_, p| p.generation != generation);
            self.pending_requests
                .lock()
                .await
                .retain(|_, p| p.generation != generation);
            self.assemblies.lock().await.retain(|(p, _), _| p != key);
            self.changed.notify_waiters();
            let _ = peer.pc.close().await;
        }
    }

    async fn open_channel(&self, key: &str) -> Option<(uuid::Uuid, Arc<RTCDataChannel>)> {
        let peers = self.peers.read().await;
        let peer = peers.get(key)?;
        let dc = peer.dc.as_ref()?;
        (dc.ready_state() == RTCDataChannelState::Open).then(|| (peer.generation, dc.clone()))
    }

    pub async fn establish_peer(
        &self,
        remote_slug: &str,
        remote_key: &str,
    ) -> Result<(), TransportError> {
        let key = remote_key.to_lowercase();
        self.check_peer(&key)?;
        if self.open_channel(&key).await.is_some() {
            return Ok(());
        }
        let lock = {
            let mut locks = self.connect_locks.lock().await;
            locks.retain(|_, v| v.strong_count() > 0);
            match locks.get(&key).and_then(std::sync::Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(Mutex::new(()));
                    locks.insert(key.clone(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        let result = timeout(Duration::from_secs(40), async {
            let _guard = lock.lock().await;
            if self.open_channel(&key).await.is_some() {
                return Ok(());
            }
            let _slot = self
                .establishment_slots
                .acquire()
                .await
                .map_err(|_| TransportError::Protocol("transport stopped".into()))?;
            self.check_peer(&key)?;
            // Incoming negotiations already in flight may finish without another offer.
            if !self.peers.read().await.contains_key(&key) {
                self.initiate(&key).await?;
            }
            loop {
                let changed = self.changed.notified();
                self.check_peer(&key)?;
                if self.open_channel(&key).await.is_some() {
                    return Ok(());
                }
                if self.establishment_slots.is_closed() {
                    return Err(TransportError::Protocol("transport stopped".into()));
                }
                changed.await;
            }
        })
        .await
        .unwrap_or_else(|_| Err(TransportError::WaitForPeer(remote_slug.to_owned())));
        if result.is_err() && self.open_channel(&key).await.is_none() {
            let generation = self
                .peers
                .read()
                .await
                .get(&key)
                .filter(|p| p.created.elapsed() >= Duration::from_secs(35))
                .map(|p| p.generation);
            if let Some(generation) = generation {
                self.remove_peer(&key, generation).await;
            }
        }
        result
    }

    async fn initiate(&self, key: &str) -> Result<(), TransportError> {
        let generation = uuid::Uuid::new_v4();
        let pc = self
            .build_peer_connection(key.to_owned(), generation)
            .await?;
        {
            let mut peers = self.peers.write().await;
            if peers.contains_key(key) {
                drop(peers);
                let _ = pc.close().await;
                return Ok(());
            }
            peers.insert(
                key.to_owned(),
                Peer {
                    generation,
                    pc: pc.clone(),
                    dc: None,
                    outbound: true,
                    writes: Default::default(),
                    created: tokio::time::Instant::now(),
                },
            );
        }
        let result = async {
            let dc = pc
                .create_data_channel(
                    "annex-federation-rpc-v1",
                    Some(RTCDataChannelInit {
                        ordered: Some(true),
                        ..Default::default()
                    }),
                )
                .await
                .map_err(|e| TransportError::WebRtc(e.to_string()))?;
            self.bind_data_channel(key.to_owned(), generation, dc).await;
            let offer = pc
                .create_offer(None)
                .await
                .map_err(|e| TransportError::WebRtc(e.to_string()))?;
            let sdp = self.gather(&pc, offer).await?;
            let session = uuid::Uuid::new_v4().to_string();
            self.pending_answers.lock().await.insert(
                session.clone(),
                PendingAnswer {
                    peer: key.to_owned(),
                    generation,
                },
            );
            self.post_signal(key, session, "offer", &sdp).await
        }
        .await;
        if result.is_err() {
            self.remove_peer(key, generation).await;
            // Losing a simultaneous offer closes this PC. The selected incoming
            // generation is still negotiating and must survive the old failure.
            if self
                .peers
                .read()
                .await
                .get(key)
                .is_some_and(|p| p.generation != generation)
            {
                return Ok(());
            }
        }
        result
    }

    async fn gather(
        &self,
        pc: &Arc<RTCPeerConnection>,
        description: RTCSessionDescription,
    ) -> Result<String, TransportError> {
        let mut complete = pc.gathering_complete_promise().await;
        pc.set_local_description(description)
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
        timeout(Duration::from_secs(12), complete.recv())
            .await
            .map_err(|_| TransportError::WebRtc("ICE candidate gathering timed out".into()))?;
        pc.local_description()
            .await
            .map(|s| s.sdp)
            .ok_or_else(|| TransportError::WebRtc("gathered description is missing".into()))
    }

    async fn post_signal(
        &self,
        key: &str,
        session_id: String,
        kind: &str,
        sdp: &str,
    ) -> Result<(), TransportError> {
        self.check_peer(key)?;
        let recipient = verifying_key_from_hex(key)?;
        let mut payload = SignalingPayload {
            format_version: FORMAT_V2,
            from_server_slug: String::new(),
            to_server_slug: String::new(),
            rendezvous_tag: rendezvous_tag_for_now(&recipient),
            session_id,
            sdp_type: kind.to_owned(),
            sdp: seal_sdp(sdp, &recipient)?,
            sent_at_ms: chrono::Utc::now().timestamp_millis(),
            from_pubkey_hex: self.local_public_key_hex.clone(),
            vrp_signature: String::new(),
        };
        payload.vrp_signature = BASE64.encode(
            self.local_signing_key
                .sign(payload.canonical_signing_input().as_bytes())
                .to_bytes(),
        );
        self.signal.post_signal(&payload).await?;
        Ok(())
    }

    async fn handle_signal_payload(&self, payload: SignalingPayload) -> Result<(), TransportError> {
        if payload.format_version != FORMAT_V2
            || !payload.from_server_slug.is_empty()
            || !payload.to_server_slug.is_empty()
            || payload.session_id.len() > 64
            || uuid::Uuid::parse_str(&payload.session_id).is_err()
            || payload.sdp.len() > 30 * 1024
        {
            return Err(TransportError::SignalingRejected(
                "unsupported signal format or bounds".into(),
            ));
        }
        let bucket = current_bucket();
        if ![
            self.own_tag(bucket),
            self.own_tag(bucket.saturating_sub(1)),
            self.own_tag(bucket + 1),
        ]
        .contains(&payload.rendezvous_tag)
            || chrono::Utc::now()
                .timestamp_millis()
                .abs_diff(payload.sent_at_ms)
                > 60_000
        {
            return Err(TransportError::SignalingRejected(
                "wrong addressee or expired signal".into(),
            ));
        }
        let key = payload.from_pubkey_hex.to_lowercase();
        self.check_peer(&key)?;
        let signature = BASE64
            .decode(&payload.vrp_signature)
            .ok()
            .and_then(|b| Signature::from_slice(&b).ok())
            .ok_or_else(|| {
                TransportError::SignalingRejected("invalid signal signature encoding".into())
            })?;
        verifying_key_from_hex(&key)?
            .verify_strict(payload.canonical_signing_input().as_bytes(), &signature)
            .map_err(|_| TransportError::SignalingRejected("invalid signal signature".into()))?;
        let replay_key = format!("{key}:{}:{}", payload.session_id, payload.sdp_type);
        {
            let mut seen = self.seen_signals.lock().await;
            seen.retain(|_, created| created.elapsed() < Duration::from_secs(90));
            if seen.contains_key(&replay_key) {
                return Err(TransportError::SignalingRejected("replayed signal".into()));
            }
            if seen.len() >= 4096 {
                return Err(TransportError::SignalingRejected(
                    "signal rate limit".into(),
                ));
            }
            seen.insert(replay_key, tokio::time::Instant::now());
        }
        let sdp = open_sdp(&payload.sdp, &self.local_signing_key)?;
        match payload.sdp_type.as_str() {
            "answer" => {
                let pending = {
                    let mut answers = self.pending_answers.lock().await;
                    if answers
                        .get(&payload.session_id)
                        .is_none_or(|p| p.peer != key)
                    {
                        return Err(TransportError::SignalingRejected(
                            "answer does not match expected peer/session".into(),
                        ));
                    }
                    answers
                        .remove(&payload.session_id)
                        .expect("checked under lock")
                };
                let pc = self
                    .peers
                    .read()
                    .await
                    .get(&key)
                    .filter(|p| p.generation == pending.generation)
                    .map(|p| p.pc.clone());
                if let Some(pc) = pc {
                    pc.set_remote_description(
                        RTCSessionDescription::answer(sdp)
                            .map_err(|e| TransportError::WebRtc(e.to_string()))?,
                    )
                    .await
                    .map_err(|e| TransportError::WebRtc(e.to_string()))?;
                }
                Ok(())
            }
            "offer" => {
                let _slot = self
                    .incoming_establishment_slots
                    .try_acquire()
                    .map_err(|_| TransportError::Protocol("too many connection attempts".into()))?;
                self.accept_offer(&key, payload.session_id, sdp).await
            }
            _ => Err(TransportError::SignalingRejected("unknown SDP type".into())),
        }
    }

    async fn accept_offer(
        &self,
        key: &str,
        session: String,
        sdp: String,
    ) -> Result<(), TransportError> {
        let generation = uuid::Uuid::new_v4();
        let pc = self
            .build_peer_connection(key.to_owned(), generation)
            .await?;
        let replaced = {
            let mut peers = self.peers.write().await;
            // Simultaneous offers converge on the lower-key initiator. An already
            // established connection can always be replaced after the peer restarts.
            if peers.get(key).is_some_and(|p| {
                p.outbound
                    && p.dc
                        .as_ref()
                        .is_none_or(|dc| dc.ready_state() != RTCDataChannelState::Open)
            }) && self.local_public_key_hex.as_str() < key
            {
                drop(peers);
                let _ = pc.close().await;
                return Ok(());
            }
            peers.insert(
                key.to_owned(),
                Peer {
                    generation,
                    pc: pc.clone(),
                    dc: None,
                    outbound: false,
                    writes: Default::default(),
                    created: tokio::time::Instant::now(),
                },
            )
        };
        if let Some(old) = replaced {
            self.pending_answers
                .lock()
                .await
                .retain(|_, p| p.generation != old.generation);
            self.pending_requests
                .lock()
                .await
                .retain(|_, p| p.generation != old.generation);
            let _ = old.pc.close().await;
        }
        let result = async {
            pc.set_remote_description(
                RTCSessionDescription::offer(sdp)
                    .map_err(|e| TransportError::WebRtc(e.to_string()))?,
            )
            .await
            .map_err(|e| TransportError::WebRtc(e.to_string()))?;
            let answer = pc
                .create_answer(None)
                .await
                .map_err(|e| TransportError::WebRtc(e.to_string()))?;
            let sdp = self.gather(&pc, answer).await?;
            self.post_signal(key, session, "answer", &sdp).await
        }
        .await;
        if result.is_err() {
            self.remove_peer(key, generation).await;
        }
        self.changed.notify_waiters();
        result
    }

    async fn build_peer_connection(
        &self,
        key: String,
        generation: uuid::Uuid,
    ) -> Result<Arc<RTCPeerConnection>, TransportError> {
        let pc = Arc::new(
            APIBuilder::new()
                .build()
                .new_peer_connection(RTCConfiguration {
                    ice_servers: vec![RTCIceServer {
                        urls: vec!["stun:stun.l.google.com:19302".into()],
                        ..Default::default()
                    }],
                    ..Default::default()
                })
                .await
                .map_err(|e| TransportError::WebRtc(e.to_string()))?,
        );
        let this = self.clone();
        let remote = key.clone();
        pc.on_data_channel(Box::new(move |dc| {
            let this = this.clone();
            let remote = remote.clone();
            Box::pin(async move {
                this.bind_data_channel(remote, generation, dc).await;
            })
        }));
        let this = self.clone();
        pc.on_peer_connection_state_change(Box::new(move |state| {
            let this = this.clone();
            let key = key.clone();
            Box::pin(async move {
                if matches!(
                    state,
                    RTCPeerConnectionState::Failed
                        | RTCPeerConnectionState::Closed
                        | RTCPeerConnectionState::Disconnected
                ) {
                    // Do not close the PC from inside its own state callback.
                    tokio::spawn(async move {
                        this.remove_peer(&key, generation).await;
                    });
                }
            })
        }));
        Ok(pc)
    }

    async fn bind_data_channel(
        &self,
        key: String,
        generation: uuid::Uuid,
        dc: Arc<RTCDataChannel>,
    ) {
        if dc.label() != "annex-federation-rpc-v1" {
            let _ = dc.close().await;
            return;
        }
        {
            let mut peers = self.peers.write().await;
            match peers.get_mut(&key).filter(|p| p.generation == generation) {
                Some(peer) if peer.dc.is_none() => peer.dc = Some(dc.clone()),
                _ => {
                    drop(peers);
                    let _ = dc.close().await;
                    return;
                }
            }
        }
        let changed = self.changed.clone();
        dc.on_open(Box::new(move || {
            let changed = changed.clone();
            Box::pin(async move {
                changed.notify_waiters();
            })
        }));
        let this = self.clone();
        let remote = key.clone();
        dc.on_close(Box::new(move || {
            let this = this.clone();
            let remote = remote.clone();
            Box::pin(async move {
                tokio::spawn(async move {
                    this.remove_peer(&remote, generation).await;
                });
            })
        }));
        let this = self.clone();
        dc.on_message(Box::new(move |message| {
            let this = this.clone();
            let key = key.clone();
            Box::pin(async move {
                this.counters
                    .bytes_received
                    .fetch_add(message.data.len() as u64, Ordering::Relaxed);
                if let Err(error) = this.receive_chunk(&key, generation, &message.data).await {
                    tracing::warn!(peer=%key,%error,"federation data channel packet rejected");
                }
            })
        }));
        self.changed.notify_waiters();
    }

    async fn send_packet(
        &self,
        key: &str,
        generation: uuid::Uuid,
        packet: &Packet,
    ) -> Result<(), TransportError> {
        self.check_peer(key)?;
        let writes = self
            .peers
            .read()
            .await
            .get(key)
            .filter(|p| p.generation == generation)
            .map(|p| p.writes.clone())
            .ok_or_else(|| TransportError::NotConnected(key.into()))?;
        let _write = writes.lock().await;
        let (_, dc) = self
            .open_channel(key)
            .await
            .filter(|(g, _)| *g == generation)
            .ok_or_else(|| TransportError::NotConnected(key.into()))?;
        let bytes =
            serde_json::to_vec(packet).map_err(|e| TransportError::Protocol(e.to_string()))?;
        if bytes.len() > MAX_PACKET {
            return Err(TransportError::Protocol("RPC packet too large".into()));
        }
        let count = bytes.len().div_ceil(CHUNK_BYTES);
        let transfer = uuid::Uuid::new_v4().to_string();
        for (index, part) in bytes.chunks(CHUNK_BYTES).enumerate() {
            self.check_peer(key)?;
            let chunk = serde_json::to_string(&Chunk {
                transfer: transfer.clone(),
                index,
                count,
                data: BASE64.encode(part),
            })
            .map_err(|e| TransportError::Protocol(e.to_string()))?;
            // Bound SCTP's internal send queue as well as our reassembly buffers.
            while dc.buffered_amount().await > 256 * 1024 {
                if dc.ready_state() != RTCDataChannelState::Open {
                    return Err(TransportError::NotConnected(key.into()));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            dc.send_text(chunk.clone())
                .await
                .map_err(|e| TransportError::WebRtc(e.to_string()))?;
            self.counters
                .bytes_sent
                .fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        Ok(())
    }

    async fn receive_chunk(
        &self,
        key: &str,
        generation: uuid::Uuid,
        bytes: &[u8],
    ) -> Result<(), TransportError> {
        self.check_peer(key)?;
        if self
            .peers
            .read()
            .await
            .get(key)
            .is_none_or(|p| p.generation != generation)
        {
            return Err(TransportError::Protocol(
                "stale connection generation".into(),
            ));
        }
        if bytes.len() > 12 * 1024 {
            return Err(TransportError::Protocol("chunk too large".into()));
        }
        let chunk: Chunk =
            serde_json::from_slice(bytes).map_err(|e| TransportError::Protocol(e.to_string()))?;
        if chunk.transfer.len() > 64
            || uuid::Uuid::parse_str(&chunk.transfer).is_err()
            || chunk.count == 0
            || chunk.count > MAX_PACKET.div_ceil(CHUNK_BYTES)
            || chunk.index >= chunk.count
        {
            return Err(TransportError::Protocol("invalid chunk bounds".into()));
        }
        let data = BASE64
            .decode(chunk.data)
            .map_err(|_| TransportError::Protocol("invalid chunk encoding".into()))?;
        if data.len() > CHUNK_BYTES {
            return Err(TransportError::Protocol("decoded chunk too large".into()));
        }
        let full = {
            let mut assemblies = self.assemblies.lock().await;
            assemblies.retain(|_, a| a.created.elapsed() < Duration::from_secs(30));
            let address = (key.to_owned(), chunk.transfer);
            if !assemblies.contains_key(&address) {
                if chunk.index != 0
                    || assemblies.len() >= 8
                    || assemblies.keys().filter(|(p, _)| p == key).count() >= 2
                {
                    return Err(TransportError::Protocol(
                        "too many or unordered transfers".into(),
                    ));
                }
                assemblies.insert(
                    address.clone(),
                    Assembly {
                        created: tokio::time::Instant::now(),
                        count: chunk.count,
                        next: 0,
                        data: Vec::new(),
                    },
                );
            }
            if assemblies.values().map(|a| a.data.len()).sum::<usize>() + data.len()
                > 2 * MAX_PACKET
            {
                assemblies.remove(&address);
                return Err(TransportError::Protocol(
                    "reassembly capacity exceeded".into(),
                ));
            }
            let assembly = assemblies.get_mut(&address).expect("inserted under lock");
            if assembly.next != chunk.index
                || assembly.count != chunk.count
                || assembly.data.len() + data.len() > MAX_PACKET
            {
                assemblies.remove(&address);
                return Err(TransportError::Protocol("invalid transfer sequence".into()));
            }
            assembly.data.extend_from_slice(&data);
            assembly.next += 1;
            if assembly.next == assembly.count {
                Some(assemblies.remove(&address).expect("same lock").data)
            } else {
                None
            }
        };
        let Some(full) = full else {
            return Ok(());
        };
        let packet: Packet =
            serde_json::from_slice(&full).map_err(|e| TransportError::Protocol(e.to_string()))?;
        packet.verify(key, &self.local_public_key_hex)?;
        self.check_peer(key)?;
        if packet.response {
            let mut pending = self.pending_requests.lock().await;
            let matched = pending.get(&packet.id).is_some_and(|p| {
                p.peer == key
                    && p.generation == generation
                    && p.digest == packet.request_hash
                    && p.endpoint == packet.endpoint
            });
            if !matched {
                return Err(TransportError::Protocol(
                    "uncorrelated or replayed receipt".into(),
                ));
            }
            let request = pending.remove(&packet.id).expect("checked under lock");
            self.counters
                .receipts_verified
                .fetch_add(1, Ordering::Relaxed);
            let _ = request.reply.send(if packet.success {
                Ok(packet.body)
            } else {
                Err(ApplicationError::from_wire(packet.body))
            });
        } else {
            if !packet.request_hash.is_empty() || !packet.success {
                return Err(TransportError::Protocol("invalid request shape".into()));
            }
            let permit = self
                .incoming_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| TransportError::Protocol("too many incoming requests".into()))?;
            let this = self.clone();
            let key = key.to_owned();
            tokio::spawn(async move {
                let _permit = permit;
                let hash = packet.digest();
                let result = timeout(
                    Duration::from_secs(45),
                    (this.inbound_handler)(key.clone(), packet.endpoint.clone(), packet.body),
                )
                .await
                .unwrap_or_else(|_| {
                    Err(ApplicationError::new(
                        504,
                        "receiver operation timed out before acknowledgement",
                    ))
                });
                let success = result.is_ok();
                let body = result.unwrap_or_else(|e| e.wire());
                if body.len() > MAX_BODY {
                    return;
                }
                if success {
                    this.counters
                        .requests_committed
                        .fetch_add(1, Ordering::Relaxed);
                }
                let mut response = Packet {
                    version: 1,
                    response: true,
                    id: packet.id,
                    from: this.local_public_key_hex.clone(),
                    to: key.clone(),
                    endpoint: packet.endpoint,
                    request_hash: hash,
                    success,
                    body,
                    created_at_ms: chrono::Utc::now().timestamp_millis(),
                    signature: String::new(),
                };
                response.sign(&this.local_signing_key);
                let _ = timeout(
                    Duration::from_secs(30),
                    this.send_packet(&key, generation, &response),
                )
                .await;
            });
        }
        Ok(())
    }

    /// Returns only after the peer's authenticated handler has completed. This
    /// is the only operation the durable outbox may treat as delivered.
    pub async fn request(
        &self,
        remote_slug: &str,
        remote_key: &str,
        endpoint: &str,
        body: &str,
    ) -> Result<String, TransportError> {
        if body.len() > MAX_REQUEST {
            return Err(TransportError::Protocol("request body too large".into()));
        }
        let _permit = self
            .outgoing_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| TransportError::Protocol("too many outgoing requests".into()))?;
        let key = remote_key.to_lowercase();
        self.establish_peer(remote_slug, &key).await?;
        let (generation, _) = self
            .open_channel(&key)
            .await
            .ok_or_else(|| TransportError::NotConnected(key.clone()))?;
        let mut packet = Packet {
            version: 1,
            response: false,
            id: uuid::Uuid::new_v4().to_string(),
            from: self.local_public_key_hex.clone(),
            to: key.clone(),
            endpoint: endpoint.into(),
            request_hash: String::new(),
            success: true,
            body: body.into(),
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            signature: String::new(),
        };
        packet.sign(&self.local_signing_key);
        let (tx, rx) = oneshot::channel();
        self.pending_requests.lock().await.insert(
            packet.id.clone(),
            PendingRequest {
                peer: key.clone(),
                generation,
                digest: packet.digest(),
                endpoint: endpoint.into(),
                reply: tx,
                created: tokio::time::Instant::now(),
            },
        );
        let result = timeout(Duration::from_secs(60), async {
            self.send_packet(&key, generation, &packet).await?;
            rx.await
                .map_err(|_| TransportError::NotConnected(key.clone()))?
                .map_err(TransportError::RemoteRejected)
        })
        .await
        .unwrap_or_else(|_| Err(TransportError::Protocol("durable receipt timed out".into())));
        self.pending_requests.lock().await.remove(&packet.id);
        if result.is_err() && self.check_peer(&key).is_err() {
            self.remove_peer(&key, generation).await;
        }
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::rendezvous_tag_for;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn transport(
        key_byte: u8,
        handler: InboundHandler,
        authorize: PeerAuthorizer,
    ) -> FederationTransport {
        let key = Arc::new(SigningKey::from_bytes(&[key_byte; 32]));
        let public = key
            .verifying_key()
            .to_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        FederationTransport::new(
            "test",
            public,
            key,
            SignalClient::with_base_url("http://127.0.0.1:1").unwrap(),
            handler,
            authorize,
        )
    }

    async fn connect_actual_data_channel(a: &FederationTransport, b: &FederationTransport) {
        let ag = uuid::Uuid::new_v4();
        let bg = uuid::Uuid::new_v4();
        let apc = a
            .build_peer_connection(b.local_public_key_hex.clone(), ag)
            .await
            .unwrap();
        let bpc = b
            .build_peer_connection(a.local_public_key_hex.clone(), bg)
            .await
            .unwrap();
        a.peers.write().await.insert(
            b.local_public_key_hex.clone(),
            Peer {
                generation: ag,
                pc: apc.clone(),
                dc: None,
                outbound: true,
                writes: Default::default(),
                created: tokio::time::Instant::now(),
            },
        );
        b.peers.write().await.insert(
            a.local_public_key_hex.clone(),
            Peer {
                generation: bg,
                pc: bpc.clone(),
                dc: None,
                outbound: false,
                writes: Default::default(),
                created: tokio::time::Instant::now(),
            },
        );
        let dc = apc
            .create_data_channel(
                "annex-federation-rpc-v1",
                Some(RTCDataChannelInit {
                    ordered: Some(true),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        a.bind_data_channel(b.local_public_key_hex.clone(), ag, dc)
            .await;
        let offer = a
            .gather(&apc, apc.create_offer(None).await.unwrap())
            .await
            .unwrap();
        assert!(
            offer.contains("a=candidate:"),
            "the offered SDP must contain gathered candidates"
        );
        bpc.set_remote_description(RTCSessionDescription::offer(offer).unwrap())
            .await
            .unwrap();
        let answer = b
            .gather(&bpc, bpc.create_answer(None).await.unwrap())
            .await
            .unwrap();
        apc.set_remote_description(RTCSessionDescription::answer(answer).unwrap())
            .await
            .unwrap();
        timeout(Duration::from_secs(15), async {
            loop {
                if a.open_channel(&b.local_public_key_hex).await.is_some()
                    && b.open_channel(&a.local_public_key_hex).await.is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("real DTLS/SCTP data channel opens");
    }

    #[tokio::test]
    async fn actual_data_channel_waits_for_commit_chunks_large_payload_and_enforces_revocation() {
        let admitted = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let gate = admitted.clone();
        let authorize: PeerAuthorizer = Arc::new(move |_| {
            if gate.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err("agreement revoked".into())
            }
        });
        let entered = Arc::new(Notify::new());
        let commit = Arc::new(Notify::new());
        let receiver_entered = entered.clone();
        let receiver_commit = commit.clone();
        let a = transport(
            21,
            Arc::new(|_, _, body| Box::pin(async move { Ok(body) })),
            authorize.clone(),
        );
        let b = transport(
            22,
            Arc::new(move |_, _, body| {
                let entered = receiver_entered.clone();
                let commit = receiver_commit.clone();
                Box::pin(async move {
                    entered.notify_one();
                    commit.notified().await;
                    Ok(body)
                })
            }),
            authorize,
        );
        connect_actual_data_channel(&a, &b).await;
        // A maximum-size authority event plus its signed enclosing receipt
        // exceeds the former 2 MiB request / 8 MiB response ceilings.
        let payload =
            serde_json::json!({"accepted_event":"x".repeat(17 * 1024 * 1024)}).to_string();
        let sender = a.clone();
        let remote = b.local_public_key_hex.clone();
        let sent = payload.clone();
        let request = tokio::spawn(async move {
            sender
                .request("receiver", &remote, "/api/federation/messages", &sent)
                .await
        });
        timeout(Duration::from_secs(30), entered.notified())
            .await
            .unwrap();
        assert!(
            !request.is_finished(),
            "writing every chunk is not a durable delivery receipt"
        );
        assert_eq!(a.counters.receipts_verified.load(Ordering::Relaxed), 0);
        commit.notify_one();
        assert_eq!(
            timeout(Duration::from_secs(45), request)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            payload
        );
        assert_eq!(a.counters.receipts_verified.load(Ordering::Relaxed), 1);
        assert_eq!(b.counters.requests_committed.load(Ordering::Relaxed), 1);
        assert!(a.counters.bytes_sent.load(Ordering::Relaxed) > 17 * 1024 * 1024);
        assert!(a.counters.bytes_received.load(Ordering::Relaxed) > 17 * 1024 * 1024);
        admitted.store(false, Ordering::SeqCst);
        let rejected = a
            .request(
                "receiver",
                &b.local_public_key_hex,
                "/api/federation/messages",
                "after revocation",
            )
            .await
            .unwrap_err();
        assert!(rejected.to_string().contains("revoked"));
        b.sweep().await;
        assert!(b.peers.read().await.is_empty());
        a.close().await;
        b.close().await;
    }

    #[tokio::test]
    async fn actual_data_channel_preserves_signed_terminal_and_transient_refusal_status() {
        let a = transport(
            41,
            Arc::new(|_, _, body| Box::pin(async move { Ok(body) })),
            Arc::new(|_| Ok(())),
        );
        let b = transport(
            42,
            Arc::new(|_, _, body| {
                Box::pin(async move {
                    Err(ApplicationError::new(
                        body.parse::<u16>().unwrap(),
                        "controlled application refusal",
                    ))
                })
            }),
            Arc::new(|_| Ok(())),
        );
        connect_actual_data_channel(&a, &b).await;
        for status in [400, 401, 403, 404, 409, 422, 408, 425, 429, 503] {
            let error = a
                .request(
                    "receiver",
                    &b.local_public_key_hex,
                    "/api/federation/channel-authority/operations",
                    &status.to_string(),
                )
                .await
                .unwrap_err();
            match error {
                TransportError::RemoteRejected(error) => {
                    assert_eq!(error.status, status);
                    assert_eq!(error.message, "controlled application refusal");
                }
                other => panic!("authenticated semantic refusal was lost: {other}"),
            }
        }
        assert_eq!(a.counters.receipts_verified.load(Ordering::Relaxed), 10);
        assert_eq!(b.counters.requests_committed.load(Ordering::Relaxed), 0);
        a.close().await;
        b.close().await;
    }

    #[test]
    fn legacy_or_malformed_receipt_text_cannot_invent_a_terminal_status() {
        for body in [
            "HTTP 403: denied",
            r#"["untrusted",{"status":403,"message":"denied"}]"#,
            r#"["annex-federation-refusal-v1",{"status":200,"message":"denied"}]"#,
        ] {
            assert_eq!(ApplicationError::from_wire(body.into()).status, 502);
        }
        let error = ApplicationError::new(403, "denied");
        assert_eq!(ApplicationError::from_wire(error.wire()).status, 403);
    }

    #[tokio::test]
    async fn signaling_checks_peer_signature_before_opening_sealed_description() {
        let a = transport(
            31,
            Arc::new(|_, _, body| Box::pin(async move { Ok(body) })),
            Arc::new(|_| Ok(())),
        );
        let b = transport(
            32,
            Arc::new(|_, _, body| Box::pin(async move { Ok(body) })),
            Arc::new(|_| Ok(())),
        );
        let mut payload = SignalingPayload {
            format_version: FORMAT_V2,
            from_server_slug: String::new(),
            to_server_slug: String::new(),
            rendezvous_tag: b.own_tag(current_bucket()),
            session_id: uuid::Uuid::new_v4().to_string(),
            sdp_type: "offer".into(),
            sdp: seal_sdp("not an SDP", &b.local_signing_key.verifying_key()).unwrap(),
            sent_at_ms: chrono::Utc::now().timestamp_millis(),
            from_pubkey_hex: a.local_public_key_hex.clone(),
            vrp_signature: String::new(),
        };
        payload.vrp_signature = BASE64.encode(
            a.local_signing_key
                .sign(payload.canonical_signing_input().as_bytes())
                .to_bytes(),
        );
        payload.sdp_type = "answer".into();
        let error = b.handle_signal_payload(payload).await.unwrap_err();
        assert!(error.to_string().contains("invalid signal signature"));
        assert!(b.peers.read().await.is_empty());
    }

    #[test]
    fn sealed_sdp_hides_ip_and_length_and_round_trips() {
        let sk = SigningKey::generate(&mut OsRng);
        let vk = sk.verifying_key();
        let sdp = "v=0\r\na=candidate:1 1 udp 2122260223 203.0.113.7 54321 typ host";
        let sealed = seal_sdp(sdp, &vk).unwrap();
        // The relay never sees the IP inside the SDP.
        assert!(!sealed.contains("203.0.113.7"));
        // Padding makes the wire length independent of the SDP size.
        let big = "x".repeat(2000);
        assert_eq!(sealed.len(), seal_sdp(&big, &vk).unwrap().len());
        // The addressed recipient recovers the exact SDP.
        assert_eq!(open_sdp(&sealed, &sk).unwrap(), sdp);
    }

    #[test]
    fn a_non_recipient_cannot_open_the_sdp() {
        let sk = SigningKey::generate(&mut OsRng);
        let other = SigningKey::generate(&mut OsRng);
        let sealed = seal_sdp("v=0\r\n...", &sk.verifying_key()).unwrap();
        assert!(open_sdp(&sealed, &other).is_err());
    }

    #[test]
    fn sender_and_recipient_derive_the_same_rendezvous_tag() {
        // What a sender addresses (recipient's identity key) equals what the
        // recipient polls (its own key) — so tag-based routing actually meets.
        let recipient = SigningKey::generate(&mut OsRng);
        let recipient_vk = recipient.verifying_key();
        let sender_addresses = rendezvous_tag_for(&recipient_vk, 1234);
        let recipient_polls = rendezvous_tag_for(&recipient.verifying_key(), 1234);
        assert_eq!(sender_addresses, recipient_polls);
    }
}
