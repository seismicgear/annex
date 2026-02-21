//! Annex server library logic.

pub mod api;
pub mod api_admin;
pub mod api_agent;
pub mod api_channels;
pub mod api_federation;
pub mod api_graph;
pub mod api_observe;
pub mod api_rtx;
pub mod api_sse;
pub mod api_upload;
pub mod api_usernames;
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
    extract::DefaultBodyLimit,
    routing::{delete, get, patch, post, put},
    Extension, Json, Router,
};
use ed25519_dalek::SigningKey;
use middleware::RateLimiter;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
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
}

/// Emits an observe event to the database and broadcasts it to the SSE stream.
///
/// This is a convenience wrapper that calls [`annex_observe::emit_event`] and,
/// on success, sends the resulting [`annex_observe::PublicEvent`] through the
/// broadcast channel. Failures are logged as warnings but never block the
/// caller.
pub fn emit_and_broadcast(
    conn: &rusqlite::Connection,
    server_id: i64,
    entity_id: &str,
    payload: &annex_observe::EventPayload,
    observe_tx: &broadcast::Sender<annex_observe::PublicEvent>,
) {
    let domain = payload.domain();
    match annex_observe::emit_event(
        conn,
        server_id,
        domain,
        payload.event_type(),
        payload.entity_type(),
        entity_id,
        payload,
    ) {
        Ok(event) => {
            if let Err(e) = observe_tx.send(event) {
                tracing::warn!(
                    domain = domain.as_str(),
                    event_type = payload.event_type(),
                    "observe broadcast channel send failed (no receivers or lagged): {}",
                    e
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                domain = domain.as_str(),
                event_type = payload.event_type(),
                "failed to emit observe event: {}",
                e
            );
        }
    }
}

/// Parses a transfer scope string from the database into a [`VrpTransferScope`].
///
/// Returns `None` for unrecognized strings.
pub(crate) fn parse_transfer_scope(s: &str) -> Option<annex_vrp::VrpTransferScope> {
    s.parse().ok()
}

/// Maximum request body size (2 MiB). Protects against OOM from oversized payloads.
const MAX_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Health check handler.
async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
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
            "/api/channels/{channelId}/voice/status",
            get(api_channels::voice_status_handler),
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
        .route(
            "/api/rtx/subscribe",
            post(api_rtx::subscribe_handler).delete(api_rtx::unsubscribe_handler),
        )
        .route(
            "/api/rtx/subscriptions",
            get(api_rtx::get_subscription_handler),
        )
        .route(
            "/api/rtx/governance/transfers",
            get(api_rtx::governance_transfers_handler),
        )
        .route(
            "/api/rtx/governance/summary",
            get(api_rtx::governance_summary_handler),
        )
        .route(
            "/api/admin/policy",
            get(api_admin::get_policy_handler).put(api_admin::update_policy_handler),
        )
        .route(
            "/api/admin/server",
            get(api_admin::get_server_handler).patch(api_admin::rename_server_handler),
        )
        .route(
            "/api/admin/members",
            get(api_admin::list_members_handler),
        )
        .route(
            "/api/admin/members/{pseudonymId}/capabilities",
            patch(api_admin::update_member_capabilities_handler),
        )
        .route(
            "/api/profile/username",
            put(api_usernames::set_username_handler)
                .delete(api_usernames::delete_username_handler),
        )
        .route(
            "/api/profile/username/grant",
            post(api_usernames::grant_username_handler),
        )
        .route(
            "/api/profile/username/grant/{granteePseudonym}",
            delete(api_usernames::revoke_grant_handler),
        )
        .route(
            "/api/profile/username/grants",
            get(api_usernames::list_grants_handler),
        )
        .route(
            "/api/usernames/visible",
            get(api_usernames::get_visible_usernames_handler),
        )
        .layer(axum::middleware::from_fn(middleware::auth_middleware));

    // Upload routes need a larger body limit for media uploads.
    // The hard ceiling is 50 MiB; the handler enforces per-category limits from policy.
    let upload_routes = Router::new()
        .route(
            "/api/admin/server/image",
            post(api_upload::upload_server_image_handler),
        )
        .route(
            "/api/channels/{channelId}/upload",
            post(api_upload::upload_chat_handler),
        )
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
        .layer(axum::middleware::from_fn(middleware::auth_middleware));

    let router = Router::new()
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
        .route(
            "/api/federation/rtx",
            post(api_federation::receive_federated_rtx_handler),
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
        .route("/api/public/events", get(api_observe::get_events_handler))
        .route("/events/stream", get(api_observe::get_event_stream_handler))
        .route(
            "/api/public/server/summary",
            get(api_observe::get_server_summary_handler),
        )
        .route(
            "/api/public/federation/peers",
            get(api_observe::get_federation_peers_handler),
        )
        .route("/api/public/agents", get(api_observe::get_agents_handler))
        .route(
            "/api/public/server/image",
            get(api_upload::get_server_image_handler),
        )
        .merge(protected_routes)
        .merge(upload_routes)
        .route("/ws", get(api_ws::ws_handler));

    // Serve uploaded files (images, etc.) under /uploads/*
    let upload_dir = state.upload_dir.clone();
    let router = if std::path::Path::new(&upload_dir).exists() {
        tracing::info!(path = %upload_dir, "serving uploaded files at /uploads");
        router.nest_service("/uploads", ServeDir::new(&upload_dir))
    } else {
        tracing::info!(path = %upload_dir, "uploads directory not found yet (will be created on first upload)");
        router
    };

    // Serve client static files if the directory exists.
    // Configured via ANNEX_CLIENT_DIR env var; defaults to "client/dist".
    let client_dir = std::env::var("ANNEX_CLIENT_DIR")
        .unwrap_or_else(|_| "client/dist".to_string());
    if !std::path::Path::new(&client_dir).is_absolute() {
        tracing::warn!(
            path = %client_dir,
            "ANNEX_CLIENT_DIR is relative — static file serving depends on working directory; \
             consider using an absolute path"
        );
    }
    let router = if std::path::Path::new(&client_dir).join("index.html").exists() {
        tracing::info!(path = %client_dir, "serving client static files");
        let index = format!("{}/index.html", client_dir);
        router.fallback_service(
            ServeDir::new(&client_dir).fallback(ServeFile::new(index)),
        )
    } else {
        tracing::info!(path = %client_dir, "client directory not found, skipping static file serving");
        router
    };

    router
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(axum::middleware::from_fn(middleware::rate_limit_middleware))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(Extension(Arc::new(state)))
}
