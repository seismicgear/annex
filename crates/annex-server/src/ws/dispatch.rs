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

use crate::ws::commands::{delete, edit, message, resume, typing, voice, webrtc};
use crate::ws::context::CommandContext;
use crate::ws::error::send_ws_error;
use crate::ws::protocol::IncomingMessage;
use annex_channels::is_member;

/// Maximum allowed length for a WebSocket message content field (64 KiB).
pub(crate) const MAX_WS_MESSAGE_CONTENT_LEN: usize = 65_536;

/// Maximum allowed length for a VoiceIntent text field (2 KiB).
/// TTS synthesis is CPU/memory intensive; limiting input size prevents
/// resource abuse from oversized text payloads.
pub(crate) const MAX_VOICE_INTENT_TEXT_LEN: usize = 2_048;

/// Error surfaced when a session exceeds its per-connection command
/// budget (see [`crate::ws::command_rate_limit`]).
const RATE_LIMIT_MESSAGE: &str = "Rate limit exceeded: slow down and retry";

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
            // Per-session flood control. The error echoes clientRequestId
            // so the sender's pending-send promise resolves as a failure
            // rather than hanging.
            if !ctx.command_rate_limiter.try_admit().await {
                crate::ws::error::send_ws_error_with_id(
                    ctx.tx,
                    RATE_LIMIT_MESSAGE.to_string(),
                    client_request_id,
                );
                return;
            }
            message::handle(ctx, channel_id, content, reply_to, client_request_id).await;
        }
        IncomingMessage::EditMessage {
            channel_id,
            message_id,
            content,
            client_request_id,
        } => {
            if !ctx.command_rate_limiter.try_admit().await {
                crate::ws::error::send_ws_error_with_id(
                    ctx.tx,
                    RATE_LIMIT_MESSAGE.to_string(),
                    client_request_id,
                );
                return;
            }
            edit::handle(ctx, channel_id, message_id, content, client_request_id).await;
        }
        IncomingMessage::DeleteMessage {
            channel_id,
            message_id,
            client_request_id,
        } => {
            if !ctx.command_rate_limiter.try_admit().await {
                crate::ws::error::send_ws_error_with_id(
                    ctx.tx,
                    RATE_LIMIT_MESSAGE.to_string(),
                    client_request_id,
                );
                return;
            }
            delete::handle(ctx, channel_id, message_id, client_request_id).await;
        }
        IncomingMessage::VoiceIntent { channel_id, text } => {
            if !ctx.command_rate_limiter.try_admit().await {
                send_ws_error(ctx.tx, RATE_LIMIT_MESSAGE.to_string());
                return;
            }
            voice::handle(ctx, channel_id, text).await;
        }
        IncomingMessage::WebRtcOffer { channel_id, sdp } => {
            webrtc::handle_offer(ctx, channel_id, sdp).await;
        }
        IncomingMessage::WebRtcAnswer { channel_id, sdp } => {
            webrtc::handle_answer(ctx, channel_id, sdp).await;
        }
        IncomingMessage::WebRtcIceCandidate {
            channel_id,
            candidate,
            sdp_mid,
            sdp_m_line_index,
            username_fragment,
        } => {
            webrtc::handle_ice(
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
            // Resume runs an indexed range scan per call — rate-limit it
            // alongside the write commands.
            if !ctx.command_rate_limiter.try_admit().await {
                send_ws_error(ctx.tx, RATE_LIMIT_MESSAGE.to_string());
                return;
            }
            resume::handle(ctx, channel_id, last_message_id).await;
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
