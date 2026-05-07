//! `IncomingMessage::Typing` — broadcast a typing indicator to every
//! subscriber of a channel.
//!
//! Behaviour preserved verbatim from the original inline arm, with one
//! addition (no wire-protocol change):
//!
//!   * If the sender is not a channel member, the indicator is dropped
//!     silently (no error frame is emitted). Membership-check errors
//!     are also silenced — non-members and DB blips look identical to
//!     the client, which is the behaviour the wire protocol relies on.
//!   * If the sender's per-session throttle rejects the event (last
//!     admitted typing for this channel was within
//!     [`crate::ws::typing_throttle::TYPING_DEBOUNCE`]), the indicator
//!     is dropped silently. Legitimate clients re-send typing events at
//!     ~1Hz and stay under the cap; a malicious client cannot
//!     amplify a single connection into a broadcast flood.
//!   * If both gates pass, broadcast
//!     `OutgoingMessage::Typing { channel_id, pseudonym_id }` to every
//!     subscriber of the channel (including the typer; the frontend
//!     filters its own pseudonym out).

use crate::ws::context::CommandContext;
use crate::ws::dispatch::{check_ws_membership, MembershipResult};
use crate::ws::protocol::OutgoingMessage;

pub(crate) async fn handle(ctx: &CommandContext<'_>, channel_id: String) {
    // Membership gate first: a non-member must not be able to
    // discover, by the throttle's behaviour, whether a channel exists.
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

    if !ctx.typing_throttle.try_admit(&channel_id).await {
        // Within the per-session debounce window for this channel — drop
        // the event. No error frame: the client is expected to keep
        // re-sending at ~1Hz, and admitting one in N has the same UX
        // effect (the typing indicator stays lit on receivers).
        return;
    }

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
