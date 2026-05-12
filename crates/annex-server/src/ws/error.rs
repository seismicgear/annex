//! Helpers for pushing protocol-framed error frames to a single WebSocket
//! sender.
//!
//! Both helpers serialise an [`OutgoingMessage::Error`] and best-effort
//! enqueue it on the per-session `mpsc::Sender<String>`. A failure to send
//! is treated as the consumer being closed or saturated and is logged at
//! warn level rather than propagated; callers do not get to fail because
//! the client dropped the connection.

use tokio::sync::mpsc;

use crate::ws::protocol::OutgoingMessage;

/// Sends a JSON-serialized error message over the WebSocket sender channel.
pub(crate) fn send_ws_error(tx: &mpsc::Sender<String>, message: String) {
    send_ws_error_with_id(tx, message, None);
}

/// Sends a JSON-serialized error message with an optional client request ID
/// so the frontend can correlate the error with the original send request.
pub(crate) fn send_ws_error_with_id(
    tx: &mpsc::Sender<String>,
    message: String,
    client_request_id: Option<String>,
) {
    match serde_json::to_string(&OutgoingMessage::Error {
        message,
        client_request_id,
    }) {
        Ok(json) => {
            if let Err(e) = tx.try_send(json) {
                tracing::warn!("failed to send WebSocket error to client: {}", e);
            }
        }
        Err(e) => {
            tracing::error!("failed to serialize WebSocket error message: {}", e);
        }
    }
}
