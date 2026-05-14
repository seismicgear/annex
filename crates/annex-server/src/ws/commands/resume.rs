//! `IncomingMessage::Resume` — replay missed messages since a given
//! `last_message_id` and acknowledge with the count.
//!
//! Behaviour preserved verbatim from the original inline arm:
//!
//! 1. Run a blocking task that verifies channel membership (non-members
//!    get an empty `messages` list — no error frame), resolves the
//!    supplied `last_message_id` to its `(created_at, id)` cursor (an
//!    unknown id also produces an empty list), and selects the next 200
//!    messages strictly after that cursor, ordered by
//!    `(created_at, id)` ascending.
//! 2. Forwards each missed message back as an
//!    `OutgoingMessage::Message { … }` frame on the per-connection mpsc
//!    (using `try_send`, breaking on backpressure exactly as before).
//! 3. Sends a final `OutgoingMessage::Resumed { channelId, missedCount }`
//!    ack with the number of messages enqueued (zero on the empty paths
//!    above).
//! 4. Surfaces blocking errors / task-join errors via `send_ws_error`
//!    with the same wording as the previous arm.

use std::sync::Arc;

use rusqlite::OptionalExtension;
use tokio::sync::mpsc;

use crate::ws::context::CommandContext;
use crate::ws::error::send_ws_error;
use crate::ws::protocol::{OutgoingMessage, WsMessagePayload};
use crate::AppState;

pub(crate) async fn handle(ctx: &CommandContext<'_>, channel_id: String, last_message_id: String) {
    let state_clone: Arc<AppState> = ctx.state.clone();
    let pseudonym_clone = ctx.pseudonym.to_string();
    let tx_clone: mpsc::Sender<String> = ctx.tx.clone();
    let channel_id_for_ack = channel_id.clone();
    let pseudonym_for_log = ctx.pseudonym.to_string();

    let res = tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| e.to_string())?;
        let is_mem =
            annex_channels::is_member(&conn, state_clone.server_id, &channel_id, &pseudonym_clone)
                .map_err(|e| e.to_string())?;
        if !is_mem {
            return Ok::<Vec<annex_channels::Message>, String>(vec![]);
        }
        let cursor: Option<(String, i64)> = conn
            .query_row(
                "SELECT created_at, id FROM messages WHERE message_id = ?1",
                [&last_message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((ts, row_id)) = cursor else {
            return Ok(vec![]);
        };
        let mut stmt = conn
            .prepare(
                "SELECT id, server_id, channel_id, message_id, sender_pseudonym, content,
                        reply_to_message_id, created_at, expires_at, edited_at, deleted_at
                 FROM messages
                 WHERE server_id = ?1 AND channel_id = ?2
                   AND (created_at > ?3 OR (created_at = ?3 AND id > ?4))
                 ORDER BY created_at ASC, id ASC
                 LIMIT 200",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(
                rusqlite::params![state_clone.server_id, channel_id, ts, row_id],
                |row| {
                    Ok(annex_channels::Message {
                        id: row.get(0)?,
                        server_id: row.get(1)?,
                        channel_id: row.get(2)?,
                        message_id: row.get(3)?,
                        sender_pseudonym: row.get(4)?,
                        content: row.get(5)?,
                        reply_to_message_id: row.get(6)?,
                        created_at: row.get(7)?,
                        expires_at: row.get(8)?,
                        edited_at: row.get(9)?,
                        deleted_at: row.get(10)?,
                    })
                },
            )
            .map_err(|e| e.to_string())?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row.map_err(|e| e.to_string())?);
        }
        Ok(messages)
    })
    .await;

    match res {
        Ok(Ok(messages)) => {
            let delivered_count = forward_resumed_messages(&tx_clone, messages);
            let ack = OutgoingMessage::Resumed {
                channel_id: channel_id_for_ack,
                missed_count: delivered_count,
            };
            if let Ok(json) = serde_json::to_string(&ack) {
                let _ = tx_clone.try_send(json);
            }
        }
        Ok(Err(e)) => {
            tracing::error!(pseudonym = %pseudonym_for_log, "resume failed: {}", e);
            send_ws_error(ctx.tx, format!("Resume failed: {e}"));
        }
        Err(e) => {
            tracing::error!(pseudonym = %pseudonym_for_log, "resume task failed: {}", e);
            send_ws_error(ctx.tx, "Resume failed: internal error".to_string());
        }
    }
}

