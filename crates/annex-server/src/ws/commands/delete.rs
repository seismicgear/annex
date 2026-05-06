//! `IncomingMessage::DeleteMessage` — soft-delete one's own message
//! inside the edit window and broadcast the result.
//!
//! Behaviour preserved verbatim from the original inline arm:
//!
//!   1. Run the same membership gate as `edit`/`message` (same wording
//!      on Denied / Error).
//!   2. Delegate persistence to
//!      [`crate::services::ChannelService::delete_message`], which
//!      enforces ownership and the edit-window in
//!      `annex_channels::delete_message`.
//!   3. Broadcast `OutgoingMessage::MessageDeleted(WsMessagePayload)`
//!      to subscribers of the *persisted* channel id (not the
//!      client-supplied one) to prevent cross-channel broadcast
//!      spoofing.
//!   4. On service error surface `Delete failed: <e>` via
//!      `send_ws_error`.

use crate::ws::context::CommandContext;
use crate::ws::dispatch::{check_ws_membership, MembershipResult};
use crate::ws::error::send_ws_error;
use crate::ws::protocol::{OutgoingMessage, WsMessagePayload};

pub(crate) async fn handle(ctx: &CommandContext<'_>, channel_id: String, message_id: String) {
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
            // Use the persisted channel_id from DB, not the
            // client-supplied one, to prevent cross-channel broadcast
            // spoofing.
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
