//! Public event and summary API handlers for the observability layer.
//!
//! Provides:
//! - `GET /api/public/events` — paginated event retrieval with filtering
//! - `GET /events/stream` — SSE real-time stream of observe events
//! - `GET /api/public/server/summary` — server metadata and aggregate counts
//! - `GET /api/public/federation/peers` — federation peer list with alignment
//! - `GET /api/public/agents` — active agent list with alignment and capabilities

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
use rusqlite::params;
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
        Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(count)) => {
            tracing::warn!(
                missed_events = count,
                "observe SSE stream lagged; {} events were dropped for this subscriber",
                count
            );
            // Send a sentinel event so the client knows it missed events
            // and can take corrective action (e.g., re-fetch from cursor).
            let sentinel = serde_json::json!({
                "type": "lagged",
                "missed_events": count
            });
            match serde_json::to_string(&sentinel) {
                Ok(data) => Some(Ok(Event::default().event("lagged").data(data))),
                Err(_) => None,
            }
        }
    });

    Sse::new(mapped_stream).keep_alive(KeepAlive::default())
}

// ── Server Summary ──────────────────────────────────────────────────

/// Response for `GET /api/public/server/summary`.
#[derive(Debug, Serialize)]
pub struct ServerSummaryResponse {
    /// The server's slug identifier.
    pub slug: String,
    /// The server's display label.
    pub label: String,
    /// Member counts by node type (e.g., `{"Human": 5, "AiAgent": 3}`).
    pub members_by_type: serde_json::Value,
    /// Total number of active members.
    pub total_active_members: i64,
    /// Total number of channels.
    pub channel_count: i64,
    /// Number of active federation peers.
    pub federation_peer_count: i64,
    /// Number of active agents.
    pub active_agent_count: i64,
}

