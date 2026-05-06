//! Per-message dispatch for the WebSocket session loop.
//!
//! [`dispatch`] is the giant `match` on [`IncomingMessage`] that used to
//! live inline inside `handle_socket`. As the per-command extractions
//! proceed each arm is replaced by a delegation to a sibling module
//! under [`crate::ws::commands`]. The `Subscribe` and `Unsubscribe`
//! variants are kept inline because they are trivial and have no
//! independent test surface.
//!
//! `MembershipResult` and [`check_ws_membership`] are exposed at the
//! module level so command handlers can run the same gate the
//! dispatcher itself does, without round-tripping through `dispatch`.
//! `MAX_WS_MESSAGE_CONTENT_LEN` lives here for the same reason — both
//! `IncomingMessage::Message` and `IncomingMessage::EditMessage` enforce
//! it.

use crate::ws::commands::typing;
use crate::ws::context::CommandContext;
use crate::ws::error::send_ws_error;
use crate::ws::error::send_ws_error_with_id;
use crate::ws::protocol::{IncomingMessage, OutgoingMessage, WsMessagePayload};
use crate::AppState;
use annex_channels::is_member;
use annex_types::RoleCode;
use rusqlite::OptionalExtension;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::api_federation::relay_message;

/// Maximum allowed length for a WebSocket message content field (64 KiB).
pub(crate) const MAX_WS_MESSAGE_CONTENT_LEN: usize = 65_536;

/// Maximum allowed length for a VoiceIntent text field (2 KiB).
/// TTS synthesis is CPU/memory intensive; limiting input size prevents
/// resource abuse from oversized text payloads.
pub(crate) const MAX_VOICE_INTENT_TEXT_LEN: usize = 2_048;

