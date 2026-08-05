//! Per-connection WebSocket session orchestration.
//!
//! [`WsSession::run`] is what `api_ws::handle_socket` delegates to once
//! the upgrade is complete and the peer has been authenticated. It
//! owns the connection lifetime: it sets up the bounded outbound
//! `mpsc` channel, registers the session with the
//! [`crate::ws::ConnectionManager`], spawns the
//! mpsc-to-WebSocket forwarding task and the per-peer ICE-candidate
//! relay task, runs the message dispatch loop, and tears all of those
//! down on disconnect.
//!
//! Per-message logic lives in [`crate::ws::dispatch`], which routes
//! each variant of [`crate::ws::protocol::IncomingMessage`] to a
//! handler under [`crate::ws::commands`].
//!
//! Activity-debouncing (`ACTIVITY_DEBOUNCE`) and the
//! [`touch_activity`] helper live here because they are part of the
//! connection's heartbeat — not of any single command.

use std::sync::Arc;

use annex_identity::PlatformIdentity;
use axum::extract::ws::{Message as AxumMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::ws::command_rate_limit::CommandRateLimiter;
use crate::ws::context::CommandContext;
use crate::ws::dispatch::dispatch;
use crate::ws::error::send_ws_error;
use crate::ws::protocol::{IncomingMessage, OutgoingMessage};
use crate::ws::typing_throttle::TypingThrottle;
use crate::AppState;

/// Minimum interval between activity updates per WebSocket connection.
/// Prevents spawning a blocking DB task on every single message.
const ACTIVITY_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(30);

/// Drives a single authenticated WebSocket connection from upgrade to
/// disconnect. Called from `api_ws::handle_socket` after auth.
pub struct WsSession;

impl WsSession {
    pub async fn run(socket: WebSocket, state: Arc<AppState>, identity: PlatformIdentity) {
        let pseudonym = identity.pseudonym_id.clone();

        // Mark as active immediately
        tokio::spawn(touch_activity(state.clone(), pseudonym.clone()));

        let (mut sender, mut receiver) = socket.split();

        // Bounded outbound queue: 256 messages. Beyond that the client
        // is too slow and messages are dropped by the per-broadcast
        // try_send paths.
        let (tx, mut rx) = mpsc::channel::<String>(256);

        let session_id = state
            .connection_manager
            .add_session(pseudonym.clone(), tx.clone())
            .await;

        // Forward outbound mpsc → websocket.
        let send_task = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if sender.send(AxumMessage::Text(msg.into())).await.is_err() {
                    break;
                }
            }
        });

        // Relay this peer's ICE candidates to the websocket.
        //
        // The receiver is on a `tokio::sync::broadcast` channel with
        // capacity 1024. `recv()` returns three variants:
        //   * `Ok(event)`      — normal delivery
        //   * `Err(Lagged(n))` — the global broadcast queue overflowed
        //                        the receiver's window; n events were
        //                        skipped but the channel is still open.
        //                        We log + continue so the per-session
        //                        ICE forwarder STAYS ALIVE. Pre-[F36]
        //                        a `while let Ok(_)` loop terminated on
        //                        Lagged, permanently disabling ICE
        //                        forwarding for this WS connection and
        //                        making the peer's voice unreachable
        //                        until they reconnected.
        //   * `Err(Closed)`    — the global sender dropped (server
        //                        shutdown); break and exit the task.
        let mut ice_rx = state.voice_service.subscribe_ice_candidates();
        let tx_for_ice = tx.clone();
        let pseudonym_for_ice = pseudonym.clone();
        let ice_task = tokio::spawn(async move {
            loop {
                let event = match ice_rx.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            pseudonym = %pseudonym_for_ice,
                            skipped = n,
                            "ice candidate broadcast lagged; some candidates skipped for this session",
                        );
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if event.peer_id != pseudonym_for_ice {
                    continue;
                }
                let outbound = OutgoingMessage::WebRtcIceCandidate {
                    channel_id: event.channel_id,
                    candidate: event.candidate.candidate,
                    sdp_mid: event.candidate.sdp_mid,
                    sdp_m_line_index: event.candidate.sdp_mline_index,
                    username_fragment: event.candidate.username_fragment,
                };
                match serde_json::to_string(&outbound) {
                    Ok(json) => {
                        if tx_for_ice.send(json).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => tracing::error!("failed to serialize webrtc ice candidate: {}", e),
                }
            }
        });

        // Server-initiated offers, on the same broadcast-and-filter shape as
        // ICE above.
        //
        // When a peer joins or leaves a call every OTHER peer's track set
        // changes, and adding a track to an established connection requires a
        // fresh offer/answer. Without this task those offers are generated and
        // never delivered, so a call can never grow past the participants it
        // started with. `Lagged` is skipped rather than fatal for the same
        // reason as ICE: dropping this session's renegotiation permanently
        // would freeze its participant list for the rest of the call.
        let mut reneg_rx = state.voice_service.subscribe_renegotiations();
        let tx_for_reneg = tx.clone();
        let pseudonym_for_reneg = pseudonym.clone();
        let reneg_task = tokio::spawn(async move {
            loop {
                let event = match reneg_rx.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            pseudonym = %pseudonym_for_reneg,
                            skipped = n,
                            "renegotiation broadcast lagged; some offers skipped for this session",
                        );
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if event.peer_id != pseudonym_for_reneg {
                    continue;
                }
                let outbound = OutgoingMessage::WebRtcOffer {
                    channel_id: event.channel_id,
                    sdp: event.sdp,
                };
                match serde_json::to_string(&outbound) {
                    Ok(json) => {
                        if tx_for_reneg.send(json).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => tracing::error!("failed to serialize webrtc offer: {}", e),
                }
            }
        });

        let mut last_activity = std::time::Instant::now();
        // Per-session throttle for IncomingMessage::Typing. Typing
        // events are not subject to the HTTP rate-limit middleware —
        // without a per-session debouncer a malicious client can fan a
        // single WS connection out to N subscribers per channel at any
        // rate the OS will deliver frames.
        let typing_throttle = TypingThrottle::new();
        // Per-session token bucket for state-mutating commands. Typing has
        // its own debouncer; this covers message/edit/delete/voice/resume,
        // which the HTTP rate-limit middleware never sees.
        let command_rate_limiter = CommandRateLimiter::new();

        while let Some(Ok(msg)) = receiver.next().await {
            if last_activity.elapsed() >= ACTIVITY_DEBOUNCE {
                tokio::spawn(touch_activity(state.clone(), pseudonym.clone()));
                last_activity = std::time::Instant::now();
            }

            match msg {
                AxumMessage::Text(text) => {
                    match serde_json::from_str::<IncomingMessage>(&text.to_string()) {
                        Ok(incoming) => {
                            let ctx = CommandContext {
                                state: &state,
                                identity: &identity,
                                pseudonym: &pseudonym,
                                tx: &tx,
                                typing_throttle: &typing_throttle,
                                command_rate_limiter: &command_rate_limiter,
                            };
                            dispatch(&ctx, incoming).await;
                        }
                        Err(_) => {
                            tracing::warn!(
                                pseudonym = %pseudonym,
                                "failed to parse incoming WebSocket message"
                            );
                            send_ws_error(&tx, "invalid message format".to_string());
                        }
                    }
                }
                AxumMessage::Close(_) => break,
                _ => {}
            }
        }

        // Cleanup with session_id check
        state
            .connection_manager
            .remove_session(&pseudonym, session_id)
            .await;
        send_task.abort();
        ice_task.abort();
        reneg_task.abort();

        // Leave any call this session was in.
        //
        // Peers were previously only removed by an explicit leave, so closing a
        // tab, losing a network, or crashing left them in the SFU room
        // forever — still on the roster, still holding a track slot on every
        // other peer's connection, and keeping the room alive so it was never
        // reaped. Everyone else saw a tile for somebody who had gone.
        let left = state
            .voice_service
            .remove_participant_everywhere(&pseudonym)
            .await;
        if !left.is_empty() {
            tracing::debug!(
                pseudonym = %pseudonym,
                channels = ?left,
                "removed disconnected peer from voice rooms",
            );
        }

        // Clean up voice session for this pseudonym. Dropping the Arc
        // decrements its reference count; when it reaches zero the
        // AgentVoiceClient drops, its broadcast sender closes, and the
        // spawned transcription task exits naturally.
        match state.voice_sessions.write() {
            Ok(mut sessions) => {
                sessions.remove(&pseudonym);
            }
            Err(e) => {
                tracing::error!(
                    pseudonym = %pseudonym,
                    "voice_sessions RwLock poisoned during cleanup: {}", e
                );
            }
        }
    }
}

async fn touch_activity(state: Arc<AppState>, pseudonym: String) {
    let pool = state.pool.clone();
    let server_id = state.server_id;
    let tx = state.presence_tx.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| {
            tracing::warn!("touch_activity: failed to get db connection: {}", e);
        })?;
        match annex_graph::update_node_activity(&conn, server_id, &pseudonym) {
            Ok(true) => {
                let _ = tx.send(annex_types::PresenceEvent::NodeUpdated {
                    pseudonym_id: pseudonym,
                    active: true,
                });
            }
            Ok(false) => { /* already active, no broadcast needed */ }
            Err(e) => {
                tracing::warn!(
                    pseudonym = %pseudonym,
                    "touch_activity: failed to update node activity: {}",
                    e
                );
            }
        }
        Ok::<(), ()>(())
    })
    .await;

    if let Err(e) = result {
        tracing::error!(
            "touch_activity: blocking task panicked or was cancelled: {}",
            e
        );
    }
}