/// Enqueues each resumed message onto the outbound mpsc and returns
/// the number of messages **actually delivered**. This is NOT
/// `messages.len()`: a slow consumer can fill the 256-deep
/// per-session queue, and from there `try_send` returns Err and we
/// stop forwarding. Reporting `delivered`, not `attempted`, in the
/// surrounding `Resumed` ack lets the client tell partial recovery
/// from a complete one and decide whether to retry from a smaller
/// last_message_id rather than advancing its pointer past the gap.
///
/// Extracted as a free function so the partial-delivery counting
/// can be exercised by a deterministic unit test without spinning up
/// an AppState / DB / membership fixture.
fn forward_resumed_messages(
    tx: &mpsc::Sender<String>,
    messages: Vec<annex_channels::Message>,
) -> usize {
    let mut delivered_count = 0usize;
    for msg in messages {
        let ws_payload: WsMessagePayload = msg.into();
        let out = OutgoingMessage::Message(ws_payload);
        let json = match serde_json::to_string(&out) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("failed to serialize resumed message: {}", e);
                continue;
            }
        };
        if tx.try_send(json).is_err() {
            break;
        }
        delivered_count += 1;
    }
    delivered_count
}

#[cfg(test)]
mod tests {
    use super::forward_resumed_messages;
    use annex_channels::Message;

    fn synth_message(id: i64, message_id: &str) -> Message {
        Message {
            id,
            server_id: 1,
            channel_id: "chan-1".to_string(),
            message_id: message_id.to_string(),
            sender_pseudonym: "psn-1".to_string(),
            content: format!("hello-{id}"),
            reply_to_message_id: None,
            created_at: format!("2026-05-12T00:00:0{id}Z"),
            expires_at: None,
            edited_at: None,
            deleted_at: None,
        }
    }

    /// Happy path: the outbound mpsc has capacity for every message,
    /// so `delivered == messages.len()`.
    #[tokio::test]
    async fn forwards_every_message_when_outbound_queue_has_capacity() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(10);
        let messages = (0..5)
            .map(|i| synth_message(i, &format!("m-{i}")))
            .collect();

        let delivered = forward_resumed_messages(&tx, messages);
        assert_eq!(delivered, 5);

        // Drain the queue to confirm every payload landed.
        let mut received = 0;
        while rx.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(received, 5);
    }

    /// Back-pressure path: the outbound mpsc has capacity 2; we hand
    /// the function 5 messages. The fix tracks ACTUAL enqueues, so
    /// `delivered == 2` (NOT 5). Pre-fix the surrounding code reported
    /// `messages.len() == 5` in the ack, which would tell the client
    /// to advance its last-seen pointer past 5 messages even though
    /// only 2 reached the outbound queue.
    #[tokio::test]
    async fn returns_actual_delivery_count_when_outbound_queue_fills() {
        // Capacity 2 means the queue fills after 2 successful sends.
        // The test deliberately doesn't drain — that simulates a slow
        // consumer that hasn't yet processed any frames.
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(2);
        let messages = (0..5)
            .map(|i| synth_message(i, &format!("m-{i}")))
            .collect();

        let delivered = forward_resumed_messages(&tx, messages);
        // Pre-fix this was 5 (the loop incremented even on Err); the
        // fix returns the count of *successful* try_send calls.
        assert_eq!(
            delivered, 2,
            "delivered_count must equal the outbound queue capacity, not messages.len()"
        );
    }

    /// Closed-channel path: the receiver dropped before we started
    /// forwarding. Every try_send returns Err immediately, so
    /// `delivered == 0`.
    #[tokio::test]
    async fn returns_zero_when_outbound_channel_is_closed() {
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(10);
        drop(rx); // Receiver gone — every try_send is Err(Closed).
        let messages = (0..3)
            .map(|i| synth_message(i, &format!("m-{i}")))
            .collect();

        let delivered = forward_resumed_messages(&tx, messages);
        assert_eq!(delivered, 0);
    }

    /// Empty input is the trivial zero-delivery case — confirms the
    /// loop doesn't underflow / panic on `messages.len() == 0`.
    #[tokio::test]
    async fn returns_zero_for_empty_input() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(10);
        let delivered = forward_resumed_messages(&tx, vec![]);
        assert_eq!(delivered, 0);
    }
}
