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
//!   4. Resolve the assigned profile. Use `default` only for an explicit
//!      unassigned profile; database failures and missing assignments fail closed.
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

use rusqlite::OptionalExtension;

use crate::ws::context::CommandContext;
use crate::ws::dispatch::{check_ws_membership, MembershipResult, MAX_VOICE_INTENT_TEXT_LEN};
use crate::ws::error::send_ws_error;
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

    // Alignment, at the moment of the action.
    //
    // VoiceIntent bypasses `ChannelService` entirely, so the gate in
    // `ensure_voice_allowed` does not cover it. Without this an agent swept to
    // Conflict keeps the server's voice speaking its words in every channel it
    // had already joined.
    {
        let pool = ctx.state.pool.clone();
        let server_id = ctx.state.server_id;
        let pid = ctx.pseudonym.to_string();
        let checked = tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| {
                crate::services::agent_policy::ActionRefusal::Internal(format!("pool: {e}"))
            })?;
            crate::services::agent_policy::check_agent_action(
                &conn,
                server_id,
                &pid,
                crate::services::agent_policy::AgentAction::VoiceIntent,
            )
        })
        .await;
        match checked {
            Ok(Ok(())) => {}
            Ok(Err(crate::services::agent_policy::ActionRefusal::Forbidden(m))) => {
                send_ws_error(ctx.tx, m);
                return;
            }
            Ok(Err(crate::services::agent_policy::ActionRefusal::Internal(e))) => {
                tracing::error!(
                    pseudonym = %ctx.pseudonym,
                    "voice intent alignment check failed: {e}",
                );
                send_ws_error(ctx.tx, "Internal error checking agent policy".to_string());
                return;
            }
            Err(e) => {
                tracing::error!(pseudonym = %ctx.pseudonym, "voice intent alignment task failed: {e}");
                send_ws_error(ctx.tx, "Internal error checking agent policy".to_string());
                return;
            }
        }
    }

    let voice_profile_id = {
        let pool = ctx.state.pool.clone();
        let server_id = ctx.state.server_id;
        let pid = ctx.pseudonym.to_string();
        let result = tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| format!("pool error: {e}"))?;
            let profile: Option<(Option<i64>,Option<String>)> = conn.query_row(
                "SELECT ar.voice_profile_id,vp.profile_id FROM agent_registrations ar LEFT JOIN voice_profiles vp ON ar.voice_profile_id=vp.id AND vp.server_id=ar.server_id WHERE ar.server_id=?1 AND ar.pseudonym_id=?2 AND ar.active=1",
                rusqlite::params![server_id,pid],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|e|format!("db error: {e}"))?;
            match profile {
                Some((None,_))=>Ok("default".to_string()),
                Some((Some(_),Some(id)))=>Ok(id),
                Some((Some(_),None))=>Err("assigned voice profile is unavailable".to_string()),
                None=>Err("active agent registration is unavailable".to_string()),
            }
        }).await;
        match result {
            Ok(Ok(id)) => id,
            Ok(Err(error)) => {
                tracing::warn!(pseudonym=%ctx.pseudonym,%error,"voice profile resolution refused");
                send_ws_error(ctx.tx, "The assigned voice profile is unavailable".into());
                return;
            }
            Err(error) => {
                tracing::error!(pseudonym=%ctx.pseudonym,%error,"voice profile lookup task failed");
                send_ws_error(ctx.tx, "Voice profile lookup failed".into());
                return;
            }
        }
    };

    let svc = crate::services::ChannelService::new(ctx.state.clone());
    if let Err(e) = svc.ensure_voice_allowed(ctx.identity, &channel_id).await {
        send_ws_error(ctx.tx, e.to_string());
        return;
    }

    match ctx
        .state
        .tts_service
        .synthesize_audio(&text, &voice_profile_id)
        .await
    {
        Ok(audio) => {
            // Synthesis may take seconds. Recheck admission after it finishes so
            // a leave, deletion or policy change cannot create a new voice client.
            if !matches!(
                check_ws_membership(
                    ctx.state.pool.clone(),
                    ctx.state.server_id,
                    &channel_id,
                    ctx.pseudonym
                )
                .await,
                MembershipResult::Allowed
            ) {
                send_ws_error(ctx.tx, format!("Not a member of channel {channel_id}"));
                return;
            }
            if let Err(e) = svc.ensure_voice_allowed(ctx.identity, &channel_id).await {
                send_ws_error(ctx.tx, e.to_string());
                return;
            }
            match svc
                .connect_agent_voice_client(ctx.identity, &channel_id)
                .await
            {
                Ok(client) => {
                    if let Err(e) = svc.ensure_voice_allowed(ctx.identity, &channel_id).await {
                        svc.disconnect_voice_participant(ctx.pseudonym).await;
                        send_ws_error(ctx.tx, e.to_string());
                        return;
                    }
                    if let Err(e) = client
                        .publish_audio_with_format(
                            &audio.pcm_s16le,
                            audio.sample_rate,
                            audio.channels,
                        )
                        .await
                    {
                        send_ws_error(ctx.tx, format!("Failed to publish audio: {e}"));
                    }
                }
                Err(e) => send_ws_error(ctx.tx, format!("Failed to connect voice: {e}")),
            }
        }
        Err(e) => send_ws_error(ctx.tx, format!("TTS failed: {e}")),
    }
}
