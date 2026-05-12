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

    // Hash-chain the event log so a tampered row is detectable when
    // the log is mirrored or exported. The chain links each event to
    // its predecessor on the same server via `prev_hash`. The first
    // event's `prev_hash` is the literal "GENESIS".
    //
    // We compute prev_hash from the last-known event on the same
    // server, then INSERT under the UNIQUE(server_id, seq)
    // constraint. If a concurrent writer raced us on seq, the unique
    // constraint surfaces it as an error rather than silently
    // producing a duplicate seq with a divergent chain.
    let prev_hash: String = conn
        .query_row(
            "SELECT event_hash FROM public_event_log \
             WHERE server_id = ?1 \
             ORDER BY seq DESC LIMIT 1",
            params![server_id],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .unwrap_or_else(|| "GENESIS".to_string());

    // Pre-compute occurred_at so it goes into the hash. SQLite's
    // datetime('now') in the INSERT can't be fed back into the hash
    // input without a second statement, and the integrity claim
    // needs occurred_at to be hashed alongside everything else.
    let occurred_at = current_iso_timestamp();

    // Compute the next seq deterministically. The UNIQUE constraint
    // will catch a race; the previous "subquery inside INSERT" trick
    // is no longer needed because we need the seq value to compute
    // the hash.
    let seq = next_seq(conn, server_id)?;

    let event_hash = compute_event_hash(
        server_id,
        seq,
        domain.as_str(),
        event_type,
        entity_type,
        entity_id,
        &payload_json,
        &occurred_at,
        &prev_hash,
    );

    let id: i64 = conn.query_row(
        "INSERT INTO public_event_log
            (server_id, domain, event_type, entity_type, entity_id, seq,
             payload_json, occurred_at, prev_hash, event_hash, event_signature)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL)
         RETURNING id",
        params![
            server_id,
            domain.as_str(),
            event_type,
            entity_type,
            entity_id,
            seq,
            payload_json,
            occurred_at,
            prev_hash,
            event_hash,
        ],
        |row| row.get::<_, i64>(0),
    )?;

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

/// Canonical SHA-256 of an event's hashed-fields, lowercase hex.
/// Field order is fixed and newline-delimited so changing any field
/// changes the digest.
#[allow(clippy::too_many_arguments)]
pub fn compute_event_hash(
    server_id: i64,
    seq: i64,
    domain: &str,
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    payload_json: &str,
    occurred_at: &str,
    prev_hash: &str,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let canonical = format!(
        "{server_id}\n{seq}\n{domain}\n{event_type}\n{entity_type}\n{entity_id}\n{payload_json}\n{occurred_at}\n{prev_hash}"
    );
    hasher.update(canonical.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn current_iso_timestamp() -> String {
    // SQLite's datetime('now') returns `YYYY-MM-DD HH:MM:SS` UTC; we
    // mirror that format so existing readers don't parse-fail.
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let secs_in_day = 86_400i64;
    let day = now.div_euclid(secs_in_day);
    let rem = now.rem_euclid(secs_in_day);
    let (h, rem2) = (rem / 3600, rem % 3600);
    let (m, s) = (rem2 / 60, rem2 % 60);
    // Convert day-since-epoch to YYYY-MM-DD without pulling in
    // chrono — the hash only needs determinism, not calendar
    // arithmetic, but we still want a readable string.
    // Civil-from-days algorithm (Howard Hinnant).
    let z = day + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

/// Verify the hash chain for one server's event log. Returns the
/// index of the first row whose hash or prev_hash is inconsistent
/// with the chain, or `None` if the chain is intact.
///
/// Operators / federation peers consume this to assert "the log I am
/// looking at has not been tampered with." A `Some(seq)` return is
/// the cryptographic evidence; the operator can then narrow down
/// what happened around that sequence number.
pub fn verify_event_log_chain(
    conn: &Connection,
    server_id: i64,
) -> Result<Option<i64>, ObserveError> {
    let mut stmt = conn.prepare(
        "SELECT seq, domain, event_type, entity_type, entity_id, \
                payload_json, occurred_at, prev_hash, event_hash \
         FROM public_event_log \
         WHERE server_id = ?1 \
         ORDER BY seq ASC",
    )?;
    let mut rows = stmt.query(params![server_id])?;
    let mut expected_prev = "GENESIS".to_string();
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get(0)?;
        let domain: String = row.get(1)?;
        let event_type: String = row.get(2)?;
        let entity_type: String = row.get(3)?;
        let entity_id: String = row.get(4)?;
        let payload_json: String = row.get(5)?;
        let occurred_at: String = row.get(6)?;
        let prev_hash: String = row.get(7)?;
        let event_hash: String = row.get(8)?;

        if prev_hash != expected_prev {
            return Ok(Some(seq));
        }
        let recomputed = compute_event_hash(
            server_id,
            seq,
            &domain,
            &event_type,
            &entity_type,
            &entity_id,
            &payload_json,
            &occurred_at,
            &prev_hash,
        );
        if recomputed != event_hash {
            return Ok(Some(seq));
        }
        expected_prev = event_hash;
    }
    Ok(None)
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
