//! SSE presence stream handlers.

use crate::AppState;
use axum::{
    extract::Extension,
    response::{sse::Event, Sse},
};
use futures_util::Stream;
use std::{convert::Infallible, sync::Arc};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

/// Handler for `GET /events/presence`.
///
/// Streams real-time presence events (node added, updated, pruned, edge changes).
pub async fn get_presence_stream_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.presence_tx.subscribe();
    let stream = BroadcastStream::new(rx);

    let mapped_stream = stream.filter_map(|result| {
        match result {
            Ok(event) => {
                // Serialize event to JSON
                match serde_json::to_string(&event) {
                    Ok(data) => Some(Ok(Event::default().data(data))),
                    Err(e) => {
                        tracing::error!("failed to serialize presence event: {}", e);
                        None
                    }
                }
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(count)) => {
                tracing::warn!(
                    missed_events = count,
                    "presence SSE stream lagged; {} events were dropped for this subscriber",
                    count
                );
                // Send a sentinel event so the client knows it missed events
                // and can take corrective action (e.g., re-fetch full state).
                let sentinel = serde_json::json!({
                    "type": "lagged",
                    "missed_events": count
                });
                match serde_json::to_string(&sentinel) {
                    Ok(data) => Some(Ok(Event::default().event("lagged").data(data))),
                    Err(_) => None,
                }
            }
        }
    });

    Sse::new(mapped_stream).keep_alive(axum::response::sse::KeepAlive::default())
}