/// Handler for `GET /api/public/server/summary`.
///
/// Returns aggregate metadata about the server: member counts by type,
/// channel count, federation peer count, and active agent count.
pub async fn get_server_summary_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<ServerSummaryResponse>, Response> {
    let pool = state.pool.clone();
    let server_id = state.server_id;

    let summary = tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;

        // Server metadata
        let (slug, label): (String, String) = conn
            .query_row(
                "SELECT slug, label FROM servers WHERE id = ?1",
                params![server_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| format!("failed to query server: {}", e))?;

        // Member counts by node type
        let mut members_map = serde_json::Map::new();
        let mut total_active: i64 = 0;
        {
            let mut stmt = conn
                .prepare(
                    "SELECT node_type, COUNT(*) FROM graph_nodes
                     WHERE server_id = ?1 AND active = 1
                     GROUP BY node_type",
                )
                .map_err(|e| format!("failed to prepare node query: {}", e))?;
            let rows = stmt
                .query_map(params![server_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|e| format!("failed to query nodes: {}", e))?;
            for row in rows {
                let (node_type, count) = row.map_err(|e| format!("row error: {}", e))?;
                total_active += count;
                members_map.insert(node_type, serde_json::Value::Number(count.into()));
            }
        }

        // Channel count
        let channel_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM channels WHERE server_id = ?1",
                params![server_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("failed to count channels: {}", e))?;

        // Federation peer count
        let federation_peer_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM federation_agreements
                 WHERE local_server_id = ?1 AND active = 1",
                params![server_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("failed to count federation peers: {}", e))?;

        // Active agent count
        let active_agent_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_registrations
                 WHERE server_id = ?1 AND active = 1",
                params![server_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("failed to count agents: {}", e))?;

        Ok::<_, String>(ServerSummaryResponse {
            slug,
            label,
            members_by_type: serde_json::Value::Object(members_map),
            total_active_members: total_active,
            channel_count,
            federation_peer_count,
            active_agent_count,
        })
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

    Ok(Json(summary))
}

// ── Federation Peers ────────────────────────────────────────────────

/// A single federation peer in the public summary.
#[derive(Debug, Serialize)]
pub struct FederationPeerEntry {
    /// The base URL of the remote instance.
    pub base_url: String,
    /// The display label of the remote instance.
    pub label: String,
    /// The alignment status (e.g., `Aligned`, `Partial`, `Conflict`).
    pub alignment_status: String,
    /// The negotiated transfer scope.
    pub transfer_scope: String,
    /// Whether the agreement is currently active.
    pub active: bool,
}

/// Response for `GET /api/public/federation/peers`.
#[derive(Debug, Serialize)]
pub struct FederationPeersResponse {
    /// List of federation peers.
    pub peers: Vec<FederationPeerEntry>,
    /// Total number of peers returned.
    pub count: usize,
}

/// Handler for `GET /api/public/federation/peers`.
///
/// Returns a list of federation peers with their alignment status and
/// transfer scope. Only active agreements are returned.
pub async fn get_federation_peers_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<FederationPeersResponse>, Response> {
    let pool = state.pool.clone();
    let server_id = state.server_id;

    let peers = tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare(
                "SELECT i.base_url, i.label, fa.alignment_status, fa.transfer_scope, fa.active
                 FROM federation_agreements fa
                 JOIN instances i ON fa.remote_instance_id = i.id
                 WHERE fa.local_server_id = ?1 AND fa.active = 1
                 ORDER BY i.label ASC",
            )
            .map_err(|e| format!("failed to prepare federation query: {}", e))?;

        let rows = stmt
            .query_map(params![server_id], |row| {
                Ok(FederationPeerEntry {
                    base_url: row.get(0)?,
                    label: row.get(1)?,
                    alignment_status: row.get(2)?,
                    transfer_scope: row.get(3)?,
                    active: row.get::<_, i64>(4)? == 1,
                })
            })
            .map_err(|e| format!("failed to query federation peers: {}", e))?;

        let mut peers = Vec::new();
        for row in rows {
            peers.push(row.map_err(|e| format!("row error: {}", e))?);
        }

        Ok::<_, String>(peers)
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

    let count = peers.len();
    Ok(Json(FederationPeersResponse { peers, count }))
}

// ── Active Agents ───────────────────────────────────────────────────

/// A single agent entry in the public summary.
#[derive(Debug, Serialize)]
pub struct AgentEntry {
    /// The agent's pseudonym identifier.
    pub pseudonym_id: String,
    /// The alignment status (e.g., `Aligned`, `Partial`).
    pub alignment_status: String,
    /// The negotiated transfer scope.
    pub transfer_scope: String,
    /// The agent's capability contract (parsed JSON).
    pub capability_contract: serde_json::Value,
    /// The agent's reputation score.
    pub reputation_score: f64,
}

/// Response for `GET /api/public/agents`.
#[derive(Debug, Serialize)]
pub struct AgentsResponse {
    /// List of active agents.
    pub agents: Vec<AgentEntry>,
    /// Total number of agents returned.
    pub count: usize,
}

/// Handler for `GET /api/public/agents`.
///
/// Returns a list of active agents with their alignment status,
/// capability summaries, and reputation scores.
pub async fn get_agents_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<AgentsResponse>, Response> {
    let pool = state.pool.clone();
    let server_id = state.server_id;

    let agents = tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| e.to_string())?;

        let mut stmt = conn
            .prepare(
                "SELECT pseudonym_id, alignment_status, transfer_scope,
                        capability_contract_json, reputation_score
                 FROM agent_registrations
                 WHERE server_id = ?1 AND active = 1
                 ORDER BY reputation_score DESC
                 LIMIT 1000",
            )
            .map_err(|e| format!("failed to prepare agents query: {}", e))?;

        let rows = stmt
            .query_map(params![server_id], |row| {
                let contract_json: String = row.get(3)?;
                let contract: serde_json::Value =
                    serde_json::from_str(&contract_json).unwrap_or_else(|e| {
                        tracing::warn!(
                            "corrupted capability_contract_json in agent listing, returning raw string: {}",
                            e
                        );
                        serde_json::Value::String(contract_json)
                    });
                Ok(AgentEntry {
                    pseudonym_id: row.get(0)?,
                    alignment_status: row.get(1)?,
                    transfer_scope: row.get(2)?,
                    capability_contract: contract,
                    reputation_score: row.get(4)?,
                })
            })
            .map_err(|e| format!("failed to query agents: {}", e))?;

        let mut agents = Vec::new();
        for row in rows {
            agents.push(row.map_err(|e| format!("row error: {}", e))?);
        }

        Ok::<_, String>(agents)
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

    let count = agents.len();
    Ok(Json(AgentsResponse { agents, count }))
}
