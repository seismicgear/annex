//! Route wiring.
//!
//! [`app`] assembles the full Axum router from per-feature handler modules,
//! attaches the static-file mounts, and applies the global layer chain. It
//! deliberately does no I/O of its own — startup-time work (database,
//! Merkle tree, key loading, channel creation) lives in
//! [`crate::startup::prepare_server`], and HTTP-layer construction (CORS,
//! body limits, middleware) lives under [`crate::http`].

use std::sync::Arc;

use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, patch, post, put},
    Extension, Json, Router,
};
use serde_json::{json, Value};

use crate::api;
use crate::api_admin;
use crate::api_agent;
use crate::api_channels;
use crate::api_federation;
use crate::api_graph;
use crate::api_invite;
use crate::api_link_preview;
use crate::api_observe;
use crate::api_rtx;
use crate::api_sse;
use crate::api_upload;
use crate::api_usernames;
use crate::api_vrp;
use crate::api_ws;
use crate::http::cors::build_cors_layer;
use crate::http::layers::apply_global_layers;
use crate::http::static_files::{attach_client_dist, attach_uploads};
use crate::middleware;
use crate::state::AppState;

/// Health check handler.
///
/// Reports basic server liveness, version, and whether voice (WebRTC) is configured.
async fn health(Extension(state): Extension<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "voice_enabled": state.voice_service.is_enabled()
    }))
}

/// Voice configuration status (public, no auth required).
///
/// Reports both the server policy voice setting and whether the WebRTC
/// infrastructure is configured, so the client can distinguish between
/// "voice disabled by admin" and "voice enabled but needs WebRTC setup".
async fn voice_config_status(Extension(state): Extension<Arc<AppState>>) -> Json<Value> {
    let infrastructure_ready = state.voice_service.is_enabled();
    // get_public_url() now returns "" for loopback-only URLs, so
    // has_public_url is false when only a loopback endpoint exists.
    let has_public_url = !state.voice_service.get_public_url().is_empty();
    // Also report whether a URL for local clients exists (includes loopback)
    let has_local_url = !state.voice_service.get_url_for_local_client().is_empty();
    let policy_enabled = state
        .policy
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .voice_enabled;

    let setup_hint = if !policy_enabled {
        "Voice is disabled in the server policy. An admin can enable it in Server Policy settings."
    } else if !infrastructure_ready {
        "Voice is enabled by policy but WebRTC is not configured. Set webrtc.url, webrtc.api_key, and webrtc.api_secret in config.toml or use ANNEX_WEBRTC_* environment variables."
    } else if !has_public_url && has_local_url {
        "WebRTC is configured with a loopback-only URL. Voice works for the host but remote users who join via invite will not be able to connect to calls. Set webrtc.public_url in config.toml to a publicly reachable WebSocket address, or set ANNEX_WEBRTC_PUBLIC_URL."
    } else if !has_public_url {
        "WebRTC URL is configured but no public URL is set. Clients may not be able to connect."
    } else {
        "Voice is configured and ready."
    };

    Json(json!({
        "voice_enabled": policy_enabled && infrastructure_ready,
        "policy_enabled": policy_enabled,
        "infrastructure_ready": infrastructure_ready,
        "has_public_url": has_public_url,
        "has_local_url": has_local_url,
        "setup_hint": setup_hint
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
            "/api/messages/search",
            get(api_channels::search_messages_handler),
        )
        .route(
            "/api/channels/{channelId}/messages",
            get(api_channels::get_channel_history_handler),
        )
        .route(
            "/api/channels/{channelId}/messages/{messageId}/edits",
            get(api_channels::get_message_edits_handler),
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
            "/api/admin/public-url",
            put(api_admin::set_public_url_handler),
        )
        .route(
            "/api/admin/webrtc-public-url",
            put(api_admin::set_webrtc_public_url_handler),
        )
        .route(
            "/api/admin/federation/{id}",
            delete(api_admin::revoke_federation_handler),
        )
        .route("/api/admin/members", get(api_admin::list_members_handler))
        .route(
            "/api/admin/members/{pseudonymId}/capabilities",
            patch(api_admin::update_member_capabilities_handler),
        )
        .route(
            "/api/profile/username",
            put(api_usernames::set_username_handler).delete(api_usernames::delete_username_handler),
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
        .route(
            "/api/link-preview",
            get(api_link_preview::link_preview_handler),
        )
        .route(
            "/api/invites",
            post(api_invite::create_invite_handler).get(api_invite::list_invites_handler),
        )
        .route(
            "/api/invites/{code}",
            delete(api_invite::delete_invite_handler),
        )
        .route("/api/ws/token", post(api_ws::create_ws_token_handler))
        .route(
            "/api/graph/profile/{targetPseudonym}",
            get(api_graph::get_profile_handler),
        )
        .route(
            "/events/presence",
            get(api_sse::get_presence_stream_handler),
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
        .route(
            "/api/session/refresh",
            post(api_ws::refresh_session_handler),
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
            "/api/invites/redeem",
            post(api_invite::redeem_invite_handler),
        )
        .route("/api/voice/config-status", get(voice_config_status))
        .route(
            "/api/public/server/image",
            get(api_upload::get_server_image_handler),
        )
        // Image proxy lives outside auth — browsers load <img src="..."> without
        // custom headers.  The handler already validates URLs (SSRF, DNS rebinding,
        // content-type, size) and only proxies public images.
        .route(
            "/api/link-preview/image",
            get(api_link_preview::image_proxy_handler),
        )
        .merge(protected_routes)
        .merge(upload_routes)
        .route("/ws", get(api_ws::ws_handler));

    // Static-file mounts: /uploads/* (when the dir exists) and the SPA
    // fallback (when ANNEX_CLIENT_DIR/index.html exists).
    let router = attach_uploads(router, &state.upload_dir);
    let router = attach_client_dist(router);

    let cors_origins = state.cors_origins.clone();
    let shared_state = Arc::new(state);

    let cors_layer = build_cors_layer(&cors_origins);

    apply_global_layers(router, shared_state, cors_layer)
}
