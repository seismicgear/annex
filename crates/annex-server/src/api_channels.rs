use crate::api_federation::find_commitment_for_pseudonym;
use crate::middleware::{verify_zk_membership_header, IdentityContext};
use crate::AppState;
use annex_channels::{
    add_member, create_channel, delete_channel, get_channel, get_edit_history, is_member,
    list_channels, list_messages, remove_member, Channel, CreateChannelParams, Message,
    MessageEdit,
};
use annex_graph::{create_edge, delete_edge};
use annex_types::{AlignmentStatus, ChannelType, EdgeKind, FederationScope, RoleCode};
use axum::{
    extract::{Extension, Path, Query},
    http::StatusCode,
    response::Json,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

/// Maximum length for a channel ID.
const MAX_CHANNEL_ID_LEN: usize = 128;
/// Maximum length for a channel name.
const MAX_CHANNEL_NAME_LEN: usize = 256;
/// Maximum length for a channel topic.
const MAX_TOPIC_LEN: usize = 1024;

/// Look up the identity commitment for a pseudonym (needed for ZK proof binding).
///
/// Returns the commitment hex string, or `None` if the pseudonym has no
/// registered commitment (e.g. pre-ZK legacy identities).
async fn lookup_commitment(
    pool: &annex_db::DbPool,
    pseudonym_id: &str,
) -> Result<Option<String>, StatusCode> {
    let pool = pool.clone();
    let pseudo = pseudonym_id.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        find_commitment_for_pseudonym(&conn, &pseudo)
            .map(|opt| opt.map(|(commitment, _topic)| commitment))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
}

/// Maps a [`ChannelError`] to the correct HTTP status code, logging non-404 errors.
///
/// `NotFound` → 404, everything else → 500 (with error logged).
fn channel_err_to_status(e: annex_channels::ChannelError) -> StatusCode {
    match e {
        annex_channels::ChannelError::NotFound(_) => StatusCode::NOT_FOUND,
        ref err => {
            tracing::error!(error = %err, "channel operation failed");
            drop(e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

#[derive(Deserialize)]
pub struct HistoryParams {
    pub before: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Deserialize)]
pub struct CreateChannelRequest {
    pub channel_id: String,
    pub name: String,
    pub channel_type: ChannelType,
    pub topic: Option<String>,
    pub vrp_topic_binding: Option<String>,
    pub required_capabilities_json: Option<String>,
    pub agent_min_alignment: Option<AlignmentStatus>,
    pub retention_days: Option<u32>,
    pub federation_scope: FederationScope,
}

#[derive(Serialize)]
pub struct IceServerResponse {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub credential: String,
}

#[derive(Serialize)]
pub struct JoinVoiceResponse {
    pub token: String,
    pub url: String,
    pub ice_servers: Vec<IceServerResponse>,
}

/// POST /api/channels
pub async fn create_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Json(payload): Json<CreateChannelRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !identity.can_moderate {
        return Err(StatusCode::FORBIDDEN);
    }

    // Validate string lengths to prevent oversized payloads
    if payload.channel_id.len() > MAX_CHANNEL_ID_LEN || payload.channel_id.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    if payload.name.len() > MAX_CHANNEL_NAME_LEN || payload.name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    if let Some(ref t) = payload.topic {
        if t.len() > MAX_TOPIC_LEN {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let params = CreateChannelParams {
        server_id: state.server_id,
        channel_id: payload.channel_id.clone(),
        name: payload.name,
        channel_type: payload.channel_type,
        topic: payload.topic,
        vrp_topic_binding: payload.vrp_topic_binding,
        required_capabilities_json: payload.required_capabilities_json,
        agent_min_alignment: payload.agent_min_alignment,
        retention_days: payload.retention_days,
        federation_scope: payload.federation_scope,
    };

    let pool = state.pool.clone();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        create_channel(&conn, &params).map_err(|e| {
            // Handle unique constraint violation -> 409 Conflict
            if let annex_channels::ChannelError::Database(rusqlite::Error::SqliteFailure(
                error_code,
                _,
            )) = e
            {
                if error_code.code == rusqlite::ffi::ErrorCode::ConstraintViolation {
                    return StatusCode::CONFLICT;
                }
            }
            StatusCode::INTERNAL_SERVER_ERROR
        })
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    // Create LiveKit room if needed and enabled
    if (payload.channel_type == ChannelType::Voice || payload.channel_type == ChannelType::Hybrid)
        && state.voice_service.is_enabled()
    {
        if let Err(e) = state.voice_service.create_room(&payload.channel_id).await {
            tracing::error!(
                "failed to create LiveKit room for channel {}: {}",
                payload.channel_id,
                e
            );
            // We log but don't fail the request since the DB record was created successfully.
        }
    }

    Ok(Json(json!({"status": "created"})))
}

/// GET /api/channels
pub async fn list_channels_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(_identity)): Extension<IdentityContext>,
) -> Result<Json<Vec<Channel>>, StatusCode> {
    let channels = tokio::task::spawn_blocking(move || {
        let conn = state.pool.get().map_err(|e| {
            tracing::error!(error = %e, "failed to get db connection for list_channels");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        list_channels(&conn, state.server_id).map_err(channel_err_to_status)
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "list_channels task join error");
        StatusCode::INTERNAL_SERVER_ERROR
    })??;

    Ok(Json(channels))
}

/// GET /api/channels/:channelId
///
/// Returns channel details. Requires the requester to be a member of the
/// channel or to have moderation privileges, preventing metadata leakage
/// for private channels.
pub async fn get_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Json<Channel>, StatusCode> {
    let pool = state.pool.clone();
    let server_id = state.server_id;
    let cid = channel_id.clone();
    let pid = identity.pseudonym_id.clone();
    let can_moderate = identity.can_moderate;

    let channel = tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| {
            tracing::error!(error = %e, "failed to get db connection for get_channel");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        // Moderators can view any channel; regular users must be members
        if !can_moderate {
            let member = is_member(&conn, server_id, &cid, &pid)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            if !member {
                return Err(StatusCode::FORBIDDEN);
            }
        }

        get_channel(&conn, &cid).map_err(channel_err_to_status)
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "get_channel task join error");
        StatusCode::INTERNAL_SERVER_ERROR
    })??;

    Ok(Json(channel))
}

/// DELETE /api/channels/:channelId
pub async fn delete_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !identity.can_moderate {
        return Err(StatusCode::FORBIDDEN);
    }

    tokio::task::spawn_blocking(move || {
        let conn = state.pool.get().map_err(|e| {
            tracing::error!(error = %e, "failed to get db connection for delete_channel");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        delete_channel(&conn, &channel_id).map_err(channel_err_to_status)
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "delete_channel task join error");
        StatusCode::INTERNAL_SERVER_ERROR
    })??;

    Ok(Json(json!({"status": "deleted"})))
}

/// GET /api/channels/:channelId/messages
pub async fn get_channel_history_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    headers: axum::http::HeaderMap,
    Path(channel_id): Path<String>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<Vec<Message>>, StatusCode> {
    // 0. ZK proof enforcement — bind proof to authenticated identity
    let commitment = lookup_commitment(&state.pool, &identity.pseudonym_id).await?;
    verify_zk_membership_header(&state, &headers, commitment.as_deref())?;

    // 1. Verify Membership
    let is_member = tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let server_id = state.server_id;
        let cid = channel_id.clone();
        let pid = identity.pseudonym_id.clone();
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            is_member(&conn, server_id, &cid, &pid).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    if !is_member {
        return Err(StatusCode::FORBIDDEN);
    }

    // 2. Fetch Messages (cap limit to 200 to prevent oversize responses)
    let messages = tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let server_id = state.server_id;
        let cid = channel_id.clone();
        let before = params.before;
        let limit = params.limit.map(|l| l.min(200));
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            list_messages(&conn, server_id, &cid, before, limit).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    Ok(Json(messages))
}

/// POST /api/channels/:channelId/join
pub async fn join_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    headers: axum::http::HeaderMap,
    Path(channel_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 0. ZK proof enforcement — bind proof to authenticated identity
    let commitment = lookup_commitment(&state.pool, &identity.pseudonym_id).await?;
    verify_zk_membership_header(&state, &headers, commitment.as_deref())?;

    // 1. Fetch Channel
    let channel = {
        let pool = state.pool.clone();
        let cid = channel_id.clone();
        let channel_res = tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            get_channel(&conn, &cid).map_err(channel_err_to_status)
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        match channel_res {
            Ok(c) => c,
            Err(code) => return Err(code),
        }
    };

    // 2. Check Capabilities
    if let Some(caps_json) = &channel.required_capabilities_json {
        let required: Vec<String> =
            serde_json::from_str(caps_json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        for req in required {
            let has_cap = match req.as_str() {
                "can_voice" => identity.can_voice,
                "can_moderate" => identity.can_moderate,
                "can_invite" => identity.can_invite,
                "can_federate" => identity.can_federate,
                "can_bridge" => identity.can_bridge,
                _ => false, // Unknown capability required -> deny
            };
            if !has_cap {
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    // 3. Check Agent Alignment
    if identity.participant_type == RoleCode::AiAgent {
        // Query agent registration
        let alignment_status: Option<String> = tokio::task::spawn_blocking({
            let pool = state.pool.clone();
            let server_id = state.server_id;
            let pseudo = identity.pseudonym_id.clone();
            move || {
                let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                conn.query_row(
                    "SELECT alignment_status FROM agent_registrations WHERE server_id = ?1 AND pseudonym_id = ?2",
                    params![server_id, pseudo],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
            }
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

        let status_str = alignment_status.ok_or(StatusCode::FORBIDDEN)?; // Agent not registered

        // Parse alignment status (handle both quoted JSON string and plain text)
        let status: AlignmentStatus = serde_json::from_str(&status_str)
            .or_else(|_| serde_json::from_str(&format!("\"{}\"", status_str)))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        // Rule: Conflict agents cannot join any channel
        if status == AlignmentStatus::Conflict {
            return Err(StatusCode::FORBIDDEN);
        }

        // Rule: Partial agents are restricted to TEXT channels only
        if status == AlignmentStatus::Partial && channel.channel_type != ChannelType::Text {
            return Err(StatusCode::FORBIDDEN);
        }

        // Rule: Channel minimum alignment requirement
        if let Some(min_alignment) = channel.agent_min_alignment {
            // Logic: Aligned > Partial > Conflict
            let allowed = match min_alignment {
                AlignmentStatus::Conflict => true, // Conflict agents are already blocked above, but for completeness
                AlignmentStatus::Partial => status != AlignmentStatus::Conflict,
                AlignmentStatus::Aligned => status == AlignmentStatus::Aligned,
            };

            if !allowed {
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    // 4. Add Member
    tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let server_id = state.server_id;
        let cid = channel_id.clone();
        let pid = identity.pseudonym_id.clone();
        let is_agent = identity.participant_type == RoleCode::AiAgent;
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            add_member(&conn, server_id, &cid, &pid)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            if is_agent {
                create_edge(&conn, server_id, &pid, &cid, EdgeKind::AgentServing, 1.0)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            }
            Ok::<(), StatusCode>(())
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    // 5. Connect Agent Voice Client if applicable
    if identity.participant_type == RoleCode::AiAgent
        && (channel.channel_type == ChannelType::Voice
            || channel.channel_type == ChannelType::Hybrid)
    {
        let pseudonym = identity.pseudonym_id.clone();
        let channel_id_clone = channel_id.clone();

        // Fast-path: check with read lock to avoid expensive connect when session exists.
        let already_exists = {
            let sessions = state
                .voice_sessions
                .read()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            sessions.contains_key(&pseudonym)
        };

        if !already_exists {
            let token = state
                .voice_service
                .generate_join_token(&channel_id_clone, &pseudonym, &pseudonym)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let url = state.voice_service.get_url();

            let client = annex_voice::AgentVoiceClient::connect(
                url,
                &token,
                &channel_id_clone,
                state.stt_service.clone(),
                state.voice_service.api_key(),
                state.voice_service.api_secret(),
            )
            .await
            .map_err(|e| {
                tracing::error!("Failed to connect agent voice client: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

            let client = Arc::new(client);

            // Double-check under write lock after the async connect gap.
            // If a concurrent request already inserted a session, we drop our
            // client and skip the transcription subscription to avoid duplicates.
            let mut sessions = state
                .voice_sessions
                .write()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            if let std::collections::hash_map::Entry::Vacant(entry) = sessions.entry(pseudonym) {
                // Subscribe to transcriptions only for the winning insert
                let mut rx = client.subscribe_transcriptions();
                let cm = state.connection_manager.clone();
                let p_clone = entry.key().clone();

                tokio::spawn(async move {
                    while let Ok(event) = rx.recv().await {
                        let msg = crate::api_ws::OutgoingMessage::Transcription {
                            channel_id: event.channel_id,
                            speaker_pseudonym: event.speaker_pseudonym,
                            text: event.text,
                        };

                        match serde_json::to_string(&msg) {
                            Ok(json) => {
                                cm.send(&p_clone, json).await;
                            }
                            Err(e) => {
                                tracing::error!("failed to serialize transcription message: {}", e);
                            }
                        }
                    }
                });

                entry.insert(client);
            }
            // else: concurrent request won the race; our client is dropped
        }
    }

    Ok(Json(json!({"status": "joined"})))
}

/// POST /api/channels/:channelId/leave
pub async fn leave_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 1. Fetch Channel (to check type)
    let channel = {
        let pool = state.pool.clone();
        let cid = channel_id.clone();
        let channel_res = tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            get_channel(&conn, &cid).map_err(channel_err_to_status)
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        match channel_res {
            Ok(c) => c,
            Err(code) => return Err(code),
        }
    };

    // 2. Remove Member
    tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let server_id = state.server_id;
        let cid = channel_id.clone();
        let pid = identity.pseudonym_id.clone();
        let is_agent = identity.participant_type == RoleCode::AiAgent;
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            remove_member(&conn, server_id, &cid, &pid).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            if is_agent {
                delete_edge(&conn, server_id, &pid, &cid, EdgeKind::AgentServing)
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            }
            Ok::<(), StatusCode>(())
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    // 3. Unsubscribe from WebSocket
    state
        .connection_manager
        .unsubscribe(&channel_id, &identity.pseudonym_id)
        .await;

    // 4. Remove from Voice Channel (if applicable)
    if (channel.channel_type == ChannelType::Voice || channel.channel_type == ChannelType::Hybrid)
        && state.voice_service.is_enabled()
    {
        if let Err(e) = state
            .voice_service
            .remove_participant(&channel_id, &identity.pseudonym_id)
            .await
        {
            // We log but don't fail, as the user has successfully left the Annex channel.
            tracing::warn!(
                "failed to remove participant {} from voice room {}: {}",
                identity.pseudonym_id,
                channel_id,
                e
            );
        }
    }

    Ok(Json(json!({"status": "left"})))
}

/// POST /api/channels/:channelId/voice/join
pub async fn join_voice_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    headers: axum::http::HeaderMap,
    Path(channel_id): Path<String>,
) -> Result<Json<JoinVoiceResponse>, (StatusCode, String)> {
    // 0. ZK proof enforcement — bind proof to authenticated identity
    let commitment = lookup_commitment(&state.pool, &identity.pseudonym_id)
        .await
        .map_err(|status| {
            (
                status,
                status
                    .canonical_reason()
                    .unwrap_or("internal error")
                    .to_string(),
            )
        })?;
    verify_zk_membership_header(&state, &headers, commitment.as_deref()).map_err(|status| {
        (
            status,
            status
                .canonical_reason()
                .unwrap_or("request failed")
                .to_string(),
        )
    })?;

    if !state.voice_service.is_enabled() || state.voice_service.get_public_url().is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({
                "error": "voice_not_configured",
                "message": "Voice is not configured. Set up LiveKit credentials in server settings to enable voice channels.",
                "setup_hint": "Configure livekit.url, livekit.api_key, and livekit.api_secret in config.toml or use ANNEX_LIVEKIT_* environment variables."
            })
            .to_string(),
        ));
    }

    // 1. Check if user is a member of the channel
    let is_member = tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let server_id = state.server_id;
        let cid = channel_id.clone();
        let pid = identity.pseudonym_id.clone();
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            is_member(&conn, server_id, &cid, &pid).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to validate channel membership".to_string(),
        )
    })?
    .map_err(|status| {
        (
            status,
            status
                .canonical_reason()
                .unwrap_or("request failed")
                .to_string(),
        )
    })?;

    if !is_member {
        return Err((StatusCode::FORBIDDEN, "Not a channel member".to_string()));
    }

    // 2. Fetch channel to verify type
    let channel = tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let cid = channel_id.clone();
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            get_channel(&conn, &cid).map_err(channel_err_to_status)
        }
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to load channel".to_string(),
        )
    })?
    .map_err(|status| {
        (
            status,
            status
                .canonical_reason()
                .unwrap_or("request failed")
                .to_string(),
        )
    })?;

    if channel.channel_type != ChannelType::Voice && channel.channel_type != ChannelType::Hybrid {
        return Err((
            StatusCode::BAD_REQUEST,
            "Channel does not support voice".to_string(),
        ));
    }

    // 3. Generate Token
    // We use the pseudonym as the participant identity and name.
    let token = state
        .voice_service
        .generate_join_token(&channel_id, &identity.pseudonym_id, &identity.pseudonym_id)
        .map_err(|e| {
            tracing::error!("failed to generate LiveKit token: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to generate voice token".to_string(),
            )
        })?;

    let ice_servers: Vec<IceServerResponse> = state
        .voice_service
        .ice_servers()
        .iter()
        .map(|s| IceServerResponse {
            urls: s.urls.clone(),
            username: s.username.clone(),
            credential: s.credential.clone(),
        })
        .collect();

    Ok(Json(JoinVoiceResponse {
        token,
        url: state.voice_service.get_public_url().to_string(),
        ice_servers,
    }))
}

/// POST /api/channels/:channelId/voice/leave
pub async fn leave_voice_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 1. Check if user is a member of the channel
    let is_member = tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let server_id = state.server_id;
        let cid = channel_id.clone();
        let pid = identity.pseudonym_id.clone();
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            is_member(&conn, server_id, &cid, &pid).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    if !is_member {
        return Err(StatusCode::FORBIDDEN);
    }

    // 2. Fetch channel to verify type
    let channel = tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let cid = channel_id.clone();
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            get_channel(&conn, &cid).map_err(channel_err_to_status)
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    if channel.channel_type != ChannelType::Voice && channel.channel_type != ChannelType::Hybrid {
        return Err(StatusCode::BAD_REQUEST);
    }

    // 3. Remove Participant
    if state.voice_service.is_enabled() {
        if let Err(e) = state
            .voice_service
            .remove_participant(&channel_id, &identity.pseudonym_id)
            .await
        {
            tracing::warn!(
                "failed to remove participant {} from voice room {}: {}",
                identity.pseudonym_id,
                channel_id,
                e
            );
            // We return OK even if this fails, as it's a best-effort cleanup.
            // If the user wasn't in the room, it's fine.
        }
    }

    Ok(Json(json!({"status": "left"})))
}

/// GET /api/channels/:channelId/voice/status
/// Returns the number of participants currently in the voice room.
pub async fn voice_status_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Verify membership
    let is_member_val = tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let server_id = state.server_id;
        let cid = channel_id.clone();
        let pid = identity.pseudonym_id.clone();
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            is_member(&conn, server_id, &cid, &pid).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    if !is_member_val {
        return Err(StatusCode::FORBIDDEN);
    }

    let count = state
        .voice_service
        .participant_count(&channel_id)
        .await
        .unwrap_or(0);

    Ok(Json(json!({
        "participants": count,
        "active": count > 0,
    })))
}

/// GET /api/channels/:channelId/messages/:messageId/edits
/// Returns the edit history for a message.
pub async fn get_message_edits_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path((channel_id, message_id)): Path<(String, String)>,
) -> Result<Json<Vec<MessageEdit>>, StatusCode> {
    // Verify membership
    let is_member_val = tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let server_id = state.server_id;
        let cid = channel_id.clone();
        let pid = identity.pseudonym_id.clone();
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            is_member(&conn, server_id, &cid, &pid).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    if !is_member_val {
        return Err(StatusCode::FORBIDDEN);
    }

    let edits = tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            get_edit_history(&conn, &message_id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    Ok(Json(edits))
}
