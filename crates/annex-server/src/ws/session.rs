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

use std::sync::{atomic::Ordering, Arc};

use annex_identity::PlatformIdentity;
use axum::extract::ws::{CloseFrame, Message as AxumMessage, WebSocket};
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

        let replacement_state = state.clone();
        let replacement_pseudonym = pseudonym.clone();
        let (session_id, cancel, close_code) = {
            let _lifecycle = state.connection_manager.lock_lifecycle(&pseudonym).await;
            // HTTP upgrade authorization can precede a moderator's revocation.
            // Check again while holding the same guard as epoch invalidation,
            // before replacing a current socket or exposing any broadcasts.
            let pool = state.pool.clone();
            let server_id = state.server_id;
            let id = pseudonym.clone();
            let epoch = identity.token_epoch;
            let authorized = tokio::task::spawn_blocking(move || {
                let conn = pool.get().ok()?;
                let current = annex_identity::get_platform_identity(&conn, server_id, &id).ok()?;
                Some(current.active && current.token_epoch == epoch)
            })
            .await;
            if !matches!(authorized, Ok(Some(true))) {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    sender.send(AxumMessage::Close(Some(CloseFrame {
                        code: 4001,
                        reason: "session_revoked".into(),
                    }))),
                )
                .await;
                return;
            }
            state
                .connection_manager
                .add_session_with_cleanup_locked(pseudonym.clone(), tx.clone(), async move {
                    crate::services::ChannelService::new(replacement_state)
                        .disconnect_voice_participant(&replacement_pseudonym)
                        .await;
                })
                .await
        };

        // Closure has priority over a full application queue. A takeover must
        // transmit its Close frame; aborting this task looks like network loss
        // and makes two linked devices reconnect and evict each other forever.
        let writer_cancel = cancel.clone();
        let writer_close_code = close_code.clone();
        let writer_shutdown = state.shutdown.clone();
        let mut send_task = tokio::spawn(async move {
            loop {
                let msg = tokio::select! {
                    biased;
                    _ = writer_cancel.cancelled() => break,
                    _ = writer_shutdown.cancelled() => {
                        let _ = writer_close_code.compare_exchange(0, 1001, Ordering::SeqCst, Ordering::SeqCst);
                        break;
                    }
                    msg = rx.recv() => match msg {
                        Some(msg) => msg,
                        None => break,
                    },
                };
                tokio::select! {
                    biased;
                    _ = writer_cancel.cancelled() => break,
                    _ = writer_shutdown.cancelled() => {
                        let _ = writer_close_code.compare_exchange(0, 1001, Ordering::SeqCst, Ordering::SeqCst);
                        break;
                    }
                    result = sender.send(AxumMessage::Text(msg.into())) => {
                        if result.is_err() { return; }
                    }
                }
            }
            let (code, reason) = match writer_close_code.load(Ordering::SeqCst) {
                4090 => (4090, "identity_active_elsewhere"),
                4001 => (4001, "session_revoked"),
                1001 => (1001, "server_shutting_down"),
                _ => (1000, "session_closed"),
            };
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                sender.send(AxumMessage::Close(Some(CloseFrame {
                    code,
                    reason: reason.into(),
                }))),
            )
            .await;
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

        // `select!` on the cancellation token, not just the socket.
        //
        // The loop used to be `while let Some(Ok(msg)) = receiver.next()`,
        // which ends only when the CLIENT closes. A session replaced by a new
        // connection from the same identity had its `Sender` dropped — so the
        // writer task exited and it stopped receiving anything — while this
        // reader, the ICE relay and the renegotiation relay all stayed alive
        // on a socket nobody could reach. Reconnecting in a loop therefore
        // pinned an unbounded number of sockets to ONE valid token, and the
        // session registry never grew, so it did not read as a leak.
        //
        // The same token is what makes `disconnect_user` mean something:
        // clearing the registry entry left the socket open before this.
        loop {
            let msg = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    tracing::debug!(
                        pseudonym = %pseudonym,
                        "closing WebSocket session: replaced or disconnected by the server"
                    );
                    break;
                }
                _ = state.shutdown.cancelled() => {
                    let _ = close_code.compare_exchange(0, 1001, Ordering::SeqCst, Ordering::SeqCst);
                    tracing::debug!(pseudonym = %pseudonym, "closing WebSocket session: server shutting down");
                    break;
                }
                next = receiver.next() => match next {
                    Some(Ok(msg)) => msg,
                    // A transport error or a clean end of stream both mean the
                    // socket is finished. The old `while let Some(Ok(_))`
                    // treated them identically too — spelled out here so the
                    // `break` is visible rather than implied by the pattern.
                    _ => break,
                },
            };

            if last_activity.elapsed() >= ACTIVITY_DEBOUNCE {
                tokio::spawn(touch_activity(state.clone(), pseudonym.clone()));
                last_activity = std::time::Instant::now();
            }

            match msg {
                AxumMessage::Text(text) => {
                    match serde_json::from_str::<IncomingMessage>(&text.to_string()) {
                        Ok(incoming) => {
                            // Each frame uses current authority. Capability changes
                            // affect the existing text socket without disconnecting it.
                            let command = async {
                                let pool = state.pool.clone();
                                let server_id = state.server_id;
                                let pid = pseudonym.clone();
                                let fresh = tokio::task::spawn_blocking(move || {
                                    let conn = pool.get().map_err(|e| e.to_string())?;
                                    annex_identity::get_platform_identity(&conn, server_id, &pid)
                                        .map_err(|e| e.to_string())
                                })
                                .await;
                                let current = match fresh {
                                    Ok(Ok(current))
                                        if current.active
                                            && current.token_epoch == identity.token_epoch =>
                                    {
                                        current
                                    }
                                    _ => {
                                        let _ = close_code.compare_exchange(
                                            0,
                                            4001,
                                            Ordering::SeqCst,
                                            Ordering::SeqCst,
                                        );
                                        return false;
                                    }
                                };
                                let ctx = CommandContext {
                                    state: &state,
                                    identity: &current,
                                    pseudonym: &pseudonym,
                                    tx: &tx,
                                    typing_throttle: &typing_throttle,
                                    command_rate_limiter: &command_rate_limiter,
                                };
                                dispatch(&ctx, incoming).await;
                                true
                            };
                            let keep_open = tokio::select! {
                                biased;
                                _ = cancel.cancelled() => false,
                                _ = state.shutdown.cancelled() => {
                                    let _ = close_code.compare_exchange(0, 1001, Ordering::SeqCst, Ordering::SeqCst);
                                    false
                                },
                                keep_open = command => keep_open,
                            };
                            if !keep_open {
                                break;
                            }
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

        cancel.cancel();
        ice_task.abort();
        reneg_task.abort();
        if tokio::time::timeout(std::time::Duration::from_millis(1500), &mut send_task)
            .await
            .is_err()
        {
            send_task.abort();
            let _ = send_task.await;
        }
        let cleanup_state = state.clone();
        let cleanup_pseudonym = pseudonym.clone();
        state
            .connection_manager
            .finalize_session(&pseudonym, session_id, async move {
                crate::services::ChannelService::new(cleanup_state)
                    .disconnect_voice_participant(&cleanup_pseudonym)
                    .await;
            })
            .await;
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
