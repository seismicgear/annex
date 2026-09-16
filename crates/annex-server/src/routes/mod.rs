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
use crate::api_e2e;
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
use crate::api_zk_circuits;
use crate::http::cors::build_cors_layer;
use crate::http::layers::apply_global_layers;
use crate::http::static_files::{attach_client_dist, attach_uploads};
use crate::middleware;
use crate::state::AppState;

/// Voice configuration status (public, no auth required).
///
/// Reports both the server policy voice setting and whether the WebRTC
/// infrastructure is configured, so the client can distinguish between
/// "voice disabled by admin" and "voice enabled but needs WebRTC setup".
///
/// Also reports `stt_ready` — whether the whisper.cpp binary and GGML
/// model file are both present and the binary is executable — and
/// `stt_detail`, which names the specific file when it is not.
/// Previously the response implied voice (and transcription) was ready
/// as long as WebRTC was configured, even though the Docker image set
/// `ANNEX_STT_MODEL_PATH` to a model it never copied in, so the first
/// transcription attempt would fail. `stt_ready` surfaces that mismatch
/// up to the client; `stt_detail` tells the operator which of the four
/// things it can be.
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
    let stt_readiness = state.stt_service.readiness();
    let stt_ready = stt_readiness.is_ready();

    let setup_hint: String = if !policy_enabled {
        "Voice is disabled in the server policy. An admin can enable it in Server Policy settings."
            .to_string()
    } else if !infrastructure_ready {
        "Voice is enabled by policy but WebRTC is not configured. Set webrtc.url, webrtc.api_key, and webrtc.api_secret in config.toml or use ANNEX_WEBRTC_* environment variables.".to_string()
    } else if !has_public_url && has_local_url {
        "WebRTC is configured with a loopback-only URL. Voice works for the host but remote users who join via invite will not be able to connect to calls. Set webrtc.public_url in config.toml to a publicly reachable WebSocket address, or set ANNEX_WEBRTC_PUBLIC_URL.".to_string()
    } else if !has_public_url {
        "WebRTC URL is configured but no public URL is set. Clients may not be able to connect."
            .to_string()
    } else if !stt_ready {
        // Not "the binary or the model is missing" — which of the two,
        // by path, and what to run. The operator reading this is the
        // person who can fix it.
        format!(
            "Voice is ready, but live captions are not: {}",
            stt_readiness.detail()
        )
    } else {
        "Voice is configured and ready.".to_string()
    };

    Json(json!({
        "voice_enabled": policy_enabled && infrastructure_ready,
        "policy_enabled": policy_enabled,
        "infrastructure_ready": infrastructure_ready,
        "has_public_url": has_public_url,
        "has_local_url": has_local_url,
        "stt_ready": stt_ready,
        // `stt_ready` stays a bare bool for wire compatibility with
        // clients that already read it; `stt_detail` is the sentence
        // naming the specific file.
        "stt_detail": stt_readiness.detail(),
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
        .route("/api/metrics", get(crate::api_metrics::metrics))
        .route(
            "/api/uploads/grant",
            post(crate::api_uploads_access::issue_upload_grant),
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
        .route(
            "/api/admin/federation/outbox",
            get(api_admin::list_federation_outbox_handler),
        )
        .route(
            "/api/admin/federation/outbox/{id}/retry",
            post(api_admin::retry_federation_outbox_handler),
        )
        .route(
            "/api/admin/storage",
            get(api_admin::get_storage_health_handler),
        )
        .route(
            "/api/admin/storage/clear",
            post(api_admin::clear_storage_gate_handler),
        )
        .route("/api/admin/members", get(api_admin::list_members_handler))
        .route(
            "/api/admin/members/{pseudonymId}/capabilities",
            patch(api_admin::update_member_capabilities_handler),
        )
        .route(
            "/api/admin/members/{pseudonymId}/revoke-sessions",
            post(api_admin::revoke_member_sessions_handler),
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
        // The trust graph is member-only. `BfsPath.path` names the
        // pseudonyms BETWEEN two people — parties to neither end of the query
        // — so serving it anonymously let anyone who could reach the server
        // walk pairs and reconstruct the social graph of a platform built on
        // pseudonymity. It was in `public_routes`; no client code ever called
        // it, so nothing depended on that.
        .route("/api/graph/degrees", get(api_graph::get_degrees_handler))
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
        // ── End-to-end encrypted channels (content-blind key distribution) ──
        .route("/api/keys/me", put(api_e2e::put_my_key_handler))
        .route(
            "/api/keys/{pseudonymId}",
            get(api_e2e::get_member_key_handler),
        )
        .route(
            "/api/channels/{channelId}/member-keys",
            get(api_e2e::list_channel_member_keys_handler),
        )
        .route(
            "/api/channels/{channelId}/key-wraps",
            get(api_e2e::get_channel_key_wraps_handler)
                .post(api_e2e::post_channel_key_wraps_handler),
        )
        .route(
            "/api/channels/{channelId}/key-status",
            get(api_e2e::get_channel_key_status_handler),
        )
        .route(
            "/api/channels/{channelId}/e2e",
            get(api_e2e::get_channel_e2e_handler).put(api_e2e::set_channel_e2e_handler),
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
        // Layer order matters. Tower applies layers in reverse: the LAST
        // `.layer()` call wraps the OUTERMOST layer, which executes FIRST
        // on the inbound request. We want auth to run first (so it can
        // attach IdentityContext to extensions), then rate_limit (so it
        // can key off the pseudonym), then the handler. That means in
        // code we write rate_limit FIRST and auth LAST.
        //
        // Without this ordering the pseudonym branch in rate_limit_middleware
        // is dead code: the global rate-limit layer (see http/layers.rs) runs
        // BEFORE per-route auth, so IdentityContext is never present and
        // every protected request gets keyed by IP. This block restores the
        // per-pseudonym budget for authenticated users while still letting
        // the global IP layer act as a cheap upstream cap.
        .layer(axum::middleware::from_fn(middleware::rate_limit_middleware))
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
        // Same layer ordering as protected_routes: auth first, then
        // pseudonym-aware rate limit, then the upload handler.
        .layer(axum::middleware::from_fn(middleware::rate_limit_middleware))
        .layer(axum::middleware::from_fn(middleware::auth_middleware));

    // Public routes — no auth_middleware. They still need a rate-limit
    // pass; with no `IdentityContext` available, `rate_limit_middleware`
    // falls back to IP keying, which is the correct upstream cap for
    // anonymous traffic.
    let public_routes = Router::new()
        // `/health` keeps its old handler and its old body. Four things poll
        // it — e2e-server.sh's readiness loop, startup.spec.ts, the puppeteer
        // harness, and the Docker healthcheck — and none of them is asking the
        // question `/readyz` answers.
        .route("/health", get(crate::api_health::live))
        .route("/livez", get(crate::api_health::live))
        .route("/readyz", get(crate::api_health::ready))
        // `/metrics` is mounted in BOTH groups and each half refuses when it
        // is not the one in force: the public handler 404s unless
        // ANNEX_METRICS_PUBLIC is set, and the authenticated one 403s a
        // non-moderator unless it is. Mounting both unconditionally keeps the
        // route table independent of process environment — a router built
        // under one setting and served under another still behaves — and
        // means the path never simply vanishes, which reads to an operator as
        // a broken build rather than a policy.
        .route("/metrics", get(crate::api_metrics::metrics_public))
        // Not behind `auth_middleware` because a browser cannot attach an
        // Authorization header to `<img src>` — which is the whole constraint
        // that shapes this. The handler authorises internally against a signed
        // grant plus a LIVE membership read, so leaving a channel takes effect
        // on the next request rather than whenever a token expires.
        .route(
            "/uploads/chat/{category}/{filename}",
            get(crate::api_uploads_access::serve_chat_upload),
        )
        .route("/api/registry/register", post(api::register_handler))
        .route(
            "/api/registry/path/{commitmentHex}",
            get(api::get_path_handler),
        )
        .route(
            "/api/registry/current-root",
            get(api::get_current_root_handler),
        )
        // Issued before a membership proof is generated, spent by the proof
        // that follows. Public because a member signing in has no session yet;
        // it discloses only a random number, and the outstanding-set cap in
        // `api_zk_challenge` bounds what an unauthenticated caller can store.
        .route(
            "/api/zk/challenge",
            post(crate::api_zk_challenge::issue_challenge_handler),
        )
        .route(
            "/api/zk/verify-membership",
            post(api::verify_membership_handler),
        )
        // Capability / linkage / federation ZK circuits (AUDIT P4-ID-1).
        .route(
            "/api/zk/channel-eligibility",
            post(api_zk_circuits::channel_eligibility_handler),
        )
        .route(
            "/api/zk/link-pseudonyms",
            post(api_zk_circuits::link_pseudonyms_handler),
        )
        .route(
            "/api/zk/federation-attestation",
            post(api_zk_circuits::federation_attestation_handler),
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
            "/api/federation/redactions",
            post(api_federation::receive_federated_redaction_handler),
        )
        .route(
            "/api/federation/edits",
            post(api_federation::receive_federated_edit_handler),
        )
        .route(
            "/api/federation/rtx",
            post(api_federation::receive_federated_rtx_handler),
        )
        .route("/api/public/events", get(api_observe::get_events_handler))
        .route(
            "/api/public/events/chain",
            get(api_observe::get_events_chain_handler),
        )
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
        // Rate-limit the public routes by IP (no auth runs, so the
        // middleware's IdentityContext branch is never taken).
        .layer(axum::middleware::from_fn(middleware::rate_limit_middleware));

    // WebSocket route is mounted separately: the upgrade handshake
    // authenticates via a one-shot WS token (issued by
    // `/api/ws/token`), so it does not go through `auth_middleware`.
    // Per-message limits live inside `api_ws`.
    let ws_routes = Router::new().route("/ws", get(api_ws::ws_handler));

    let router = public_routes
        .merge(protected_routes)
        .merge(upload_routes)
        .merge(ws_routes);

    // Static-file mounts: /uploads/* (when the dir exists) and the SPA
    // fallback (when ANNEX_CLIENT_DIR/index.html exists).
    let router = attach_uploads(router, &state.upload_dir);
    let router = attach_client_dist(router);

    let cors_origins = state.cors_origins.clone();
    let shared_state = Arc::new(state);

    let cors_layer = build_cors_layer(&cors_origins);

    apply_global_layers(router, shared_state, cors_layer)
}
