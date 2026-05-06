//! RTX persistence: pure SQL helpers for the `rtx_*` tables (bundles,
//! subscriptions, transfer log) plus governance summary aggregates.
//!
//! Follows the same shape as `federation_repository`: each function takes
//! an `&Connection` (or `&Transaction`) and returns parsed rows. No
//! policy, no crypto, no networking lives here.
//!
//! The `log_rtx_transfer` helper used by both `RtxService::publish_bundle`
//! and `FederationService::receive_federated_rtx` is owned by
//! `federation_repository::log_rtx_transfer` and re-exported through this
//! module for ergonomics (`repo::log_rtx_transfer` reads identically from
//! either service's perspective).

use rusqlite::{
    params, params_from_iter, types::ToSql, Connection, OptionalExtension, Transaction,
};

pub(crate) use crate::services::federation_repository::log_rtx_transfer;

/// One row of `rtx_subscriptions` for a single subscriber.
#[derive(Debug, Clone)]
pub(crate) struct SubscriptionRow {
    pub domain_filters_json: String,
    pub accept_federated: bool,
    pub created_at: String,
}

/// One row of the joined subscriber-with-scope view used to fan out a
/// freshly published bundle to local consumers.
#[derive(Debug, Clone)]
pub(crate) struct LocalRtxSubscriber {
    pub pseudonym: String,
    pub domain_filters_json: String,
    pub transfer_scope_str: String,
}

/// Active sender context for the `publish_handler` gate: the agent's
/// declared transfer scope and capability contract JSON.
#[derive(Debug, Clone)]
pub(crate) struct AgentPublishContext {
    pub transfer_scope_str: String,
    pub capability_contract_json: String,
}

/// Filter set for `count_filtered_transfers` / `list_filtered_transfers`.
#[derive(Debug, Clone, Default)]
pub(crate) struct TransferLogFilter {
    pub bundle_id: Option<String>,
    pub source: Option<String>,
    pub destination: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

/// One row of `rtx_transfer_log` returned by the governance listing.
#[derive(Debug, Clone)]
pub(crate) struct TransferLogRow {
    pub id: i64,
    pub bundle_id: String,
    pub source_pseudonym: String,
    pub destination_pseudonym: Option<String>,
    pub transfer_scope_applied: String,
    pub redactions_applied: Option<String>,
    pub transferred_at: String,
}

/// Aggregate counters returned by the governance summary endpoint.
#[derive(Debug, Clone)]
pub(crate) struct GovernanceCounts {
    pub total_transfers: i64,
    pub unique_bundles: i64,
    pub unique_sources: i64,
    pub unique_destinations: i64,
    pub redacted_transfers: i64,
}

/// One `(scope, count)` pair from the governance summary's scope breakdown.
#[derive(Debug, Clone)]
pub(crate) struct ScopeCount {
    pub scope: String,
    pub count: i64,
}

/// Active federation peer for outbound RTX relay.
#[derive(Debug, Clone)]
pub(crate) struct FederationPeerRelayTarget {
    pub base_url: String,
    pub transfer_scope: String,
}

/// Look up a sender's active agent registration (transfer scope and
/// capability contract). Returns `None` when no active row matches.
pub(crate) fn agent_publish_context(
    conn: &Connection,
    server_id: i64,
    pseudonym: &str,
) -> Result<Option<AgentPublishContext>, rusqlite::Error> {
    conn.query_row(
        "SELECT transfer_scope, capability_contract_json
         FROM agent_registrations
         WHERE server_id = ?1 AND pseudonym_id = ?2 AND active = 1",
        params![server_id, pseudonym],
        |row| {
            Ok(AgentPublishContext {
                transfer_scope_str: row.get(0)?,
                capability_contract_json: row.get(1)?,
            })
        },
    )
    .optional()
}

/// Look up the active transfer scope string for an agent. Used by the
/// subscribe / unsubscribe paths to gate access to the RTX surface.
pub(crate) fn agent_active_transfer_scope(
    conn: &Connection,
    server_id: i64,
    pseudonym: &str,
) -> Result<Option<String>, rusqlite::Error> {
    conn.query_row(
        "SELECT transfer_scope FROM agent_registrations
         WHERE server_id = ?1 AND pseudonym_id = ?2 AND active = 1",
        params![server_id, pseudonym],
        |row| row.get(0),
    )
    .optional()
}

/// `INSERT INTO rtx_bundles` for a locally-published bundle (provenance
/// is inherited from the source server, so `provenance_json` is left as
/// the column default — matching the existing inline statement).
/// Returns `Err(rusqlite::Error::SqliteFailure(.., ConstraintViolation))`
/// on duplicate `bundle_id`; the caller maps this to a 409.
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_local_rtx_bundle(
    tx: &Transaction<'_>,
    server_id: i64,
    bundle_id: &str,
    source_pseudonym: &str,
    source_server: &str,
    domain_tags_json: &str,
    summary: &str,
    reasoning_chain: Option<&str>,
    caveats_json: &str,
    created_at_ms: i64,
    signature: &str,
    vrp_handshake_ref: &str,
) -> Result<usize, rusqlite::Error> {
    tx.execute(
        "INSERT INTO rtx_bundles (
            server_id, bundle_id, source_pseudonym, source_server,
            domain_tags_json, summary, reasoning_chain, caveats_json,
            created_at_ms, signature, vrp_handshake_ref
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            server_id,
            bundle_id,
            source_pseudonym,
            source_server,
            domain_tags_json,
            summary,
            reasoning_chain,
            caveats_json,
            created_at_ms,
            signature,
            vrp_handshake_ref,
        ],
    )
}

