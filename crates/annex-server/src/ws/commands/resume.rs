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
            let count = messages.len();
            for msg in messages {
                let ws_payload: WsMessagePayload = msg.into();
                let out = OutgoingMessage::Message(ws_payload);
                if let Ok(json) = serde_json::to_string(&out) {
                    if tx_clone.try_send(json).is_err() {
                        break;
                    }
                }
            }
            let ack = OutgoingMessage::Resumed {
                channel_id: channel_id_for_ack,
                missed_count: count,
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
