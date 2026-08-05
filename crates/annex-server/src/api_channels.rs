//! HTTP handlers for the `/api/channels/*` and `/api/messages/search`
//! routes. Every handler is intentionally thin: extract `Extension`,
//! deserialize, hand off to [`ChannelService`], render the success body or
//! the matching status code on failure.
//!
//! Two wire-shape concerns are preserved deliberately:
//!
//!   1. Non-voice handlers continue to return bare `StatusCode` on errors
//!      (empty body), matching the previous behaviour. We use
//!      [`ChannelServiceError::status_code`] rather than the
//!      `From<ChannelServiceError> for ApiError` impl so the
//!      `{"error": …}` body shape never appears here.
//!   2. `POST /api/channels/:id/voice/join` keeps its structured-JSON
//!      error body (`{error, message, setup_hint}`) for the
//!      `voice_disabled` and `voice_not_configured` cases. Those two
//!      service variants are matched explicitly.

use crate::middleware::IdentityContext;
use crate::services::{ChannelService, ChannelServiceError};
use crate::AppState;
use annex_channels::{Channel, Message, MessageEdit};
use axum::{
    extract::{Extension, Path, Query},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

// Re-exports kept so the route table in `routes/mod.rs` and integration
// tests that name these types directly continue to compile unchanged.
pub use crate::services::channel_service::{
    ChannelWithMembership, CreateChannelRequest, IceServerResponse, JoinVoiceResponse,
    VoiceStatusResponse,
};

#[derive(Deserialize)]
pub struct HistoryParams {
    pub before: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: String,
    pub channel_id: Option<String>,
    pub limit: Option<u32>,
}

/// Maps a [`ChannelServiceError`] from the non-voice handlers to a bare
/// `StatusCode`, preserving the previous empty-body error contract.
fn err_to_status(e: ChannelServiceError) -> StatusCode {
    e.status_code()
}

/// `POST /api/channels`
pub async fn create_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Json(payload): Json<CreateChannelRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let svc = ChannelService::new(state.clone());
    let outcome = svc
        .create_channel(&identity, payload)
        .await
        .map_err(err_to_status)?;

    // Broadcast channel_created so subscribed clients refresh their lists.
    let out = crate::api_ws::OutgoingMessage::ChannelCreated {
        channel: outcome.broadcast_payload,
    };
    if let Ok(broadcast_json) = serde_json::to_string(&out) {
        state.connection_manager.broadcast_all(broadcast_json).await;
    }

    Ok(Json(json!({"status": "created"})))
}

/// `GET /api/channels`
pub async fn list_channels_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Json<Vec<ChannelWithMembership>>, StatusCode> {
    let svc = ChannelService::new(state);
    let channels = svc
        .list_channels(&identity.pseudonym_id)
        .await
        .map_err(err_to_status)?;
    Ok(Json(channels))
}

/// `GET /api/channels/:channelId`
///
/// Moderators can view any channel; regular users must be members,
/// preventing metadata leakage on private channels.
pub async fn get_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Json<Channel>, StatusCode> {
    let svc = ChannelService::new(state);
    let channel = svc
        .get_channel(&identity, &channel_id)
        .await
        .map_err(err_to_status)?;
    Ok(Json(channel))
}

/// `DELETE /api/channels/:channelId`
pub async fn delete_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let svc = ChannelService::new(state.clone());
    svc.delete_channel(&identity, &channel_id)
        .await
        .map_err(err_to_status)?;

    // Unsubscribe live websocket subscribers and broadcast the deletion.
    state
        .connection_manager
        .unsubscribe_channel(&channel_id)
        .await;

    let out = crate::api_ws::OutgoingMessage::ChannelDeleted {
        channel_id: channel_id.clone(),
    };
    if let Ok(broadcast_json) = serde_json::to_string(&out) {
        state.connection_manager.broadcast_all(broadcast_json).await;
    }

    Ok(Json(json!({"status": "deleted"})))
}

/// `GET /api/channels/:channelId/messages`
pub async fn get_channel_history_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    headers: axum::http::HeaderMap,
    Path(channel_id): Path<String>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Vec<Message>>, StatusCode> {
    let svc = ChannelService::new(state);
    let messages = svc
        .get_history(
            &identity,
            &headers,
            &channel_id,
            params.before,
            params.limit,
        )
        .await
        .map_err(err_to_status)?;
    Ok(Json(messages))
}