/// UPSERT for `rtx_subscriptions`. On conflict, refreshes the
/// `domain_filters_json` and `accept_federated` columns (preserves the
/// existing inline statement byte-for-byte).
pub(crate) fn upsert_subscription(
    conn: &Connection,
    server_id: i64,
    pseudonym: &str,
    domain_filters_json: &str,
    accept_federated: bool,
) -> Result<usize, rusqlite::Error> {
    let accept_fed_int: i32 = if accept_federated { 1 } else { 0 };
    conn.execute(
        "INSERT INTO rtx_subscriptions (
            server_id, subscriber_pseudonym, domain_filters_json, accept_federated
        ) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(server_id, subscriber_pseudonym) DO UPDATE SET
            domain_filters_json = excluded.domain_filters_json,
            accept_federated = excluded.accept_federated",
        params![server_id, pseudonym, domain_filters_json, accept_fed_int],
    )
}

/// Read back a single subscription row for `(server_id, pseudonym)`.
pub(crate) fn read_subscription(
    conn: &Connection,
    server_id: i64,
    pseudonym: &str,
) -> Result<Option<SubscriptionRow>, rusqlite::Error> {
    conn.query_row(
        "SELECT domain_filters_json, accept_federated, created_at
         FROM rtx_subscriptions
         WHERE server_id = ?1 AND subscriber_pseudonym = ?2",
        params![server_id, pseudonym],
        |row| {
            Ok(SubscriptionRow {
                domain_filters_json: row.get(0)?,
                accept_federated: row.get(1)?,
                created_at: row.get(2)?,
            })
        },
    )
    .optional()
}

/// Delete the subscription row for a pseudonym. Returns the number of
/// rows actually removed.
pub(crate) fn delete_subscription(
    conn: &Connection,
    server_id: i64,
    pseudonym: &str,
) -> Result<usize, rusqlite::Error> {
    conn.execute(
        "DELETE FROM rtx_subscriptions
         WHERE server_id = ?1 AND subscriber_pseudonym = ?2",
        params![server_id, pseudonym],
    )
}

