//! `IncomingMessage::EditMessage` — edit one's own message inside the
//! edit window and broadcast the result.
//!
//! Behaviour preserved verbatim from the original inline arm:
//!
//!   1. Reject empty / overlong content with the same `Message content
//!      must not be empty` and `… exceeds maximum length …` strings,
//!      same `MAX_WS_MESSAGE_CONTENT_LEN` cap.
//!   2. Run the same membership gate (`Not a member of channel <id>`
//!      on Denied; logged + `Internal error checking channel
//!      membership` on Error).
//!   3. Delegate persistence to
//!      [`crate::services::ChannelService::edit_message`], which
//!      enforces ownership and the edit-window in
//!      `annex_channels::edit_message`.
//!   4. Broadcast `OutgoingMessage::MessageEdited(WsMessagePayload)`
//!      to subscribers of the *persisted* channel id (not the
//!      client-supplied one) to prevent cross-channel broadcast
//!      spoofing.
//!   5. On service error surface `Edit failed: <e>` via
//!      `send_ws_error`.
//!   6. Federation delivery runs from the durable outbox. The service commits
//!      signed edit envelopes in the same transaction as the body and history,
//!      before this handler acknowledges the edit by broadcasting it.

use crate::ws::context::CommandContext;
use crate::ws::dispatch::{check_ws_membership, MembershipResult, MAX_WS_MESSAGE_CONTENT_LEN};
use crate::ws::error::send_ws_error_with_id;
use crate::ws::protocol::{OutgoingMessage, WsMessagePayload};

pub(crate) async fn handle(
    ctx: &CommandContext<'_>,
    channel_id: String,
    message_id: String,
    content: String,
    attachment_ids: Option<Vec<String>>,
    client_request_id: Option<String>,
) {
    // Every error this handler can send goes through here, so none of them
    // can lose the correlation id and leave the client unable to tell which
    // of its in-flight operations was refused.
    let fail = |msg: String| send_ws_error_with_id(ctx.tx, msg, client_request_id.clone());
    if content.trim().is_empty() {
        fail("Message content must not be empty".to_string());
        return;
    }
    if content.len() > MAX_WS_MESSAGE_CONTENT_LEN {
        fail(format!(
            "Message content exceeds maximum length of {MAX_WS_MESSAGE_CONTENT_LEN} bytes"
        ));
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
            fail(format!("Not a member of channel {channel_id}"));
            return;
        }
        MembershipResult::Error(e) => {
            tracing::error!(
                pseudonym = %ctx.pseudonym,
                channel_id = %channel_id,
                "edit membership check failed: {}",
                e
            );
            fail("Internal error checking channel membership".to_string());
            return;
        }
    }

    let svc = crate::services::ChannelService::new(ctx.state.clone());
    let remote=crate::services::channel_replication::maybe_message(
        ctx.state.clone(),ctx.identity,&channel_id,client_request_id.as_deref(),
        serde_json::json!({"kind":"edit","message_id":message_id,"content":content,"attachments":null}),attachment_ids.as_deref(),
    ).await;
    let result = match remote {
        Ok(Some(message)) => Ok((message, true)),
        Err(error) => Err(error.into()),
        Ok(None) => {
            svc.edit_message_with_attachments(
                ctx.pseudonym,
                &channel_id,
                &message_id,
                &content,
                attachment_ids,
            )
            .await
        }
    };
    match result {
        Ok((updated, _is_federated)) => {
            // Use the persisted channel_id from DB, not the
            // client-supplied one, to prevent cross-channel broadcast
            // spoofing.
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

            // The service committed the signed outbox obligation with the
            // edit. A detached relay here would recreate the acknowledgement
            // before durability gap and would enqueue a duplicate edit event.
        }
        Err(e) => {
            fail(format!("Edit failed: {e}"));
        }
    }
}