/// `POST /api/channels/:channelId/join`
pub async fn join_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    headers: axum::http::HeaderMap,
    Path(channel_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let svc = ChannelService::new(state);
    svc.join_channel(&identity, &headers, &channel_id)
        .await
        .map_err(err_to_status)?;
    Ok(Json(json!({"status": "joined"})))
}

/// `POST /api/channels/:channelId/leave`
pub async fn leave_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let svc = ChannelService::new(state.clone());
    let _channel = svc
        .leave_channel(&identity, &channel_id)
        .await
        .map_err(err_to_status)?;

    // Unsubscribe the leaver from the websocket channel so they stop
    // receiving broadcasts after the row is gone.
    state
        .connection_manager
        .unsubscribe(&channel_id, &identity.pseudonym_id)
        .await;

    Ok(Json(json!({"status": "left"})))
}

/// `POST /api/channels/:channelId/voice/join`
///
/// Returns a structured JSON error body for the two voice-specific
/// failure modes (`voice_disabled`, `voice_not_configured`) and a bare
/// status-code error body otherwise — preserving the wire shapes the
/// previous inline handler produced.
pub async fn join_voice_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    headers: axum::http::HeaderMap,
    Path(channel_id): Path<String>,
    connect_info: axum::extract::ConnectInfo<std::net::SocketAddr>,
) -> Result<Json<JoinVoiceResponse>, (StatusCode, String)> {
    // For proxied deployments, the loopback check alone is misleading —
    // a request that arrives at us over loopback may have come from a
    // remote client through a reverse proxy. Treat the presence of
    // `X-Forwarded-For` as authoritative.
    let has_forwarded_for = headers.get("x-forwarded-for").is_some();
    let is_local_client = connect_info.0.ip().is_loopback() && !has_forwarded_for;

    let svc = ChannelService::new(state);
    match svc
        .join_voice_channel(&identity, &headers, &channel_id, is_local_client)
        .await
    {
        Ok(resp) => Ok(Json(resp)),
        Err(ChannelServiceError::VoiceDisabled) => Err((
            StatusCode::FORBIDDEN,
            json!({
                "error": "voice_disabled",
                "message": "Voice is disabled by the server administrator.",
                "setup_hint": "An admin can enable voice in Server Policy settings."
            })
            .to_string(),
        )),
        Err(ChannelServiceError::VoiceNotConfigured) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "error": "voice_not_configured",
                "message": "Voice is not configured. Set up WebRTC credentials in server settings to enable voice channels.",
                "setup_hint": "Configure webrtc.url, webrtc.api_key, and webrtc.api_secret in config.toml or use ANNEX_WEBRTC_* environment variables."
            })
            .to_string(),
        )),
        Err(e) => {
            let status = e.status_code();
            Err((
                status,
                status
                    .canonical_reason()
                    .unwrap_or("request failed")
                    .to_string(),
            ))
        }
    }
}

/// `POST /api/channels/:channelId/voice/leave`
pub async fn leave_voice_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let svc = ChannelService::new(state);
    svc.leave_voice_channel(&identity, &channel_id)
        .await
        .map_err(err_to_status)?;
    Ok(Json(json!({"status": "left"})))
}

/// `GET /api/channels/:channelId/voice/status`
pub async fn voice_status_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Json<VoiceStatusResponse>, StatusCode> {
    // Return the struct rather than hand-building the JSON.
    //
    // This used to re-list the fields in a `json!` literal, so adding
    // `participant_ids` to `VoiceStatusResponse` compiled, serialized in the
    // service, and was then silently dropped here — the client never saw it.
    // Nothing failed: the client defaults a missing roster to `[]`, which is
    // indistinguishable from an empty call. A typed response cannot drift from
    // its own struct.
    let svc = ChannelService::new(state);
    let resp = svc
        .voice_status(&identity, &channel_id)
        .await
        .map_err(err_to_status)?;
    Ok(Json(resp))
}

/// `GET /api/channels/:channelId/messages/:messageId/edits`
pub async fn get_message_edits_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path((channel_id, message_id)): Path<(String, String)>,
) -> Result<Json<Vec<MessageEdit>>, StatusCode> {
    let svc = ChannelService::new(state);
    let edits = svc
        .get_message_edits(&identity, &channel_id, &message_id)
        .await
        .map_err(err_to_status)?;
    Ok(Json(edits))
}

/// `GET /api/messages/search`
pub async fn search_messages_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<Message>>, StatusCode> {
    let svc = ChannelService::new(state);
    let messages = svc
        .search_messages(&identity, params.q, params.channel_id, params.limit)
        .await
        .map_err(err_to_status)?;
    Ok(Json(messages))
}
