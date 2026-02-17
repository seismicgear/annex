//! Annex server library logic.

pub mod api;
pub mod api_admin;
pub mod api_agent;
pub mod api_channels;
pub mod api_federation;
pub mod api_graph;
pub mod api_rtx;
pub mod api_sse;
pub mod api_vrp;
pub mod api_ws;
pub mod background;
pub mod config;
pub mod middleware;
pub mod policy;
pub mod retention;

use annex_db::DbPool;
use annex_identity::zk::{Bn254, VerifyingKey};
use annex_identity::MerkleTree;
use annex_types::ServerPolicy;
use axum::{
    routing::{get, post, put},
    Extension, Json, Router,
};
use ed25519_dalek::SigningKey;
use middleware::RateLimiter;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::broadcast;

/// Application state shared across all request handlers.
#[derive(Clone)]
pub struct AppState {
    /// Database connection pool.
    pub pool: DbPool,
    /// In-memory Merkle tree state.
    pub merkle_tree: Arc<Mutex<MerkleTree>>,
    /// ZK Membership verification key.
    pub membership_vkey: Arc<VerifyingKey<Bn254>>,
    /// The local server ID.
    pub server_id: i64,
    /// The local server signing key (Ed25519).
    pub signing_key: Arc<SigningKey>,
    /// The public URL of the server.
    pub public_url: String,
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
    pub voice_sessions:
        Arc<RwLock<std::collections::HashMap<String, Arc<annex_voice::AgentVoiceClient>>>>,
}

/// Health check handler.
async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": "0.0.1"
    }))
}

/// Builds the application router with all routes.
pub fn app(state: AppState) -> Router {
    let protected_routes = Router::new()
        .route(
            "/api/channels",
            post(api_channels::create_channel_handler).get(api_channels::list_channels_handler),
        )
        .route(
            "/api/channels/{channelId}",
            get(api_channels::get_channel_handler).delete(api_channels::delete_channel_handler),
        )
        .route(
            "/api/channels/{channelId}/join",
            post(api_channels::join_channel_handler),
        )
        .route(
            "/api/channels/{channelId}/voice/join",
            post(api_channels::join_voice_channel_handler),
        )
        .route(
            "/api/channels/{channelId}/voice/leave",
            post(api_channels::leave_voice_channel_handler),
        )
        .route(
            "/api/channels/{channelId}/leave",
            post(api_channels::leave_channel_handler),
        )
        .route(
            "/api/channels/{channelId}/messages",
            get(api_channels::get_channel_history_handler),
        )
        .route(
            "/api/agents/{pseudonymId}",
            get(api_agent::get_agent_profile_handler),
        )
        .route(
            "/api/agents/{pseudonymId}/voice-profile",
            put(api_agent::update_agent_voice_profile_handler),
        )
        .route("/api/rtx/publish", post(api_rtx::publish_handler))
        .route("/api/admin/policy", put(api_admin::update_policy_handler))
        .layer(axum::middleware::from_fn(middleware::auth_middleware));

    Router::new()
        .route("/health", get(health))
        .route("/api/registry/register", post(api::register_handler))
        .route(
            "/api/registry/path/{commitmentHex}",
            get(api::get_path_handler),
        )
        .route(
            "/api/registry/current-root",
            get(api::get_current_root_handler),
        )
        .route(
            "/api/zk/verify-membership",
            post(api::verify_membership_handler),
        )
        .route("/api/registry/topics", get(api::get_topics_handler))
        .route("/api/registry/roles", get(api::get_roles_handler))
        .route(
            "/api/identity/{pseudonymId}",
            get(api::get_identity_handler),
        )
        .route(
            "/api/identity/{pseudonymId}/capabilities",
            get(api::get_identity_capabilities_handler),
        )
        .route(
            "/api/vrp/agent-handshake",
            post(api_vrp::agent_handshake_handler),
        )
        .route(
            "/api/federation/handshake",
            post(api_federation::federation_handshake_handler),
        )
        .route(
            "/api/federation/vrp-root",
            get(api_federation::get_vrp_root_handler),
        )
        .route(
            "/api/federation/attest-membership",
            post(api_federation::attest_membership_handler),
        )
        .route(
            "/api/federation/channels",
            get(api_federation::get_federated_channels_handler),
        )
        .route(
            "/api/federation/channels/{channelId}/join",
            post(api_federation::join_federated_channel_handler),
        )
        .route(
            "/api/federation/messages",
            post(api_federation::receive_federated_message_handler),
        )
        .route("/api/graph/degrees", get(api_graph::get_degrees_handler))
        .route(
            "/api/graph/profile/{targetPseudonym}",
            get(api_graph::get_profile_handler),
        )
        .route(
            "/events/presence",
            get(api_sse::get_presence_stream_handler),
        )
        .merge(protected_routes)
        .route("/ws", get(api_ws::ws_handler))
        .layer(axum::middleware::from_fn(middleware::rate_limit_middleware))
        .layer(Extension(Arc::new(state)))
}
