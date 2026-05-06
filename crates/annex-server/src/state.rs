//! Shared application state.
//!
//! `AppState` is constructed once in [`crate::startup::prepare_server`] and
//! cloned (cheaply — every interior field is `Arc`-wrapped or `Copy`) into
//! the Axum extension that handlers extract from. The struct lives in its own
//! module so that route wiring, startup, and HTTP-layer construction can each
//! depend on it without depending on each other.

use std::sync::{Arc, Mutex, RwLock};

use annex_db::DbPool;
use annex_identity::zk::{Bn254, VerifyingKey};
use annex_identity::MerkleTree;
use annex_types::ServerPolicy;
use ed25519_dalek::SigningKey;
use tokio::sync::broadcast;

use crate::api_link_preview;
use crate::api_ws;
use crate::middleware::RateLimiter;

/// Application state shared across all request handlers.
#[derive(Clone)]
pub struct AppState {
    /// Database connection pool.
    pub pool: DbPool,
    /// In-memory Merkle tree state.
    pub merkle_tree: Arc<Mutex<MerkleTree>>,
    /// ZK Membership verification key (v1 — commitment-derived nullifier).
    pub membership_vkey: Arc<VerifyingKey<Bn254>>,
    /// ZK Membership verification key (v2 — secret-derived nullifier).
    ///
    /// `Some` only when `"v2"` is in `Config::security.enabled_zk_versions`
    /// at boot. The server uses this to verify proofs whose `protocol_version`
    /// field is `"v2"`; an incoming `"v2"` payload on a server where this is
    /// `None` is rejected with `409 Conflict`. v1 and v2 are NEVER merged
    /// silently — each proof is dispatched to exactly one verifier by its
    /// declared protocol version.
    pub membership_vkey_v2: Option<Arc<VerifyingKey<Bn254>>>,
    /// The local server ID.
    pub server_id: i64,
    /// The local server signing key (Ed25519).
    pub signing_key: Arc<SigningKey>,
    /// The public URL of the server.
    ///
    /// Wrapped in `Arc<RwLock<_>>` so that when no explicit URL is configured,
    /// the server can auto-detect it from the first incoming HTTP request's
    /// `Host` / `X-Forwarded-Host` headers.
    pub public_url: Arc<RwLock<String>>,
    /// Server policy configuration.
    pub policy: Arc<RwLock<ServerPolicy>>,
    /// Rate limiter state.
    pub rate_limiter: RateLimiter,
    /// Connection manager for WebSockets.
    pub connection_manager: api_ws::ConnectionManager,
    /// Broadcast channel for presence events.
    pub presence_tx: broadcast::Sender<annex_types::PresenceEvent>,
    /// Voice service.
    pub voice_service: Arc<annex_voice::VoiceService>,
    /// TTS service.
    pub tts_service: Arc<annex_voice::TtsService>,
    /// STT service.
    pub stt_service: Arc<annex_voice::SttService>,
    /// Active agent voice sessions (pseudonym -> client).
    ///
    /// Uses `std::sync::RwLock` intentionally: all lock acquisitions are brief
    /// HashMap operations (get/insert/remove) that never span `.await` points,
    /// making a synchronous lock safe and more efficient than `tokio::sync::RwLock`.
    pub voice_sessions:
        Arc<RwLock<std::collections::HashMap<String, Arc<annex_voice::AgentVoiceClient>>>>,
    /// Broadcast channel for public observe events (SSE stream).
    pub observe_tx: broadcast::Sender<annex_observe::PublicEvent>,
    /// Directory for uploaded files (images, etc.).
    pub upload_dir: String,
    /// In-memory cache for link preview metadata and proxied images.
    pub preview_cache: api_link_preview::PreviewCache,
    /// HMAC secret for signing WebSocket session tokens. Derived at startup
    /// from the server's Ed25519 key to avoid managing a separate secret.
    pub ws_token_secret: Arc<[u8; 32]>,
    /// Configured CORS allowed origins (empty = same-origin only, ["*"] = permissive).
    pub cors_origins: Vec<String>,
    /// When true, channel access endpoints require ZK membership proof via
    /// the `x-annex-zk-proof` header.
    pub enforce_zk_proofs: bool,
    /// Base URL for generated invite links (e.g. "https://monolithannex.com/invite").
    pub invite_base_url: String,
}

impl AppState {
    /// Returns the current public URL, or an empty string if not yet detected.
    pub fn get_public_url(&self) -> String {
        self.public_url
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}