/// Result of a WebSocket membership check.
pub(crate) enum MembershipResult {
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
pub(crate) async fn check_ws_membership(
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

/// Dispatch a single decoded incoming frame.
///
/// `state` and `tx` are passed through `ctx`; this function simply
/// fans the variants out to per-command handlers (or, for the trivial
/// arms, executes them inline).
pub(crate) async fn dispatch(ctx: &CommandContext<'_>, msg: IncomingMessage) {
    match msg {
        IncomingMessage::Subscribe { channel_id } => {
            handle_subscribe(ctx, channel_id).await;
        }
        IncomingMessage::Unsubscribe { channel_id } => {
            ctx.state
                .connection_manager
                .unsubscribe(&channel_id, ctx.pseudonym)
                .await;
        }
        IncomingMessage::Message {
            channel_id,
            content,
            reply_to,
            client_request_id,
        } => {
            handle_message(ctx, channel_id, content, reply_to, client_request_id).await;
        }
        IncomingMessage::EditMessage {
            channel_id,
            message_id,
            content,
        } => {
            handle_edit(ctx, channel_id, message_id, content).await;
        }
        IncomingMessage::DeleteMessage {
            channel_id,
            message_id,
        } => {
            handle_delete(ctx, channel_id, message_id).await;
        }
        IncomingMessage::VoiceIntent { channel_id, text } => {
            handle_voice_intent(ctx, channel_id, text).await;
        }
        IncomingMessage::WebRtcOffer { channel_id, sdp } => {
            handle_webrtc_offer(ctx, channel_id, sdp).await;
        }
        IncomingMessage::WebRtcIceCandidate {
            channel_id,
            candidate,
            sdp_mid,
            sdp_m_line_index,
            username_fragment,
        } => {
            handle_webrtc_ice(
                ctx,
                channel_id,
                candidate,
                sdp_mid,
                sdp_m_line_index,
                username_fragment,
            )
            .await;
        }
        IncomingMessage::Typing { channel_id } => {
            typing::handle(ctx, channel_id).await;
        }
        IncomingMessage::Resume {
            channel_id,
            last_message_id,
        } => {
            handle_resume(ctx, channel_id, last_message_id).await;
        }
    }
}

async fn handle_subscribe(ctx: &CommandContext<'_>, channel_id: String) {
    match check_ws_membership(
        ctx.state.pool.clone(),
        ctx.state.server_id,
        &channel_id,
        ctx.pseudonym,
    )
    .await
    {
        MembershipResult::Allowed => {
            ctx.state
                .connection_manager
                .subscribe(channel_id, ctx.pseudonym.to_string())
                .await;
        }
        MembershipResult::Denied => {
            send_ws_error(ctx.tx, format!("Not a member of channel {channel_id}"));
        }
        MembershipResult::Error(e) => {
            tracing::error!(
                pseudonym = %ctx.pseudonym,
                channel_id = %channel_id,
                "subscribe membership check failed: {}",
                e
            );
            send_ws_error(
                ctx.tx,
                "Internal error checking channel membership".to_string(),
            );
        }
    }
}

async fn handle_message(
    ctx: &CommandContext<'_>,
    channel_id: String,
    content: String,
    reply_to: Option<String>,
    client_request_id: Option<String>,
) {
    if content.trim().is_empty() {
        send_ws_error_with_id(
            ctx.tx,
            "Message content must not be empty".to_string(),
            client_request_id,
        );
        return;
    }
    if content.len() > MAX_WS_MESSAGE_CONTENT_LEN {
        send_ws_error_with_id(
            ctx.tx,
            format!("Message content exceeds maximum length of {MAX_WS_MESSAGE_CONTENT_LEN} bytes"),
            client_request_id,
        );
        return;
    }

    match check_ws_membership(
        ctx.state.pool.clone(),
        ctx.state.server_id,
        &channel_id,
        ctx.pseudonym,
    )
    .await
    {
        MembershipResult::Allowed => {}
        MembershipResult::Denied => {
            send_ws_error_with_id(
                ctx.tx,
                format!("Not a member of channel {channel_id}"),
                client_request_id,
            );
            return;
        }
        MembershipResult::Error(e) => {
            tracing::error!(
                pseudonym = %ctx.pseudonym,
                channel_id = %channel_id,
                "message membership check failed: {}",
                e
            );
            send_ws_error_with_id(
                ctx.tx,
                "Internal error checking channel membership".to_string(),
                client_request_id,
            );
            return;
        }
    }

    // Persistence + federation-flag lookup is delegated to
    // ChannelService::send_message; broadcast and the federated-relay
    // spawn stay here because they are websocket-protocol concerns. The
    // membership gate above runs first, so the service's own membership
    // check is a redundant fast read.
    let svc = crate::services::ChannelService::new(ctx.state.clone());
    match svc
        .send_message(ctx.pseudonym, &channel_id, content, reply_to)
        .await
    {
        Ok((message, is_federated)) => {
            let mut ws_payload: WsMessagePayload = message.clone().into();
            ws_payload.client_request_id = client_request_id.clone();
            let broadcast_channel_id = message.channel_id.clone();
            let out = OutgoingMessage::Message(ws_payload);
            match serde_json::to_string(&out) {
                Ok(json) => {
                    ctx.state
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

            if is_federated {
                tokio::spawn(relay_message(
                    ctx.state.clone(),
                    message.channel_id.clone(),
                    message,
                ));
            }
        }
        Err(e) => {
            tracing::error!(
                pseudonym = %ctx.pseudonym,
                channel_id = %channel_id,
                "failed to persist message: {}",
                e
            );
            send_ws_error_with_id(
                ctx.tx,
                "Failed to send message: internal error".to_string(),
                client_request_id,
            );
        }
    }
}

async fn handle_edit(
    ctx: &CommandContext<'_>,
    channel_id: String,
    message_id: String,
    content: String,
) {
    if content.trim().is_empty() {
        send_ws_error(ctx.tx, "Message content must not be empty".to_string());
        return;
    }
    if content.len() > MAX_WS_MESSAGE_CONTENT_LEN {
        send_ws_error(
            ctx.tx,
            format!("Message content exceeds maximum length of {MAX_WS_MESSAGE_CONTENT_LEN} bytes"),
        );
        return;
    }

    match check_ws_membership(
        ctx.state.pool.clone(),
        ctx.state.server_id,
        &channel_id,
        ctx.pseudonym,
    )
    .await
    {
        MembershipResult::Allowed => {}
        MembershipResult::Denied => {
            send_ws_error(ctx.tx, format!("Not a member of channel {channel_id}"));
            return;
        }
        MembershipResult::Error(e) => {
            tracing::error!(
                pseudonym = %ctx.pseudonym,
                channel_id = %channel_id,
                "edit membership check failed: {}",
                e
            );
            send_ws_error(
                ctx.tx,
                "Internal error checking channel membership".to_string(),
            );
            return;
        }
    }

    let svc = crate::services::ChannelService::new(ctx.state.clone());
    match svc
        .edit_message(ctx.pseudonym, &channel_id, &message_id, &content)
        .await
    {
        Ok(updated) => {
            let persisted_channel_id = updated.channel_id.clone();
            let ws_payload: WsMessagePayload = updated.into();
            let out = OutgoingMessage::MessageEdited(ws_payload);
            match serde_json::to_string(&out) {
                Ok(json) => {
                    ctx.state
                        .connection_manager
                        .broadcast(&persisted_channel_id, json)
                        .await;
                }
                Err(e) => {
                    tracing::error!("failed to serialize edit broadcast: {}", e);
                }
            }
        }
        Err(e) => {
            send_ws_error(ctx.tx, format!("Edit failed: {e}"));
        }
    }
}

async fn handle_delete(ctx: &CommandContext<'_>, channel_id: String, message_id: String) {
    match check_ws_membership(
        ctx.state.pool.clone(),
        ctx.state.server_id,
        &channel_id,
        ctx.pseudonym,
    )
    .await
    {
        MembershipResult::Allowed => {}
        MembershipResult::Denied => {
            send_ws_error(ctx.tx, format!("Not a member of channel {channel_id}"));
            return;
        }
        MembershipResult::Error(e) => {
            tracing::error!(
                pseudonym = %ctx.pseudonym,
                channel_id = %channel_id,
                "delete membership check failed: {}",
                e
            );
            send_ws_error(
                ctx.tx,
                "Internal error checking channel membership".to_string(),
            );
            return;
        }
    }

    let svc = crate::services::ChannelService::new(ctx.state.clone());
    match svc
        .delete_message(ctx.pseudonym, &channel_id, &message_id)
        .await
    {
        Ok(updated) => {
            let persisted_channel_id = updated.channel_id.clone();
            let ws_payload: WsMessagePayload = updated.into();
            let out = OutgoingMessage::MessageDeleted(ws_payload);
            match serde_json::to_string(&out) {
                Ok(json) => {
                    ctx.state
                        .connection_manager
                        .broadcast(&persisted_channel_id, json)
                        .await;
                }
                Err(e) => {
                    tracing::error!("failed to serialize delete broadcast: {}", e);
                }
            }
        }
        Err(e) => {
            send_ws_error(ctx.tx, format!("Delete failed: {e}"));
        }
    }
}

async fn handle_voice_intent(ctx: &CommandContext<'_>, channel_id: String, text: String) {
    if ctx.identity.participant_type != RoleCode::AiAgent {
        send_ws_error(ctx.tx, "Only AI agents can use VoiceIntent".to_string());
        return;
    }

    if text.trim().is_empty() {
        send_ws_error(ctx.tx, "VoiceIntent text must not be empty".to_string());
        return;
    }
    if text.len() > MAX_VOICE_INTENT_TEXT_LEN {
        send_ws_error(
            ctx.tx,
            format!("VoiceIntent text exceeds maximum length of {MAX_VOICE_INTENT_TEXT_LEN} bytes"),
        );
        return;
    }

    match check_ws_membership(
        ctx.state.pool.clone(),
        ctx.state.server_id,
        &channel_id,
        ctx.pseudonym,
    )
    .await
    {
        MembershipResult::Allowed => {}
        MembershipResult::Denied => {
            send_ws_error(ctx.tx, format!("Not a member of channel {channel_id}"));
            return;
        }
        MembershipResult::Error(e) => {
            tracing::error!(
                pseudonym = %ctx.pseudonym,
                channel_id = %channel_id,
                "voice intent membership check failed: {}",
                e
            );
            send_ws_error(
                ctx.tx,
                "Internal error checking channel membership".to_string(),
            );
            return;
        }
    }

    let voice_profile_id = {
        let pool = ctx.state.pool.clone();
        let server_id = ctx.state.server_id;
        let pid = ctx.pseudonym.to_string();
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
                    pseudonym = %ctx.pseudonym,
                    "voice profile lookup failed, using default: {}",
                    e
                );
                "default".to_string()
            }
            Err(e) => {
                tracing::warn!(
                    pseudonym = %ctx.pseudonym,
                    "voice profile lookup task failed, using default: {}",
                    e
                );
                "default".to_string()
            }
        }
    };

    match ctx
        .state
        .tts_service
        .synthesize(&text, &voice_profile_id)
        .await
    {
        Ok(audio) => {
            let client_opt = match ctx.state.voice_sessions.read() {
                Ok(sessions) => sessions.get(ctx.pseudonym).cloned(),
                Err(_) => {
                    tracing::error!("voice_sessions lock poisoned");
                    return;
                }
            };

            let client = if let Some(c) = client_opt {
                c
            } else {
                let room_name = channel_id.clone();
                let token = match ctx.state.voice_service.generate_join_token(
                    &room_name,
                    ctx.pseudonym,
                    ctx.pseudonym,
                ) {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::error!(
                            pseudonym = %ctx.pseudonym,
                            room = %room_name,
                            "failed to generate voice join token: {}",
                            e
                        );
                        send_ws_error(ctx.tx, "Failed to generate voice token".to_string());
                        return;
                    }
                };
                let url = ctx.state.voice_service.get_url();

                match annex_voice::AgentVoiceClient::connect(
                    url,
                    &token,
                    &room_name,
                    ctx.state.stt_service.clone(),
                    ctx.state.voice_service.api_key(),
                    ctx.state.voice_service.api_secret(),
                    ctx.state.voice_service.clone(),
                )
                .await
                {
                    Ok(c) => {
                        let arc = Arc::new(c);

                        match ctx.state.voice_sessions.write() {
                            Ok(mut sessions) => {
                                use std::collections::hash_map::Entry;
                                match sessions.entry(ctx.pseudonym.to_string()) {
                                    Entry::Vacant(entry) => {
                                        let mut rx = arc.subscribe_transcriptions();
                                        let cm = ctx.state.connection_manager.clone();
                                        let p_clone = ctx.pseudonym.to_string();

                                        tokio::spawn(async move {
                                            while let Ok(event) = rx.recv().await {
                                                let msg = OutgoingMessage::Transcription {
                                                    channel_id: event.channel_id,
                                                    speaker_pseudonym: event.speaker_pseudonym,
                                                    text: event.text,
                                                };

                                                match serde_json::to_string(&msg) {
                                                    Ok(json) => {
                                                        cm.send(&p_clone, json).await;
                                                    }
                                                    Err(e) => {
                                                        tracing::error!(
                                                            "failed to serialize transcription message: {}",
                                                            e
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
                                match sessions.get(ctx.pseudonym).cloned() {
                                    Some(s) => s,
                                    None => {
                                        tracing::error!(
                                            pseudonym = %ctx.pseudonym,
                                            "voice session missing after insert; this is a bug"
                                        );
                                        return;
                                    }
                                }
                            }
                            Err(_) => {
                                tracing::error!("voice_sessions lock poisoned");
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        send_ws_error(ctx.tx, format!("Failed to connect voice: {e}"));
                        return;
                    }
                }
            };

            if let Err(e) = client.publish_audio(&audio).await {
                send_ws_error(ctx.tx, format!("Failed to publish audio: {e}"));
            }
        }
        Err(e) => {
            send_ws_error(ctx.tx, format!("TTS failed: {e}"));
        }
    }
}

async fn handle_webrtc_offer(ctx: &CommandContext<'_>, channel_id: String, sdp: String) {
    match check_ws_membership(
        ctx.state.pool.clone(),
        ctx.state.server_id,
        &channel_id,
        ctx.pseudonym,
    )
    .await
    {
        MembershipResult::Allowed => {
            match ctx
                .state
                .voice_service
                .clone()
                .handle_sdp_offer(&channel_id, ctx.pseudonym, &sdp)
                .await
            {
                Ok(answer) => {
                    let out = OutgoingMessage::WebRtcAnswer {
                        channel_id,
                        sdp: answer.sdp,
                    };
                    match serde_json::to_string(&out) {
                        Ok(json) => {
                            let _ = ctx.tx.send(json).await;
                        }
                        Err(e) => {
                            tracing::error!("failed to serialize webrtc answer: {}", e);
                        }
                    }
                }
                Err(e) => send_ws_error(ctx.tx, format!("WebRTC offer handling failed: {e}")),
            }
        }
        MembershipResult::Denied => {
            send_ws_error(ctx.tx, format!("Not a member of channel {channel_id}"));
        }
        MembershipResult::Error(e) => {
            tracing::error!(
                pseudonym = %ctx.pseudonym,
                channel_id = %channel_id,
                "webrtc offer membership check failed: {}",
                e
            );
            send_ws_error(
                ctx.tx,
                "Internal error checking channel membership".to_string(),
            );
        }
    }
}

async fn handle_webrtc_ice(
    ctx: &CommandContext<'_>,
    channel_id: String,
    candidate: String,
    sdp_mid: Option<String>,
    sdp_m_line_index: Option<u16>,
    username_fragment: Option<String>,
) {
    let candidate = annex_voice::RTCIceCandidateInit {
        candidate,
        sdp_mid,
        sdp_mline_index: sdp_m_line_index,
        username_fragment,
    };

    if let Err(e) = ctx
        .state
        .voice_service
        .add_ice_candidate(&channel_id, ctx.pseudonym, candidate)
        .await
    {
        send_ws_error(ctx.tx, format!("Failed to add ICE candidate: {e}"));
    }
}

async fn handle_resume(ctx: &CommandContext<'_>, channel_id: String, last_message_id: String) {
    let state_clone: Arc<AppState> = ctx.state.clone();
    let pseudonym_clone = ctx.pseudonym.to_string();
    let tx_clone: mpsc::Sender<String> = ctx.tx.clone();
    let channel_id_for_ack = channel_id.clone();
    let pseudonym_for_log = ctx.pseudonym.to_string();

    let res = tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| e.to_string())?;
        // Verify membership
        let is_mem =
            annex_channels::is_member(&conn, state_clone.server_id, &channel_id, &pseudonym_clone)
                .map_err(|e| e.to_string())?;
        if !is_mem {
            return Ok::<Vec<annex_channels::Message>, String>(vec![]);
        }
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
        let mut stmt = conn
            .prepare(
                "SELECT id, server_id, channel_id, message_id, sender_pseudonym, content,
                        reply_to_message_id, created_at, expires_at, edited_at, deleted_at
                 FROM messages
                 WHERE server_id = ?1 AND channel_id = ?2
                   AND (created_at > ?3 OR (created_at = ?3 AND id > ?4))
                 ORDER BY created_at ASC, id ASC
                 LIMIT 200",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                rusqlite::params![state_clone.server_id, channel_id, ts, row_id],
                |row| {
                    Ok(annex_channels::Message {
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
                    })
                },
            )
            .map_err(|e| e.to_string())?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(|e| e.to_string())?);
        }
        Ok(messages)
    })
    .await;

    match res {
        Ok(Ok(messages)) => {
            let count = messages.len();
            for msg in messages {
                let ws_payload: WsMessagePayload = msg.into();
                let out = OutgoingMessage::Message(ws_payload);
                if let Ok(json) = serde_json::to_string(&out) {
                    if tx_clone.try_send(json).is_err() {
                        break;
                    }
                }
            }
            let ack = OutgoingMessage::Resumed {
                channel_id: channel_id_for_ack,
                missed_count: count,
            };
            if let Ok(json) = serde_json::to_string(&ack) {
                let _ = tx_clone.try_send(json);
            }
        }
        Ok(Err(e)) => {
            tracing::error!(pseudonym = %pseudonym_for_log, "resume failed: {}", e);
            send_ws_error(ctx.tx, format!("Resume failed: {e}"));
        }
        Err(e) => {
            tracing::error!(pseudonym = %pseudonym_for_log, "resume task failed: {}", e);
            send_ws_error(ctx.tx, "Resume failed: internal error".to_string());
        }
    }
}