/// List local RTX subscribers for fan-out. Excludes the sender so they
/// do not receive their own publish.
pub(crate) fn list_local_rtx_subscribers(
    conn: &Connection,
    server_id: i64,
    exclude_pseudonym: &str,
) -> Result<Vec<LocalRtxSubscriber>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT s.subscriber_pseudonym, s.domain_filters_json, a.transfer_scope
         FROM rtx_subscriptions s
         JOIN agent_registrations a
           ON a.server_id = s.server_id AND a.pseudonym_id = s.subscriber_pseudonym
         WHERE s.server_id = ?1 AND a.active = 1
           AND s.subscriber_pseudonym != ?2",
    )?;

    let rows = stmt.query_map(params![server_id, exclude_pseudonym], |row| {
        Ok(LocalRtxSubscriber {
            pseudonym: row.get::<_, String>(0)?,
            domain_filters_json: row.get::<_, String>(1)?,
            transfer_scope_str: row.get::<_, String>(2)?,
        })
    })?;

    let mut collected = Vec::new();
    for row in rows {
        collected.push(row?);
    }
    Ok(collected)
}

/// Active federation peers (`base_url`, `transfer_scope`) for outbound
/// RTX relay. Mirrors `federation_repository::list_active_peers` —
/// duplicated here because the SQL is owned by both relay paths and
/// changing one without the other would silently break a fan-out.
pub(crate) fn list_active_federation_peers(
    conn: &Connection,
    server_id: i64,
) -> Result<Vec<FederationPeerRelayTarget>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT i.base_url, fa.transfer_scope
         FROM federation_agreements fa
         JOIN instances i ON fa.remote_instance_id = i.id
         WHERE fa.local_server_id = ?1 AND fa.active = 1 AND i.status = 'ACTIVE'",
    )?;

    let rows = stmt.query_map(params![server_id], |row| {
        Ok(FederationPeerRelayTarget {
            base_url: row.get::<_, String>(0)?,
            transfer_scope: row.get::<_, String>(1)?,
        })
    })?;

    let mut peers = Vec::new();
    for row in rows {
        peers.push(row?);
    }
    Ok(peers)
}

/// Build the dynamic `WHERE` clause shared by `count_filtered_transfers`
/// and `list_filtered_transfers`. Returns `(sql_fragment, params, next_idx)`.
fn build_filter_clause(
    server_id: i64,
    filter: &TransferLogFilter,
) -> (String, Vec<Box<dyn ToSql>>, u32) {
    let mut conditions = vec!["server_id = ?1".to_string()];
    let mut params: Vec<Box<dyn ToSql>> = vec![Box::new(server_id)];
    let mut idx = 2u32;

    if let Some(ref bid) = filter.bundle_id {
        conditions.push(format!("bundle_id = ?{idx}"));
        params.push(Box::new(bid.clone()));
        idx += 1;
    }
    if let Some(ref src) = filter.source {
        conditions.push(format!("source_pseudonym = ?{idx}"));
        params.push(Box::new(src.clone()));
        idx += 1;
    }
    if let Some(ref dst) = filter.destination {
        conditions.push(format!("destination_pseudonym = ?{idx}"));
        params.push(Box::new(dst.clone()));
        idx += 1;
    }
    if let Some(ref since) = filter.since {
        conditions.push(format!("transferred_at >= ?{idx}"));
        params.push(Box::new(since.clone()));
        idx += 1;
    }
    if let Some(ref until) = filter.until {
        conditions.push(format!("transferred_at <= ?{idx}"));
        params.push(Box::new(until.clone()));
        idx += 1;
    }

    (conditions.join(" AND "), params, idx)
}

/// Count rows of `rtx_transfer_log` matching the filter.
pub(crate) fn count_filtered_transfers(
    conn: &Connection,
    server_id: i64,
    filter: &TransferLogFilter,
) -> Result<i64, rusqlite::Error> {
    let (where_clause, params, _next) = build_filter_clause(server_id, filter);
    let sql = format!("SELECT COUNT(*) FROM rtx_transfer_log WHERE {where_clause}");
    conn.query_row(
        &sql,
        params_from_iter(params.iter().map(|p| p.as_ref())),
        |row| row.get(0),
    )
}

