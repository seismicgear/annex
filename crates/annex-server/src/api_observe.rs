//! Public event API handlers for the observability layer.
//!
//! Provides:
//! - `GET /api/public/events` — paginated event retrieval with filtering
//! - `GET /events/stream` — SSE real-time stream of observe events

use crate::AppState;
use annex_observe::{query_events, EventDomain, EventFilter, PublicEvent};
use axum::{
    extract::{Extension, Query},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    Json,
};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, sync::Arc};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

/// Query parameters for `GET /api/public/events`.
#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    /// Filter by event domain (e.g., `IDENTITY`, `PRESENCE`).
    pub domain: Option<String>,
    /// Filter by event type (e.g., `IDENTITY_REGISTERED`).
    pub event_type: Option<String>,
    /// Filter by entity type (e.g., `identity`, `node`).
    pub entity_type: Option<String>,
    /// Filter by entity ID.
    pub entity_id: Option<String>,
    /// Return events that occurred at or after this ISO 8601 timestamp.
    pub since: Option<String>,
    /// Maximum number of events to return (default: 100, max: 1000).
    pub limit: Option<i64>,
}

/// Response wrapper for paginated event retrieval.
#[derive(Debug, Serialize)]
pub struct EventsResponse {
    /// The matching events in chronological order.
    pub events: Vec<PublicEvent>,
    /// The number of events returned.
    pub count: usize,
}

/// Handler for `GET /api/public/events`.
///
/// Returns paginated, filterable events from the public event log.
/// This is an unauthenticated endpoint — the event log is public by design.
pub async fn get_events_handler(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<EventsQuery>,
) -> Result<Json<EventsResponse>, Response> {
    let pool = state.pool.clone();
    let server_id = state.server_id;

    let domain = match &params.domain {
        Some(d) => {
            let parsed: EventDomain = d.parse().map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": format!("invalid domain: {}. Expected one of: IDENTITY, PRESENCE, FEDERATION, AGENT, MODERATION", d)
                    })),
                )
                    .into_response()
            })?;
            Some(parsed)
        }
        None => None,
    };

    let limit = params.limit.unwrap_or(100).clamp(1, 1000);

    let filter = EventFilter {
        domain,
        event_type: params.event_type,
        entity_type: params.entity_type,
        entity_id: params.entity_id,
        since: params.since,
        limit: Some(limit),
    };

    let events = tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;
        query_events(&conn, server_id, &filter).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("task join error: {}", e) })),
        )
            .into_response()
    })?
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response()
    })?;

    let count = events.len();
    Ok(Json(EventsResponse { events, count }))
}

/// Query parameters for `GET /events/stream`.
#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    /// Filter by event domain (e.g., `IDENTITY`, `PRESENCE`).
    pub domain: Option<String>,
}

/// Handler for `GET /events/stream`.
///
/// Streams real-time observe events via SSE. Optionally filtered by domain.
/// This is an unauthenticated endpoint — the event log is public by design.
pub async fn get_event_stream_handler(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<StreamQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let domain_filter: Option<EventDomain> = params.domain.as_deref().and_then(|d| d.parse().ok());

    let rx = state.observe_tx.subscribe();
    let stream = BroadcastStream::new(rx);

    let mapped_stream = stream.filter_map(move |result| match result {
        Ok(event) => {
            // Apply domain filter if specified
            if let Some(ref filter_domain) = domain_filter {
                if event.domain != filter_domain.as_str() {
                    return None;
                }
            }

            match serde_json::to_string(&event) {
                Ok(data) => Some(Ok(Event::default().data(data))),
                Err(e) => {
                    tracing::error!("failed to serialize observe event: {}", e);
                    None
                }
            }
        }
        Err(_broadcast_error) => None,
    });

    Sse::new(mapped_stream).keep_alive(KeepAlive::default())
}
