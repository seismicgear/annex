//! `IncomingMessage::VoiceIntent` — synthesize TTS audio for an AI agent
//! and publish it into the channel's voice room.
//!
//! Behaviour preserved verbatim from the original inline arm:
//!
//!   1. Reject non-`AiAgent` participants with
//!      `"Only AI agents can use VoiceIntent"`.
//!   2. Reject empty / overlong text with the same wording, same
//!      `MAX_VOICE_INTENT_TEXT_LEN` cap.
//!   3. Same membership gate.
//!   4. Look up the agent's voice profile via the `agent_registrations`
//!      JOIN. Failures (DB / task-join / NULL) fall back to
//!      `"default"` with a warn-level trace, matching the previous
//!      handler exactly.
//!   5. Synthesize via `tts_service.synthesize(text, voice_profile_id)`.
//!   6. Reuse an existing `voice_sessions` entry for this pseudonym if
//!      one is present; otherwise generate a fresh voice-room token,
//!      connect a new `AgentVoiceClient`, and *atomically* insert the
//!      handle (winning insert subscribes to transcriptions; losing
//!      insert drops its client). Same TOCTOU-safe write-lock
//!      double-check as before.
//!   7. Publish the synthesized audio. Failures surface
//!      `"Failed to publish audio: <e>"`; TTS failures surface
//!      `"TTS failed: <e>"`.
//!
//! No protocol or capability changes.

use std::sync::Arc;

use rusqlite::OptionalExtension;

use crate::ws::context::CommandContext;
use crate::ws::dispatch::{check_ws_membership, MembershipResult, MAX_VOICE_INTENT_TEXT_LEN};
use crate::ws::error::send_ws_error;
use crate::ws::protocol::OutgoingMessage;
use annex_types::RoleCode;

pub(crate) async fn handle(ctx: &CommandContext<'_>, channel_id: String, text: String) {
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
            // Fast-path: read lock to check for an existing session.
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
                    &ctx.state.voice_token_secret,
                    annex_voice::VOICE_TOKEN_DEFAULT_TTL_SECS,
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
                    &ctx.state.voice_token_secret,
                    ctx.state.stt_service.clone(),
                    ctx.state.voice_service.api_key(),
                    ctx.state.voice_service.api_secret(),
                    ctx.state.voice_service.clone(),
                )
                .await
                {
                    Ok(c) => {
                        let arc = Arc::new(c);

                        // Double-check under write lock to prevent TOCTOU
                        // race with concurrent voice intents.
                        match ctx.state.voice_sessions.write() {
                            Ok(mut sessions) => {
                                use std::collections::hash_map::Entry;
                                match sessions.entry(ctx.pseudonym.to_string()) {
                                    Entry::Vacant(entry) => {
                                        // Subscribe to transcriptions only for the winning insert
                                        let mut rx = arc.subscribe_transcriptions();
                                        let cm = ctx.state.connection_manager.clone();
                                        let p_clone = ctx.pseudonym.to_string();

                                        // Differentiate `Lagged` from `Closed` so a brief
                                        // burst of transcription events that overflows the
                                        // 256-deep broadcast window does NOT terminate this
                                        // forwarder permanently. See [F36] for the analysis.
                                        tokio::spawn(async move {
                                            loop {
                                                let event = match rx.recv().await {
                                                    Ok(e) => e,
                                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                                        tracing::warn!(
                                                            pseudonym = %p_clone,
                                                            skipped = n,
                                                            "transcription broadcast lagged; some events skipped",
                                                        );
                                                        continue;
                                                    }
                                                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                                                };
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
