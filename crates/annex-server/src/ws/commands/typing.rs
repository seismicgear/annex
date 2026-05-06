//! `IncomingMessage::Typing` — broadcast a typing indicator to every
//! subscriber of a channel.
//!
//! Behaviour preserved verbatim from the original inline arm:
//!
//!   * If the sender is not a channel member, the indicator is dropped
//!     silently (no error frame is emitted). Membership-check errors
//!     are also silenced — non-members and DB blips look identical to
//!     the client, which is the behaviour the wire protocol relies on.
//!   * If the sender is a member, broadcast
//!     `OutgoingMessage::Typing { channel_id, pseudonym_id }` to every
//!     subscriber of the channel (including the typer; the frontend
//!     filters its own pseudonym out).
//!
//! No protocol or capability changes.

use crate::ws::context::CommandContext;
use crate::ws::dispatch::{check_ws_membership, MembershipResult};
use crate::ws::protocol::OutgoingMessage;

pub(crate) async fn handle(ctx: &CommandContext<'_>, channel_id: String) {
    let MembershipResult::Allowed = check_ws_membership(
        ctx.state.pool.clone(),
        ctx.state.server_id,
        &channel_id,
        ctx.pseudonym,
    )
    .await
    else {
        // Silently ignore typing from non-members and on DB errors; the
        // typing indicator is best-effort and must not leak channel
        // existence to non-members.
        return;
    };

    let out = OutgoingMessage::Typing {
        channel_id: channel_id.clone(),
        pseudonym_id: ctx.pseudonym.to_string(),
    };
    if let Ok(json) = serde_json::to_string(&out) {
        ctx.state
            .connection_manager
            .broadcast(&channel_id, json)
            .await;
    }
}
