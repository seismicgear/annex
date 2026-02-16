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
            Err(_broadcast_error) => {
                // Lagged or closed. We can ignore lag for now or log it.
                // BroadcastStream handles Lagged by returning an error, which we filter out here.
                None
            }
        }
    });

    Sse::new(mapped_stream).keep_alive(axum::response::sse::KeepAlive::default())
}
