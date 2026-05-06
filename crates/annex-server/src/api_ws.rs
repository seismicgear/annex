//! WebSocket dispatcher: HTTP-to-WS upgrade, per-message arm logic, and
//! the small handlers that issue WebSocket session tokens.
//!
//! Wire-protocol types, HMAC token helpers, and the connection-manager
//! registry have moved to [`crate::ws`]. Everything that was public from
//! this module before the split is re-exported below so external paths
//! (`annex_server::api_ws::ConnectionManager`, `OutgoingMessage`,
//! `generate_session_token`, `SESSION_TOKEN_TTL_SECS`, …) keep working
//! unchanged for handlers, services, integration tests, and the route
//! registration in [`crate::routes`].

use crate::api_federation::relay_message;
use crate::ws::error::{send_ws_error, send_ws_error_with_id};
use crate::ws::tokens::verify_ws_token;
use crate::AppState;
use annex_channels::is_member;
use annex_identity::{get_platform_identity, PlatformIdentity};
use annex_types::RoleCode;
use axum::{
    extract::{
        ws::{Message as AxumMessage, WebSocket},
        ConnectInfo, Extension, Query, WebSocketUpgrade,
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use rusqlite::OptionalExtension;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::mpsc;

// ── Public re-exports — preserve `annex_server::api_ws::Foo` paths ──────
pub use crate::ws::connection_manager::ConnectionManager;
pub use crate::ws::protocol::{
    IncomingMessage, OutgoingMessage, WsConnectParams, WsMessagePayload,
};
pub use crate::ws::tokens::{
    derive_ws_token_secret, generate_session_token, verify_token_allow_expired,
    verify_ws_token_for_auth, SESSION_TOKEN_TTL_SECS, WS_TOKEN_TTL_SECS,
};

/// `POST /api/session/refresh` — re-issues a session token for a returning user
/// whose previous token has expired. Accepts expired-but-validly-signed tokens.
///
/// This does NOT go through `auth_middleware` (which rejects expired tokens).
/// Instead it manually verifies the HMAC signature (proving the token was issued
/// by this server) and confirms the identity is still active before issuing
/// a fresh session token.
pub async fn refresh_session_handler(
    Extension(state): Extension<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<axum::Json<serde_json::Value>, StatusCode> {
    // Extract Bearer token from Authorization header
    let auth_val = headers
        .get("Authorization")
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let auth_str = auth_val.to_str().map_err(|_| StatusCode::UNAUTHORIZED)?;
    let token = auth_str
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Verify HMAC (but allow expired)
    let pseudonym = verify_token_allow_expired(token, &state.ws_token_secret)?;

    // Verify identity is still active in the database
    let server_id = state.server_id;
    let pool = state.pool.clone();
    let pseudonym_clone = pseudonym.clone();
    let identity = tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        annex_identity::get_platform_identity(&conn, server_id, &pseudonym_clone)
            .map_err(|_| StatusCode::UNAUTHORIZED)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    if !identity.active {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Issue fresh session token
    let new_token =
        generate_session_token(&pseudonym, &state.ws_token_secret, SESSION_TOKEN_TTL_SECS);
    Ok(axum::Json(serde_json::json!({
        "sessionToken": new_token,
        "expires_in_secs": SESSION_TOKEN_TTL_SECS,
    })))
}

/// `POST /api/ws/token` — issues a short-lived, HMAC-signed WebSocket session
/// token for the authenticated user. Clients should call this endpoint and
/// then connect to `/ws?token=<token>` instead of passing raw pseudonyms.
///
/// Requires authentication via `auth_middleware` (X-Annex-Pseudonym or Bearer).
pub async fn create_ws_token_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(crate::middleware::IdentityContext(identity)): Extension<
        crate::middleware::IdentityContext,
    >,
) -> Result<axum::Json<serde_json::Value>, StatusCode> {
    let token = generate_session_token(
        &identity.pseudonym_id,
        &state.ws_token_secret,
        WS_TOKEN_TTL_SECS,
    );
    Ok(axum::Json(serde_json::json!({
        "token": token,
        "expires_in_secs": WS_TOKEN_TTL_SECS,
    })))
}

/// WebSocket handler: `GET /ws?token=...` (preferred) or `GET /ws?pseudonym=...` (legacy).
///
/// When a signed `token` parameter is present, the server verifies the HMAC
/// signature and expiry, then resolves the bound pseudonym. This prevents
/// impersonation and replay attacks.
///
/// The legacy `pseudonym` parameter is still accepted for backwards compatibility
/// but should be considered deprecated. All new clients should use the token flow.
///
/// All auth attempts (success and failure) are logged with the remote address
/// for security monitoring.
pub async fn ws_handler(
    Extension(state): Extension<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
    Query(params): Query<WsConnectParams>,
) -> impl IntoResponse {
    // 1. Resolve pseudonym — prefer signed token over raw pseudonym
    let pseudonym = if let Some(ref token) = params.token {
        match verify_ws_token(token, &state.ws_token_secret) {
            Ok(p) => p,
            Err(code) => {
                tracing::warn!(
                    remote_addr = %addr,
                    status = %code,
                    "websocket token verification failed"
                );
                return code.into_response();
            }
        }
    } else if let Some(ref p) = params.pseudonym {
        // Reject raw pseudonym connections when ZK proof enforcement is enabled.
        // The raw pseudonym parameter offers no cryptographic binding and allows
        // impersonation by anyone who knows (or can compute) the pseudonym.
        if state.enforce_zk_proofs {
            tracing::warn!(
                pseudonym = %p,
                remote_addr = %addr,
                "websocket raw pseudonym rejected (enforce_zk_proofs is enabled)"
            );
            return StatusCode::UNAUTHORIZED.into_response();
        }
        // Validate pseudonym format before DB lookup — reject injection payloads
        if p.is_empty()
            || p.len() > 128
            || !p
                .as_bytes()
                .iter()
                .all(|&b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            tracing::warn!(
                remote_addr = %addr,
                "websocket raw pseudonym rejected (invalid format)"
            );
            return StatusCode::UNAUTHORIZED.into_response();
        }
        tracing::debug!(
            pseudonym = %p,
            remote_addr = %addr,
            "websocket auth via legacy pseudonym parameter (deprecated)"
        );
        p.clone()
    } else {
        tracing::warn!(remote_addr = %addr, "websocket connect missing token and pseudonym");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    // 2. Authenticate via DB
    let server_id = state.server_id;
    let pseudonym_clone = pseudonym.clone();

    let state_clone = state.clone();
    let auth_result = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        match get_platform_identity(&conn, server_id, &pseudonym_clone) {
            Ok(identity) if identity.active => Ok(identity),
            Ok(_) => Err(StatusCode::FORBIDDEN), // Inactive
            Err(_) => Err(StatusCode::UNAUTHORIZED),
        }
    })
    .await;

    match auth_result {
        Ok(Ok(identity)) => {
            tracing::info!(
                pseudonym = %pseudonym,
                remote_addr = %addr,
                token_auth = params.token.is_some(),
                "websocket auth success"
            );
            ws.on_upgrade(move |socket| handle_socket(socket, state, identity))
        }
        Ok(Err(code)) => {
            tracing::warn!(
                pseudonym = %pseudonym,
                remote_addr = %addr,
                status = %code,
                "websocket auth failed"
            );
            code.into_response()
        }
        Err(_) => {
            tracing::warn!(
                pseudonym = %pseudonym,
                remote_addr = %addr,
                "websocket auth internal error"
            );
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Result of a WebSocket membership check.
enum MembershipResult {
    /// The user is a confirmed member.
    Allowed,
    /// The user is not a member.
    Denied,
    /// An internal error occurred during the check.
    Error(String),
}

/// Checks channel membership via a blocking DB query.
///
/// Returns [`MembershipResult`] rather than silently swallowing errors.
async fn check_ws_membership(
    pool: annex_db::DbPool,
    server_id: i64,
    channel_id: &str,
    pseudonym: &str,
) -> MembershipResult {
    let cid = channel_id.to_string();
    let pid = pseudonym.to_string();
    let result = tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| format!("pool error: {e}"))?;
        is_member(&conn, server_id, &cid, &pid).map_err(|e| format!("db error: {e}"))
    })
    .await;

    match result {
        Ok(Ok(true)) => MembershipResult::Allowed,
        Ok(Ok(false)) => MembershipResult::Denied,
        Ok(Err(e)) => MembershipResult::Error(e),
        Err(e) => MembershipResult::Error(format!("task join error: {e}")),
    }
}

/// Maximum allowed length for a WebSocket message content field (64 KiB).
const MAX_WS_MESSAGE_CONTENT_LEN: usize = 65_536;

/// Maximum allowed length for a VoiceIntent text field (2 KiB).
/// TTS synthesis is CPU/memory intensive; limiting input size prevents
/// resource abuse from oversized text payloads.
const MAX_VOICE_INTENT_TEXT_LEN: usize = 2_048;

/// Minimum interval between activity updates per WebSocket connection.
/// Prevents spawning a blocking DB task on every single message.
const ACTIVITY_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(30);

/// Handles the WebSocket connection.
async fn handle_socket(socket: WebSocket, state: Arc<AppState>, identity: PlatformIdentity) {
    let pseudonym = identity.pseudonym_id.clone();

    // 1. Mark as active immediately
    tokio::spawn(touch_activity(state.clone(), pseudonym.clone()));

    let (mut sender, mut receiver) = socket.split();

    // Create a bounded channel for this session to prevent unbounded memory growth
    // from slow consumers. 256 messages provides sufficient buffer for normal
    // operation; beyond that the client is too slow and messages are dropped.
    let (tx, mut rx) = mpsc::channel::<String>(256);

    // Register session
    let session_id = state
        .connection_manager
        .add_session(pseudonym.clone(), tx.clone())
        .await;

    // Spawn a task to forward messages from rx to the websocket sender
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(AxumMessage::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });
    let mut ice_rx = state.voice_service.subscribe_ice_candidates();
    let tx_for_ice = tx.clone();
    let pseudonym_for_ice = pseudonym.clone();
    let ice_task = tokio::spawn(async move {
        while let Ok(event) = ice_rx.recv().await {
            if event.peer_id != pseudonym_for_ice {
                continue;
            }
            let outbound = OutgoingMessage::WebRtcIceCandidate {
                channel_id: event.channel_id,
                candidate: event.candidate.candidate,
                sdp_mid: event.candidate.sdp_mid,
                sdp_m_line_index: event.candidate.sdp_mline_index,
                username_fragment: event.candidate.username_fragment,
            };
            match serde_json::to_string(&outbound) {
                Ok(json) => {
                    if tx_for_ice.send(json).await.is_err() {
                        break;
                    }
                }
                Err(e) => tracing::error!("failed to serialize webrtc ice candidate: {}", e),
            }
        }
    });

    // Track last activity update to debounce DB writes
    let mut last_activity = std::time::Instant::now();

    // Handle incoming messages
    while let Some(Ok(msg)) = receiver.next().await {
        // Debounce activity updates: only spawn a DB write if enough time has passed
        if last_activity.elapsed() >= ACTIVITY_DEBOUNCE {
            tokio::spawn(touch_activity(state.clone(), pseudonym.clone()));
            last_activity = std::time::Instant::now();
        }

        if let AxumMessage::Text(text) = msg {
            if let Ok(incoming) = serde_json::from_str::<IncomingMessage>(&text.to_string()) {
                match incoming {
                    IncomingMessage::Subscribe { channel_id } => {
                        match check_ws_membership(
                            state.pool.clone(),
                            state.server_id,
                            &channel_id,
                            &pseudonym,
                        )
                        .await
                        {
                            MembershipResult::Allowed => {
                                state
                                    .connection_manager
                                    .subscribe(channel_id, pseudonym.clone())
                                    .await;
                            }
                            MembershipResult::Denied => {
                                send_ws_error(&tx, format!("Not a member of channel {channel_id}"));
                            }
                            MembershipResult::Error(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "subscribe membership check failed: {}",
                                    e
                                );
                                send_ws_error(
                                    &tx,
                                    "Internal error checking channel membership".to_string(),
                                );
                            }
                        }
                    }
                    IncomingMessage::Unsubscribe { channel_id } => {
                        state
                            .connection_manager
                            .unsubscribe(&channel_id, &pseudonym)
                            .await;
                    }
                    IncomingMessage::Message {
                        channel_id,
                        content,
                        reply_to,
                        client_request_id,
                    } => {
                        // 0. Validate content length
                        if content.trim().is_empty() {
                            send_ws_error_with_id(
                                &tx,
                                "Message content must not be empty".to_string(),
                                client_request_id,
                            );
                            continue;
                        }
                        if content.len() > MAX_WS_MESSAGE_CONTENT_LEN {
                            send_ws_error_with_id(
                                &tx,
                                format!(
                                    "Message content exceeds maximum length of {MAX_WS_MESSAGE_CONTENT_LEN} bytes"
                                ),
                                client_request_id,
                            );
                            continue;
                        }

                        // 1. Validate membership (enforcing Phase 4.4 requirements)
                        match check_ws_membership(
                            state.pool.clone(),
                            state.server_id,
                            &channel_id,
                            &pseudonym,
                        )
                        .await
                        {
                            MembershipResult::Allowed => {}
                            MembershipResult::Denied => {
                                send_ws_error_with_id(
                                    &tx,
                                    format!("Not a member of channel {channel_id}"),
                                    client_request_id,
                                );
                                continue;
                            }
                            MembershipResult::Error(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "message membership check failed: {}",
                                    e
                                );
                                send_ws_error_with_id(
                                    &tx,
                                    "Internal error checking channel membership".to_string(),
                                    client_request_id,
                                );
                                continue;
                            }
                        }

                        // Persistence + federation-flag lookup is delegated
                        // to ChannelService::send_message; broadcast and the
                        // federated-relay spawn stay here because they are
                        // websocket-protocol concerns. The membership gate
                        // above runs first, so the service's own membership
                        // check is a redundant fast read.
                        let svc = crate::services::ChannelService::new(state.clone());
                        match svc
                            .send_message(&pseudonym, &channel_id, content, reply_to)
                            .await
                        {
                            Ok((message, is_federated)) => {
                                // Broadcast via WebSocket (camelCase payload).
                                // clientRequestId is included in the broadcast for
                                // the sender's pending-send correlation. Other clients
                                // ignore unrecognized IDs (random UUIDs, no information leak).
                                let mut ws_payload: WsMessagePayload = message.clone().into();
                                ws_payload.client_request_id = client_request_id.clone();
                                let broadcast_channel_id = message.channel_id.clone();
                                let out = OutgoingMessage::Message(ws_payload);
                                match serde_json::to_string(&out) {
                                    Ok(json) => {
                                        state
                                            .connection_manager
                                            .broadcast(&broadcast_channel_id, json)
                                            .await;
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            channel_id = %broadcast_channel_id,
                                            "failed to serialize outgoing message for broadcast: {}", e
                                        );
                                    }
                                }

                                // Relay if federated
                                if is_federated {
                                    tokio::spawn(relay_message(
                                        state.clone(),
                                        message.channel_id.clone(),
                                        message,
                                    ));
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "failed to persist message: {}",
                                    e
                                );
                                send_ws_error_with_id(
                                    &tx,
                                    "Failed to send message: internal error".to_string(),
                                    client_request_id,
                                );
                            }
                        }
                    }
                    IncomingMessage::EditMessage {
                        channel_id,
                        message_id,
                        content,
                    } => {
                        if content.trim().is_empty() {
                            send_ws_error(&tx, "Message content must not be empty".to_string());
                            continue;
                        }
                        if content.len() > MAX_WS_MESSAGE_CONTENT_LEN {
                            send_ws_error(
                                &tx,
                                format!(
                                    "Message content exceeds maximum length of {MAX_WS_MESSAGE_CONTENT_LEN} bytes"
                                ),
                            );
                            continue;
                        }

                        // Membership check: same gate as Message handler
                        match check_ws_membership(
                            state.pool.clone(),
                            state.server_id,
                            &channel_id,
                            &pseudonym,
                        )
                        .await
                        {
                            MembershipResult::Allowed => {}
                            MembershipResult::Denied => {
                                send_ws_error(&tx, format!("Not a member of channel {channel_id}"));
                                continue;
                            }
                            MembershipResult::Error(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "edit membership check failed: {}",
                                    e
                                );
                                send_ws_error(
                                    &tx,
                                    "Internal error checking channel membership".to_string(),
                                );
                                continue;
                            }
                        }

                        // Persistence delegated to ChannelService::edit_message;
                        // ownership and edit-window enforcement live in
                        // annex_channels::edit_message and surface as a service
                        // error here.
                        let svc = crate::services::ChannelService::new(state.clone());
                        match svc
                            .edit_message(&pseudonym, &channel_id, &message_id, &content)
                            .await
                        {
                            Ok(updated) => {
                                // Use the persisted channel_id from DB, not the
                                // client-supplied one, to prevent cross-channel
                                // broadcast spoofing.
                                let persisted_channel_id = updated.channel_id.clone();
                                let ws_payload: WsMessagePayload = updated.into();
                                let out = OutgoingMessage::MessageEdited(ws_payload);
                                match serde_json::to_string(&out) {
                                    Ok(json) => {
                                        state
                                            .connection_manager
                                            .broadcast(&persisted_channel_id, json)
                                            .await;
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "failed to serialize edit broadcast: {}",
                                            e
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                send_ws_error(&tx, format!("Edit failed: {e}"));
                            }
                        }
                    }
                    IncomingMessage::DeleteMessage {
                        channel_id,
                        message_id,
                    } => {
                        // Membership check: same gate as Message handler
                        match check_ws_membership(
                            state.pool.clone(),
                            state.server_id,
                            &channel_id,
                            &pseudonym,
                        )
                        .await
                        {
                            MembershipResult::Allowed => {}
                            MembershipResult::Denied => {
                                send_ws_error(&tx, format!("Not a member of channel {channel_id}"));
                                continue;
                            }
                            MembershipResult::Error(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "delete membership check failed: {}",
                                    e
                                );
                                send_ws_error(
                                    &tx,
                                    "Internal error checking channel membership".to_string(),
                                );
                                continue;
                            }
                        }

                        // Persistence delegated to ChannelService::delete_message;
                        // ownership + edit-window checks live in annex_channels.
                        let svc = crate::services::ChannelService::new(state.clone());
                        match svc
                            .delete_message(&pseudonym, &channel_id, &message_id)
                            .await
                        {
                            Ok(updated) => {
                                // Use the persisted channel_id from DB, not the
                                // client-supplied one, to prevent cross-channel
                                // broadcast spoofing.
                                let persisted_channel_id = updated.channel_id.clone();
                                let ws_payload: WsMessagePayload = updated.into();
                                let out = OutgoingMessage::MessageDeleted(ws_payload);
                                match serde_json::to_string(&out) {
                                    Ok(json) => {
                                        state
                                            .connection_manager
                                            .broadcast(&persisted_channel_id, json)
                                            .await;
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "failed to serialize delete broadcast: {}",
                                            e
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                send_ws_error(&tx, format!("Delete failed: {e}"));
                            }
                        }
                    }
                    IncomingMessage::VoiceIntent { channel_id, text } => {
                        if identity.participant_type != RoleCode::AiAgent {
                            send_ws_error(&tx, "Only AI agents can use VoiceIntent".to_string());
                            continue;
                        }

                        // Validate text before expensive TTS synthesis
                        if text.trim().is_empty() {
                            send_ws_error(&tx, "VoiceIntent text must not be empty".to_string());
                            continue;
                        }
                        if text.len() > MAX_VOICE_INTENT_TEXT_LEN {
                            send_ws_error(
                                &tx,
                                format!(
                                    "VoiceIntent text exceeds maximum length of {MAX_VOICE_INTENT_TEXT_LEN} bytes"
                                ),
                            );
                            continue;
                        }

                        // Check membership
                        match check_ws_membership(
                            state.pool.clone(),
                            state.server_id,
                            &channel_id,
                            &pseudonym,
                        )
                        .await
                        {
                            MembershipResult::Allowed => {}
                            MembershipResult::Denied => {
                                send_ws_error(&tx, format!("Not a member of channel {channel_id}"));
                                continue;
                            }
                            MembershipResult::Error(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "voice intent membership check failed: {}",
                                    e
                                );
                                send_ws_error(
                                    &tx,
                                    "Internal error checking channel membership".to_string(),
                                );
                                continue;
                            }
                        }

                        // Get voice profile ID
                        let voice_profile_id = {
                            let pool = state.pool.clone();
                            let server_id = state.server_id;
                            let pid = pseudonym.clone();
                            let result = tokio::task::spawn_blocking(move || {
                                let conn = pool.get().map_err(|e| format!("pool error: {e}"))?;
                                let profile_id: Option<String> = conn
                                    .query_row(
                                        "SELECT vp.profile_id
                                     FROM agent_registrations ar
                                     JOIN voice_profiles vp ON ar.voice_profile_id = vp.id
                                     WHERE ar.server_id = ?1 AND ar.pseudonym_id = ?2",
                                        rusqlite::params![server_id, pid],
                                        |row| row.get(0),
                                    )
                                    .optional()
                                    .map_err(|e| format!("db error: {e}"))?;
                                Ok::<Option<String>, String>(profile_id)
                            })
                            .await;

                            match result {
                                Ok(Ok(Some(id))) => id,
                                Ok(Ok(None)) => "default".to_string(),
                                Ok(Err(e)) => {
                                    tracing::warn!(
                                        pseudonym = %pseudonym,
                                        "voice profile lookup failed, using default: {}",
                                        e
                                    );
                                    "default".to_string()
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        pseudonym = %pseudonym,
                                        "voice profile lookup task failed, using default: {}",
                                        e
                                    );
                                    "default".to_string()
                                }
                            }
                        };

                        // Synthesize
                        match state.tts_service.synthesize(&text, &voice_profile_id).await {
                            Ok(audio) => {
                                // Get or create voice client.
                                // Fast-path: read lock to check for existing session.
                                let client_opt = match state.voice_sessions.read() {
                                    Ok(sessions) => sessions.get(&pseudonym).cloned(),
                                    Err(_) => {
                                        tracing::error!("voice_sessions lock poisoned");
                                        continue;
                                    }
                                };

                                let client = if let Some(c) = client_opt {
                                    c
                                } else {
                                    // Connect a new voice client
                                    let room_name = channel_id.clone();
                                    let token = match state
                                        .voice_service
                                        .generate_join_token(&room_name, &pseudonym, &pseudonym)
                                    {
                                        Ok(t) => t,
                                        Err(e) => {
                                            tracing::error!(
                                                pseudonym = %pseudonym,
                                                room = %room_name,
                                                "failed to generate voice join token: {}",
                                                e
                                            );
                                            send_ws_error(
                                                &tx,
                                                "Failed to generate voice token".to_string(),
                                            );
                                            continue;
                                        }
                                    };
                                    let url = state.voice_service.get_url();

                                    match annex_voice::AgentVoiceClient::connect(
                                        url,
                                        &token,
                                        &room_name,
                                        state.stt_service.clone(),
                                        state.voice_service.api_key(),
                                        state.voice_service.api_secret(),
                                        state.voice_service.clone(),
                                    )
                                    .await
                                    {
                                        Ok(c) => {
                                            let arc = Arc::new(c);

                                            // Double-check under write lock to prevent
                                            // TOCTOU race with concurrent voice intents.
                                            match state.voice_sessions.write() {
                                                Ok(mut sessions) => {
                                                    use std::collections::hash_map::Entry;
                                                    match sessions.entry(pseudonym.clone()) {
                                                        Entry::Vacant(entry) => {
                                                            // Subscribe to transcriptions only for the winning insert
                                                            let mut rx =
                                                                arc.subscribe_transcriptions();
                                                            let cm =
                                                                state.connection_manager.clone();
                                                            let p_clone = pseudonym.clone();

                                                            tokio::spawn(async move {
                                                                while let Ok(event) =
                                                                    rx.recv().await
                                                                {
                                                                    let msg = OutgoingMessage::Transcription {
                                                                        channel_id: event.channel_id,
                                                                        speaker_pseudonym: event.speaker_pseudonym,
                                                                        text: event.text,
                                                                    };

                                                                    match serde_json::to_string(
                                                                        &msg,
                                                                    ) {
                                                                        Ok(json) => {
                                                                            cm.send(&p_clone, json)
                                                                                .await;
                                                                        }
                                                                        Err(e) => {
                                                                            tracing::error!(
                                                                                "failed to serialize transcription message: {}", e
                                                                            );
                                                                        }
                                                                    }
                                                                }
                                                            });

                                                            entry.insert(arc.clone());
                                                        }
                                                        Entry::Occupied(_) => {
                                                            // Concurrent request won; drop our client
                                                        }
                                                    }
                                                    match sessions.get(&pseudonym).cloned() {
                                                        Some(s) => s,
                                                        None => {
                                                            // Should never happen: we either just inserted or the Occupied branch
                                                            // guarantees presence. If it does, log and skip the voice operation.
                                                            tracing::error!(
                                                                pseudonym = %pseudonym,
                                                                "voice session missing after insert; this is a bug"
                                                            );
                                                            continue;
                                                        }
                                                    }
                                                }
                                                Err(_) => {
                                                    tracing::error!("voice_sessions lock poisoned");
                                                    continue;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            send_ws_error(
                                                &tx,
                                                format!("Failed to connect voice: {e}"),
                                            );
                                            continue;
                                        }
                                    }
                                };

                                if let Err(e) = client.publish_audio(&audio).await {
                                    send_ws_error(&tx, format!("Failed to publish audio: {e}"));
                                }
                            }
                            Err(e) => {
                                send_ws_error(&tx, format!("TTS failed: {e}"));
                            }
                        }
                    }
                    IncomingMessage::WebRtcOffer { channel_id, sdp } => {
                        match check_ws_membership(
                            state.pool.clone(),
                            state.server_id,
                            &channel_id,
                            &pseudonym,
                        )
                        .await
                        {
                            MembershipResult::Allowed => {
                                match state
                                    .voice_service
                                    .clone()
                                    .handle_sdp_offer(&channel_id, &pseudonym, &sdp)
                                    .await
                                {
                                    Ok(answer) => {
                                        let out = OutgoingMessage::WebRtcAnswer {
                                            channel_id,
                                            sdp: answer.sdp,
                                        };
                                        match serde_json::to_string(&out) {
                                            Ok(json) => {
                                                let _ = tx.send(json).await;
                                            }
                                            Err(e) => {
                                                tracing::error!(
                                                    "failed to serialize webrtc answer: {}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => send_ws_error(
                                        &tx,
                                        format!("WebRTC offer handling failed: {e}"),
                                    ),
                                }
                            }
                            MembershipResult::Denied => {
                                send_ws_error(&tx, format!("Not a member of channel {channel_id}"));
                            }
                            MembershipResult::Error(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "webrtc offer membership check failed: {}",
                                    e
                                );
                                send_ws_error(
                                    &tx,
                                    "Internal error checking channel membership".to_string(),
                                );
                            }
                        }
                    }
                    IncomingMessage::WebRtcIceCandidate {
                        channel_id,
                        candidate,
                        sdp_mid,
                        sdp_m_line_index,
                        username_fragment,
                    } => {
                        let candidate = annex_voice::RTCIceCandidateInit {
                            candidate,
                            sdp_mid,
                            sdp_mline_index: sdp_m_line_index,
                            username_fragment,
                        };

                        if let Err(e) = state
                            .voice_service
                            .add_ice_candidate(&channel_id, &pseudonym, candidate)
                            .await
                        {
                            send_ws_error(&tx, format!("Failed to add ICE candidate: {e}"));
                        }
                    }
                    IncomingMessage::Typing { channel_id } => {
                        // Verify membership before broadcasting typing indicator
                        match check_ws_membership(
                            state.pool.clone(),
                            state.server_id,
                            &channel_id,
                            &pseudonym,
                        )
                        .await
                        {
                            MembershipResult::Allowed => {
                                let out = OutgoingMessage::Typing {
                                    channel_id: channel_id.clone(),
                                    pseudonym_id: pseudonym.clone(),
                                };
                                if let Ok(json) = serde_json::to_string(&out) {
                                    state.connection_manager.broadcast(&channel_id, json).await;
                                }
                            }
                            _ => {
                                // Silently ignore typing from non-members
                            }
                        }
                    }
                    IncomingMessage::Resume {
                        channel_id,
                        last_message_id,
                    } => {
                        // Replay missed messages since the given message_id.
                        let state_clone = state.clone();
                        let pseudonym_clone = pseudonym.clone();
                        let tx_clone = tx.clone();
                        let channel_id_for_ack = channel_id.clone();

                        let res = tokio::task::spawn_blocking(move || {
                            let conn = state_clone.pool.get().map_err(|e| e.to_string())?;
                            // Verify membership
                            let is_mem = annex_channels::is_member(
                                &conn, state_clone.server_id, &channel_id, &pseudonym_clone,
                            ).map_err(|e| e.to_string())?;
                            if !is_mem {
                                return Ok::<Vec<annex_channels::Message>, String>(vec![]);
                            }
                            // Fetch messages created after the given message_id.
                            // First resolve the message's created_at timestamp.
                            let cursor: Option<(String, i64)> = conn
                                .query_row(
                                    "SELECT created_at, id FROM messages WHERE message_id = ?1",
                                    [&last_message_id],
                                    |row| Ok((row.get(0)?, row.get(1)?)),
                                )
                                .optional()
                                .map_err(|e| e.to_string())?;
                            let Some((ts, row_id)) = cursor else {
                                return Ok(vec![]);
                            };
                            // Get messages after the cursor, up to 200
                            let mut stmt = conn.prepare(
                                "SELECT id, server_id, channel_id, message_id, sender_pseudonym, content,
                                        reply_to_message_id, created_at, expires_at, edited_at, deleted_at
                                 FROM messages
                                 WHERE server_id = ?1 AND channel_id = ?2
                                   AND (created_at > ?3 OR (created_at = ?3 AND id > ?4))
                                 ORDER BY created_at ASC, id ASC
                                 LIMIT 200"
                            ).map_err(|e| e.to_string())?;
                            let rows = stmt.query_map(
                                rusqlite::params![state_clone.server_id, channel_id, ts, row_id],
                                |row| Ok(annex_channels::Message {
                                    id: row.get(0)?,
                                    server_id: row.get(1)?,
                                    channel_id: row.get(2)?,
                                    message_id: row.get(3)?,
                                    sender_pseudonym: row.get(4)?,
                                    content: row.get(5)?,
                                    reply_to_message_id: row.get(6)?,
                                    created_at: row.get(7)?,
                                    expires_at: row.get(8)?,
                                    edited_at: row.get(9)?,
                                    deleted_at: row.get(10)?,
                                }),
                            ).map_err(|e| e.to_string())?;
                            let mut messages = Vec::new();
                            for row in rows {
                                messages.push(row.map_err(|e| e.to_string())?);
                            }
                            Ok(messages)
                        }).await;

                        match res {
                            Ok(Ok(messages)) => {
                                let count = messages.len();
                                // Send each missed message as a normal message frame
                                for msg in messages {
                                    let ws_payload: WsMessagePayload = msg.into();
                                    let out = OutgoingMessage::Message(ws_payload);
                                    if let Ok(json) = serde_json::to_string(&out) {
                                        if tx_clone.try_send(json).is_err() {
                                            break; // Client too slow, stop replaying
                                        }
                                    }
                                }
                                // Send resume acknowledgement
                                let ack = OutgoingMessage::Resumed {
                                    channel_id: channel_id_for_ack,
                                    missed_count: count,
                                };
                                if let Ok(json) = serde_json::to_string(&ack) {
                                    let _ = tx_clone.try_send(json);
                                }
                            }
                            Ok(Err(e)) => {
                                tracing::error!(pseudonym = %pseudonym, "resume failed: {}", e);
                                send_ws_error(&tx, format!("Resume failed: {e}"));
                            }
                            Err(e) => {
                                tracing::error!(pseudonym = %pseudonym, "resume task failed: {}", e);
                                send_ws_error(&tx, "Resume failed: internal error".to_string());
                            }
                        }
                    }
                }
            } else {
                tracing::warn!(pseudonym = %pseudonym, "failed to parse incoming WebSocket message");
                send_ws_error(&tx, "invalid message format".to_string());
            }
        } else if let AxumMessage::Close(_) = msg {
            break;
        }
    }

    // Cleanup with session_id check
    state
        .connection_manager
        .remove_session(&pseudonym, session_id)
        .await;
    send_task.abort();
    ice_task.abort();

    // Clean up voice session for this pseudonym. Dropping the Arc will
    // decrement the reference count; when it reaches zero the
    // AgentVoiceClient is dropped, its internal broadcast sender closes,
    // and the spawned transcription task will exit naturally.
    match state.voice_sessions.write() {
        Ok(mut sessions) => {
            sessions.remove(&pseudonym);
        }
        Err(e) => {
            tracing::error!(
                pseudonym = %pseudonym,
                "voice_sessions RwLock poisoned during cleanup: {}", e
            );
        }
    }
}

async fn touch_activity(state: Arc<AppState>, pseudonym: String) {
    let pool = state.pool.clone();
    let server_id = state.server_id;
    let tx = state.presence_tx.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| {
            tracing::warn!("touch_activity: failed to get db connection: {}", e);
        })?;
        match annex_graph::update_node_activity(&conn, server_id, &pseudonym) {
            Ok(true) => {
                let _ = tx.send(annex_types::PresenceEvent::NodeUpdated {
                    pseudonym_id: pseudonym,
                    active: true,
                });
            }
            Ok(false) => { /* Node was already active, no broadcast needed */ }
            Err(e) => {
                tracing::warn!(
                    pseudonym = %pseudonym,
                    "touch_activity: failed to update node activity: {}",
                    e
                );
            }
        }
        Ok::<(), ()>(())
    })
    .await;

    if let Err(e) = result {
        tracing::error!(
            "touch_activity: blocking task panicked or was cancelled: {}",
            e
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use annex_channels::Message;

    #[test]
    fn ws_message_payload_serializes_camel_case() {
        let payload = WsMessagePayload {
            channel_id: "ch-1".to_string(),
            message_id: "msg-1".to_string(),
            sender_pseudonym: "alice".to_string(),
            content: "hello".to_string(),
            reply_to_message_id: Some("msg-0".to_string()),
            created_at: "2025-01-01T00:00:00Z".to_string(),
            edited_at: None,
            deleted_at: None,
            client_request_id: None,
        };

        let json = serde_json::to_value(&payload).expect("serialization should not fail");
        assert!(
            json.get("channelId").is_some(),
            "expected camelCase channelId"
        );
        assert!(
            json.get("messageId").is_some(),
            "expected camelCase messageId"
        );
        assert!(
            json.get("senderPseudonym").is_some(),
            "expected camelCase senderPseudonym"
        );
        assert!(
            json.get("replyToMessageId").is_some(),
            "expected camelCase replyToMessageId"
        );
        assert!(
            json.get("createdAt").is_some(),
            "expected camelCase createdAt"
        );

        assert!(
            json.get("channel_id").is_none(),
            "snake_case channel_id should not be present"
        );
        assert!(
            json.get("message_id").is_none(),
            "snake_case message_id should not be present"
        );

        // Verify clientRequestId is omitted when None
        assert!(
            json.get("clientRequestId").is_none(),
            "clientRequestId should be omitted when None"
        );

        // Verify clientRequestId is present when Some
        let payload_with_id = WsMessagePayload {
            client_request_id: Some("req-123".to_string()),
            ..payload
        };
        let json_with_id =
            serde_json::to_value(&payload_with_id).expect("serialization should not fail");
        assert_eq!(
            json_with_id.get("clientRequestId").and_then(|v| v.as_str()),
            Some("req-123"),
            "clientRequestId should be echoed when present"
        );
    }

    #[test]
    fn ws_message_payload_from_message() {
        let msg = Message {
            id: 0,
            server_id: 0,
            channel_id: "ch-2".to_string(),
            message_id: "msg-2".to_string(),
            sender_pseudonym: "bob".to_string(),
            content: "world".to_string(),
            reply_to_message_id: None,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: None,
            edited_at: None,
            deleted_at: None,
        };

        let payload: WsMessagePayload = msg.into();
        assert_eq!(payload.channel_id, "ch-2");
        assert_eq!(payload.message_id, "msg-2");
        assert_eq!(payload.sender_pseudonym, "bob");
        assert_eq!(payload.content, "world");
        assert!(payload.reply_to_message_id.is_none());
    }

    #[test]
    fn outgoing_message_wraps_with_type_tag() {
        let payload = WsMessagePayload {
            channel_id: "ch-1".to_string(),
            message_id: "msg-1".to_string(),
            sender_pseudonym: "alice".to_string(),
            content: "test".to_string(),
            reply_to_message_id: None,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            edited_at: None,
            deleted_at: None,
            client_request_id: None,
        };

        let out = OutgoingMessage::Message(payload);
        let json = serde_json::to_value(&out).expect("serialization should not fail");
        assert_eq!(json.get("type").and_then(|v| v.as_str()), Some("message"));
        assert!(json.get("channelId").is_some());
    }
}
