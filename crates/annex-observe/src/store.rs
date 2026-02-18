//! Persistence operations for the public event log.
//!
//! All writes go through [`emit_event`], which serialises the payload,
//! assigns a monotonically increasing sequence number, and inserts into
//! the `public_event_log` table in a single statement.
//!
//! Reads go through [`query_events`], which supports filtering by domain,
//! event type, entity, and time range with cursor-based pagination.

use rusqlite::{params, Connection};

use crate::error::ObserveError;
use crate::event::{EventDomain, EventPayload, PublicEvent};

/// Writes a single event to the public event log.
///
/// The caller supplies the domain, event type, entity type, entity ID,
/// and a structured payload. A monotonically increasing sequence number
/// is assigned automatically via [`next_seq`].
///
/// # Errors
///
/// Returns `ObserveError::Database` on SQL failure or
/// `ObserveError::Serialization` if the payload cannot be serialised.
pub fn emit_event(
    conn: &Connection,
    server_id: i64,
    domain: EventDomain,
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    payload: &EventPayload,
) -> Result<PublicEvent, ObserveError> {
    let payload_json = serde_json::to_string(payload)?;

    // Atomically assign sequence number and insert in a single statement.
    // The subquery computes COALESCE(MAX(seq), 0) + 1 within the same INSERT,
    // eliminating the read-modify-write race condition where two concurrent
    // writers could observe the same MAX(seq) and produce duplicate sequence
    // numbers.
    let row = conn.query_row(
        "INSERT INTO public_event_log
            (server_id, domain, event_type, entity_type, entity_id, seq, payload_json, occurred_at)
         VALUES (
            ?1, ?2, ?3, ?4, ?5,
            (SELECT COALESCE(MAX(seq), 0) + 1 FROM public_event_log WHERE server_id = ?1),
            ?6,
            datetime('now')
         )
         RETURNING id, seq, occurred_at",
        params![
            server_id,
            domain.as_str(),
            event_type,
            entity_type,
            entity_id,
            payload_json,
        ],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?)),
    )?;

    let (id, seq, occurred_at) = row;

    Ok(PublicEvent {
        id,
        server_id,
        domain: domain.as_str().to_string(),
        event_type: event_type.to_string(),
        entity_type: entity_type.to_string(),
        entity_id: entity_id.to_string(),
        seq,
        payload_json,
        occurred_at,
    })
}

/// Returns the next sequence number for the given server.
///
/// Sequence numbers are monotonically increasing per server and are used
/// for ordering events within a server's event stream.
///
/// # Errors
///
/// Returns `ObserveError::Database` on SQL failure.
pub fn next_seq(conn: &Connection, server_id: i64) -> Result<i64, ObserveError> {
    let max_seq: Option<i64> = conn.query_row(
        "SELECT MAX(seq) FROM public_event_log WHERE server_id = ?1",
        params![server_id],
        |row| row.get(0),
    )?;
    Ok(max_seq.unwrap_or(0) + 1)
}

/// Filter criteria for querying the public event log.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Filter by event domain.
    pub domain: Option<EventDomain>,
    /// Filter by event type string.
    pub event_type: Option<String>,
    /// Filter by entity type string.
    pub entity_type: Option<String>,
    /// Filter by entity ID.
    pub entity_id: Option<String>,
    /// Return events that occurred at or after this ISO 8601 timestamp.
    pub since: Option<String>,
    /// Maximum number of events to return (default: 100).
    pub limit: Option<i64>,
}

/// Queries the public event log with optional filters.
///
/// Results are returned in chronological order (oldest first), bounded by
/// `filter.limit` (default 100). Use `filter.since` for cursor-based
/// pagination.
///
/// # Errors
///
/// Returns `ObserveError::Database` on SQL failure.
pub fn query_events(
    conn: &Connection,
    server_id: i64,
    filter: &EventFilter,
) -> Result<Vec<PublicEvent>, ObserveError> {
    // Build a parameterised query dynamically.  We collect WHERE clauses
    // and bind parameters separately so nothing is interpolated.
    let mut clauses = vec!["server_id = ?1".to_string()];
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(server_id)];
    let mut idx = 2u32;

    if let Some(ref domain) = filter.domain {
        clauses.push(format!("domain = ?{idx}"));
        param_values.push(Box::new(domain.as_str().to_string()));
        idx += 1;
    }

    if let Some(ref et) = filter.event_type {
        clauses.push(format!("event_type = ?{idx}"));
        param_values.push(Box::new(et.clone()));
        idx += 1;
    }

    if let Some(ref ent_type) = filter.entity_type {
        clauses.push(format!("entity_type = ?{idx}"));
        param_values.push(Box::new(ent_type.clone()));
        idx += 1;
    }

    if let Some(ref ent_id) = filter.entity_id {
        clauses.push(format!("entity_id = ?{idx}"));
        param_values.push(Box::new(ent_id.clone()));
        idx += 1;
    }

    if let Some(ref since) = filter.since {
        clauses.push(format!("occurred_at >= ?{idx}"));
        param_values.push(Box::new(since.clone()));
        idx += 1;
    }

    let limit = filter.limit.unwrap_or(100);
    let where_clause = clauses.join(" AND ");
    let sql = format!(
        "SELECT id, server_id, domain, event_type, entity_type, entity_id, seq, payload_json, occurred_at
         FROM public_event_log
         WHERE {where_clause}
         ORDER BY seq ASC
         LIMIT ?{idx}"
    );

    param_values.push(Box::new(limit));

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| &**p).collect();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        Ok(PublicEvent {
            id: row.get(0)?,
            server_id: row.get(1)?,
            domain: row.get(2)?,
            event_type: row.get(3)?,
            entity_type: row.get(4)?,
            entity_id: row.get(5)?,
            seq: row.get(6)?,
            payload_json: row.get(7)?,
            occurred_at: row.get(8)?,
        })
    })?;

    let mut events = Vec::new();
    for row in rows {
        events.push(row?);
    }

    Ok(events)
}
