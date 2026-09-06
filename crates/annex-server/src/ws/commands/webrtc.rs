//! `IncomingMessage::WebRtcOffer` and
//! `IncomingMessage::WebRtcIceCandidate` — SDP-offer answering and ICE
//! relay for the in-process WebRTC voice layer.
//!
//! Behaviour preserved verbatim from the original inline arms:
//!
//!   * `handle_offer` runs the membership gate (same wording on
//!     Denied / Error), then asks the voice service to answer the
//!     offer. On success it pushes
//!     `OutgoingMessage::WebRtcAnswer { channelId, sdp }` directly to
//!     the originating socket via `tx.send` (NOT a broadcast — the
//!     answer is a unicast reply). On voice-service error the wording
//!     is `"WebRTC offer handling failed: <e>"`.
//!   * `handle_ice` does NOT run a membership check (matching the
//!     previous inline arm — ICE candidates are just plumbing for an
//!     already-negotiated session). It builds the
//!     `RTCIceCandidateInit` and forwards to `voice_service.add_ice_candidate`.
//!     On failure the wording is `"Failed to add ICE candidate: <e>"`.
//!
//! No protocol shape changes; field names on the outgoing answer
//! frame match the previous inline definition.

use crate::ws::context::CommandContext;
use crate::ws::dispatch::{check_ws_membership, MembershipResult};
use crate::ws::error::send_ws_error;
use crate::ws::protocol::OutgoingMessage;

pub(crate) async fn handle_offer(ctx: &CommandContext<'_>, channel_id: String, sdp: String) {
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

/// The client's answer to an offer the SERVER initiated.
///
/// The server offers when a peer's track set changes — somebody joined or left
/// the call — because adding a track to an established connection needs a
/// fresh offer/answer. Membership is re-checked here for the same reason it is
/// on the initial offer: the answer mutates a live peer connection, and a
/// member can be removed from a channel while a call is in progress.
pub(crate) async fn handle_answer(ctx: &CommandContext<'_>, channel_id: String, sdp: String) {
    // A non-member is dropped silently: an answer responds to something the
    // server asked for, so a refusal is not something the user did and not
    // something they can act on. The offer simply goes unanswered and the peer
    // keeps the track set it had.
    if !matches!(
        check_ws_membership(
            ctx.state.pool.clone(),
            ctx.state.server_id,
            &channel_id,
            ctx.pseudonym,
        )
        .await,
        MembershipResult::Allowed
    ) {
        return;
    }

    if let Err(e) = ctx
        .state
        .voice_service
        .handle_renegotiation_answer(&channel_id, ctx.pseudonym, &sdp)
        .await
    {
        tracing::debug!(
            pseudonym = %ctx.pseudonym,
            error = %e,
            "renegotiation answer rejected",
        );
    }
}

pub(crate) async fn handle_ice(
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