/// List rows of `rtx_transfer_log` matching the filter, ordered by `id DESC`,
/// with `LIMIT` / `OFFSET` from the filter.
pub(crate) fn list_filtered_transfers(
    conn: &Connection,
    server_id: i64,
    filter: &TransferLogFilter,
) -> Result<Vec<TransferLogRow>, rusqlite::Error> {
    let (where_clause, mut params, mut idx) = build_filter_clause(server_id, filter);

    let sql = format!(
        "SELECT id, bundle_id, source_pseudonym, destination_pseudonym,
                transfer_scope_applied, redactions_applied, transferred_at
         FROM rtx_transfer_log
         WHERE {}
         ORDER BY id DESC
         LIMIT ?{} OFFSET ?{}",
        where_clause,
        idx,
        idx + 1,
    );

    params.push(Box::new(filter.limit));
    params.push(Box::new(filter.offset));
    idx += 2;
    let _ = idx; // silence unused-final-increment warning

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params.iter().map(|p| p.as_ref())), |row| {
        Ok(TransferLogRow {
            id: row.get(0)?,
            bundle_id: row.get(1)?,
            source_pseudonym: row.get(2)?,
            destination_pseudonym: row.get(3)?,
            transfer_scope_applied: row.get(4)?,
            redactions_applied: row.get(5)?,
            transferred_at: row.get(6)?,
        })
    })?;

    let mut transfers = Vec::new();
    for row in rows {
        transfers.push(row?);
    }
    Ok(transfers)
}

/// Aggregate counters used by the governance summary endpoint. Each
/// query is byte-identical to the previous inline implementation.
pub(crate) fn governance_counts(
    conn: &Connection,
    server_id: i64,
) -> Result<GovernanceCounts, rusqlite::Error> {
    let total_transfers: i64 = conn.query_row(
        "SELECT COUNT(*) FROM rtx_transfer_log WHERE server_id = ?1",
        params![server_id],
        |row| row.get(0),
    )?;

    let unique_bundles: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT bundle_id) FROM rtx_transfer_log WHERE server_id = ?1",
        params![server_id],
        |row| row.get(0),
    )?;

    let unique_sources: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT source_pseudonym) FROM rtx_transfer_log WHERE server_id = ?1",
        params![server_id],
        |row| row.get(0),
    )?;

    let unique_destinations: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT destination_pseudonym) FROM rtx_transfer_log
         WHERE server_id = ?1 AND destination_pseudonym IS NOT NULL",
        params![server_id],
        |row| row.get(0),
    )?;

    let redacted_transfers: i64 = conn.query_row(
        "SELECT COUNT(*) FROM rtx_transfer_log
         WHERE server_id = ?1 AND redactions_applied IS NOT NULL",
        params![server_id],
        |row| row.get(0),
    )?;

    Ok(GovernanceCounts {
        total_transfers,
        unique_bundles,
        unique_sources,
        unique_destinations,
        redacted_transfers,
    })
}

/// Per-scope transfer counts for the governance summary's scope
/// breakdown, ordered by descending count.
pub(crate) fn governance_scope_breakdown(
    conn: &Connection,
    server_id: i64,
) -> Result<Vec<ScopeCount>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT transfer_scope_applied, COUNT(*) as cnt
         FROM rtx_transfer_log
         WHERE server_id = ?1
         GROUP BY transfer_scope_applied
         ORDER BY cnt DESC",
    )?;

    let rows = stmt.query_map(params![server_id], |row| {
        Ok(ScopeCount {
            scope: row.get(0)?,
            count: row.get(1)?,
        })
    })?;

    let mut by_scope = Vec::new();
    for row in rows {
        by_scope.push(row?);
    }
    Ok(by_scope)
}
