//! `IncomingMessage::Message` — send a new message into a channel,
//! broadcast to local subscribers, and (if the channel is federated)
//! relay to peer servers.
//!
//! Behaviour preserved verbatim from the original inline arm:
//!
//!   1. Reject empty / overlong content with the same
//!      `Message content must not be empty` and `… exceeds maximum
//!      length …` strings, same `MAX_WS_MESSAGE_CONTENT_LEN` cap.
//!      `clientRequestId` is echoed in the error frame.
//!   2. Run the same membership gate (same wording on Denied / Error,
//!      `clientRequestId` echoed).
//!   3. Delegate persistence + the federation-flag lookup to
//!      [`crate::services::ChannelService::send_message`]. The service
//!      owns the `INSERT INTO messages` + `SELECT … federation_scope`
//!      pair; this handler only orchestrates the frame around it.
//!   4. Broadcast `OutgoingMessage::Message(WsMessagePayload)` to
//!      subscribers of the persisted channel id. The sender's
//!      `clientRequestId` is included on the payload so they can
//!      correlate the broadcast with their pending send; other
//!      clients ignore unrecognised IDs (random UUIDs, no information
//!      leak).
//!   5. If `is_federated`, spawn `relay_message(state, channel_id,
//!      message)` exactly as before so peer servers receive the
//!      message asynchronously. The federation relay call site is
//!      preserved; the relay logic itself is unchanged.

use crate::api_federation::relay_message;
use crate::ws::context::CommandContext;
use crate::ws::dispatch::{check_ws_membership, MembershipResult, MAX_WS_MESSAGE_CONTENT_LEN};
use crate::ws::error::send_ws_error_with_id;
use crate::ws::protocol::{OutgoingMessage, WsMessagePayload};

pub(crate) async fn handle(
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

    let svc = crate::services::ChannelService::new(ctx.state.clone());
    match svc
        .send_message(ctx.pseudonym, &channel_id, content, reply_to)
        .await
    {
        Ok((message, is_federated)) => {
            // Broadcast via WebSocket (camelCase payload).
            // clientRequestId is included in the broadcast for the
            // sender's pending-send correlation. Other clients ignore
            // unrecognised IDs (random UUIDs, no information leak).
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

            // Relay if federated.
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
