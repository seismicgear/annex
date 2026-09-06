//! Background tasks for the Annex server.
//!
//! Includes:
//! - Pruning inactive graph nodes.
//! - Periodic rate limiter cleanup.

use crate::middleware::RateLimiter;
use crate::AppState;
use annex_graph::prune_inactive_nodes;
use annex_observe::EventPayload;
use annex_types::PresenceEvent;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

/// Starts the graph node pruning task.
///
/// This task runs indefinitely, periodically checking for inactive nodes
/// and pruning them (setting `active = 0`). Pruned nodes emit a `NodePruned` event.
pub async fn start_pruning_task(state: Arc<AppState>, threshold_seconds: u64) {
    if threshold_seconds == 0 {
        tracing::warn!("pruning task disabled (threshold=0)");
        return;
    }

    // Run check every 60 seconds or threshold/2, whichever is smaller (but min 1s)
    let interval_seconds = (threshold_seconds / 2).clamp(1, 60);
    let interval = Duration::from_secs(interval_seconds);

    tracing::info!(
        threshold_seconds,
        interval_seconds,
        "starting graph pruning task"
    );

    loop {
        sleep(interval).await;

        let pool = state.pool.clone();
        let server_id = state.server_id;
        let tx = state.presence_tx.clone();
        let observe_tx = state.observe_tx.clone();
        let signing_key = state.signing_key.clone();

        let res = tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| e.to_string())?;
            let pruned = prune_inactive_nodes(&conn, server_id, threshold_seconds)
                .map_err(|e| e.to_string())?;

            // Write pruned events to the persistent audit log
            for pseudonym_id in &pruned {
                let observe_payload = EventPayload::NodePruned {
                    pseudonym_id: pseudonym_id.clone(),
                };
                crate::emit_and_broadcast(
                    &conn,
                    server_id,
                    pseudonym_id,
                    &observe_payload,
                    &observe_tx,
                    &signing_key,
                );
            }

            Ok::<_, String>(pruned)
        })
        .await;

        match res {
            Ok(Ok(pruned_list)) => {
                if !pruned_list.is_empty() {
                    tracing::info!(count = pruned_list.len(), "pruned inactive graph nodes");
                    for pseudonym_id in pruned_list {
                        let _ = tx.send(PresenceEvent::NodePruned { pseudonym_id });
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::error!("failed to prune graph nodes: {}", e);
            }
            Err(e) => {
                tracing::error!("pruning task join error: {}", e);
            }
        }
    }
}

/// Periodically evicts expired entries from the in-memory rate limiter.
///
/// This prevents unbounded memory growth from many unique IPs/pseudonyms
/// sending requests. Runs every 120 seconds.
pub async fn start_rate_limit_cleanup_task(rate_limiter: RateLimiter) {
    let interval = Duration::from_secs(120);
    tracing::info!("starting rate limiter cleanup task (every 120s)");

    loop {
        sleep(interval).await;
        rate_limiter.cleanup_expired();
    }
}

/// Drains pending rows from the `federation_outbox` table, posts the
/// signed envelope to the target peer, and records the result.
///
/// Backoff: bounded exponential, capped at one hour. The schedule is
///   delay(attempts) = min(60 * 2^attempts, 3600) seconds.
/// At the default `outbox_max_attempts = 12` the total window is
/// roughly 3 hours, after which the row moves to `status='failed'`.
///
/// The receiver side is idempotent on the receipt ledger introduced
/// in migration 036 — retries are safe.
pub async fn start_federation_outbox_task(state: Arc<AppState>) {
    let interval = Duration::from_secs(state.federation_config.outbox_interval_seconds);
    let max_attempts = state.federation_config.outbox_max_attempts;
    tracing::info!(
        interval_seconds = state.federation_config.outbox_interval_seconds,
        max_attempts,
        "starting federation outbox task"
    );
    // Say it out loud, at the one place an operator is already looking when
    // federation is not delivering. The check this relaxes is also the one
    // that silently drops every row to a LAN or VPN peer, so the presence of
    // this line confirms the relaxation took effect, and its absence is the
    // first thing to check when a private-network peer receives nothing.
    if state.federation_config.allow_private_peer_addresses {
        tracing::warn!(
            "ANNEX_FEDERATION_ALLOW_PRIVATE_PEERS is enabled: federation peers at private, \
             loopback and link-local addresses are permitted. Intended for LAN, container and \
             VPN topologies. Peer URLs are operator-provisioned, but this does remove a \
             defence-in-depth check against an `instances` row edited to point at an internal \
             service."
        );
    }

    loop {
        sleep(interval).await;

        // The worker fetches a small batch under spawn_blocking, then
        // POSTs each envelope back on the async side. We avoid holding
        // a DB connection across `.await`.
        let batch_result = drain_outbox_batch(state.clone(), 32).await;
        match batch_result {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("federation outbox batch failed: {}", e);
            }
        }

        // Move terminally-failed rows out of pending after each batch
        // so the operator sees an accurate count via the admin
        // endpoint. We do this best-effort; missing it just means the
        // next loop catches it.
        let pool = state.pool.clone();
        let _ = tokio::task::spawn_blocking(move || -> rusqlite::Result<()> {
            let conn = pool.get().map_err(|e| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                    Some(format!("pool: {e}")),
                )
            })?;
            conn.execute(
                "UPDATE federation_outbox SET status = 'failed', updated_at = datetime('now') \
                 WHERE status = 'pending' AND attempts >= ?1",
                rusqlite::params![max_attempts],
            )?;
            Ok(())
        })
        .await;
    }
}

/// Drains up to `batch_size` pending outbox rows.
///
/// Exposed as `pub` (not `pub(crate)`) so integration tests in
/// `tests/` can exercise the dequeue-time SSRF gate (introduced in
/// [F33]) end-to-end against a real SQLite-backed `AppState` without
/// having to wait for the once-per-`outbox_interval_seconds` cadence
/// of [`start_federation_outbox_task`].
/// Resolves the receiver endpoint path for one serialized outbox
/// envelope.
///
/// The federation outbox stores both message envelopes and redaction
/// tombstones (ADR-0011) in the same table. Redaction envelopes carry
/// an `envelopeKind: "redaction"` discriminator in their JSON; message
/// envelopes have no such field. A cheap targeted deserialization of
/// just that field routes the row without parsing the full envelope.
/// Unparseable JSON falls through to the messages endpoint, where the
/// receiver's full deserialization produces the proper error and the
/// row ages out through the normal retry/backoff path.
fn outbox_envelope_endpoint_path(envelope_json: &str) -> &'static str {
    #[derive(serde::Deserialize)]
    struct KindPeek {
        #[serde(rename = "envelopeKind")]
        envelope_kind: Option<String>,
    }
    match serde_json::from_str::<KindPeek>(envelope_json) {
        Ok(KindPeek {
            envelope_kind: Some(kind),
        }) if kind == annex_federation::FEDERATED_ENVELOPE_KIND_REDACTION => {
            "/api/federation/redactions"
        }
        Ok(KindPeek {
            envelope_kind: Some(kind),
        }) if kind == annex_federation::FEDERATED_ENVELOPE_KIND_EDIT => "/api/federation/edits",
        _ => "/api/federation/messages",
    }
}

pub async fn drain_outbox_batch(state: Arc<AppState>, batch_size: i64) -> Result<(), String> {
    // 1. Pull a batch of pending rows whose retry time has passed.
    //
    //    Fairness gate (ADR-0008 amendment): the inner window function
    //    ranks each peer's due rows by retry time and the outer filter
    //    keeps at most `outbox_per_peer_batch` per peer. Without this,
    //    one unreachable peer with a deep backlog of due rows fills the
    //    whole batch every tick (the global ORDER BY next_retry_at
    //    favours its oldest rows), starving healthy peers and burning
    //    the tick's entire HTTP fan-out against a host that is down.
    //    The per-row exponential backoff bounds each row's retry rate;
    //    this bounds each PEER's share of a tick.
    let pool = state.pool.clone();
    let per_peer_cap = i64::from(state.federation_config.outbox_per_peer_batch.max(1));
    let rows =
        tokio::task::spawn_blocking(move || -> Result<Vec<(i64, i64, String, u32)>, String> {
            let conn = pool.get().map_err(|e| format!("pool: {e}"))?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, peer_instance_id, envelope_json, attempts FROM (\
                         SELECT id, peer_instance_id, envelope_json, attempts, next_retry_at, \
                                ROW_NUMBER() OVER (\
                                    PARTITION BY peer_instance_id \
                                    ORDER BY next_retry_at ASC, id ASC\
                                ) AS peer_rank \
                         FROM federation_outbox \
                         WHERE status = 'pending' AND next_retry_at <= datetime('now')\
                     ) WHERE peer_rank <= ?1 \
                     ORDER BY next_retry_at ASC LIMIT ?2",
                )
                .map_err(|e| format!("prepare: {e}"))?;
            let it = stmt
                .query_map(rusqlite::params![per_peer_cap, batch_size], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, u32>(3)?,
                    ))
                })
                .map_err(|e| format!("query: {e}"))?;
            let mut out = Vec::new();
            for r in it {
                out.push(r.map_err(|e| format!("row: {e}"))?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| format!("join: {e}"))??;

    if rows.is_empty() {
        return Ok(());
    }

    // 2. For each row, resolve the peer base_url and POST. This runs
    //    in parallel — small fan-out, bounded by batch_size.
    let pool_for_resolve = state.pool.clone();
    let peer_urls: std::collections::HashMap<i64, String> =
        tokio::task::spawn_blocking(move || -> std::collections::HashMap<i64, String> {
            let conn = match pool_for_resolve.get() {
                Ok(c) => c,
                Err(_) => return Default::default(),
            };
            let mut map = std::collections::HashMap::new();
            let mut stmt =
                match conn.prepare("SELECT id, base_url FROM instances WHERE status = 'ACTIVE'") {
                    Ok(s) => s,
                    Err(_) => return map,
                };
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            }) {
                for r in rows.flatten() {
                    map.insert(r.0, r.1);
                }
            }
            map
        })
        .await
        .map_err(|e| format!("join resolve: {e}"))?;

    let client = match crate::services::federation_service::federation_http_client() {
        Ok(c) => c,
        Err(e) => return Err(format!("http client: {e}")),
    };

    let mut handles = Vec::new();
    for (id, peer_id, envelope_json, attempts) in rows {
        let peer_base = match peer_urls.get(&peer_id).cloned() {
            Some(u) => u,
            None => {
                let pool = state.pool.clone();
                tokio::task::spawn_blocking(move || {
                    let conn = pool.get().ok();
                    if let Some(c) = conn {
                        if let Err(e) = c.execute(
                            "UPDATE federation_outbox SET attempts = attempts + 1, \
                             last_error = 'unknown peer', \
                             next_retry_at = datetime('now', '+1 hour'), \
                             updated_at = datetime('now') WHERE id = ?1",
                            rusqlite::params![id],
                        ) {
                            tracing::error!(
                                row = id,
                                "outbox: could not record unknown-peer backoff: {e}"
                            );
                        }
                    }
                });
                continue;
            }
        };

        // Defence-in-depth SSRF gate at dequeue time.
        //
        // `relay_message` already applies `is_peer_url_disallowed`
        // at ENQUEUE time, so a row should never be written for a
        // private/loopback/link-local peer. But the outbox is durable
        // across restarts and the `instances` table is admin-editable,
        // so an enqueued row may be paired with a peer whose base_url
        // has since been changed to a private host (operator error or
        // compromised admin account). Re-check here so the worker
        // can never POST a signed federation envelope to an internal
        // service. We mark the row as terminally failed — re-enqueue
        // requires a new message_id, which itself goes through the
        // enqueue-time SSRF gate.
        if crate::api_link_preview::is_peer_url_disallowed(
            &peer_base,
            state.federation_config.allow_private_peer_addresses,
        ) {
            tracing::warn!(
                outbox_id = id,
                peer = %peer_base,
                "dropping outbox row: peer base_url is not an allowed federation peer address (peer URL likely changed after enqueue)"
            );
            let pool = state.pool.clone();
            let _ = tokio::task::spawn_blocking(move || {
                if let Ok(conn) = pool.get() {
                    if let Err(e) = conn.execute(
                        "UPDATE federation_outbox SET status = 'failed', \
                         attempts = attempts + 1, \
                         last_error = 'peer base_url is not an allowed peer address (dequeue-time SSRF gate)', \
                         updated_at = datetime('now') WHERE id = ?1",
                        rusqlite::params![id],
                    ) {
                        tracing::error!(row = id, "outbox: could not mark row failed: {e}");
                    }
                }
            })
            .await;
            continue;
        }

        let url = format!(
            "{peer_base}{}",
            outbox_envelope_endpoint_path(&envelope_json)
        );

        let client = client.clone();
        let state = state.clone();
        handles.push(tokio::spawn(async move {
            let result = client
                .post(&url)
                .body(envelope_json.clone())
                .header("content-type", "application/json")
                .send()
                .await;
            match result {
                Ok(resp) if resp.status().is_success() => {
                    let pool = state.pool.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Ok(conn) = pool.get() {
                            // The one that matters most. A delivered row
                            // that cannot be marked delivered stays pending,
                            // is sent again on the next tick, and keeps
                            // going until `max_attempts` records it as
                            // failed — a message the peer already has,
                            // reported as undelivered, from a write nobody
                            // was told about.
                            if let Err(e) = conn.execute(
                                "UPDATE federation_outbox SET status = 'delivered', \
                                 attempts = attempts + 1, \
                                 last_error = NULL, \
                                 updated_at = datetime('now') WHERE id = ?1",
                                rusqlite::params![id],
                            ) {
                                tracing::error!(
                                    row = id,
                                    "outbox: delivered, but could not mark it so — it will be \
                                     redelivered until it exhausts its attempts: {e}"
                                );
                            }
                        }
                    })
                    .await;
                }
                Ok(resp) => {
                    let status_code = resp.status().as_u16();
                    let body = resp.text().await.unwrap_or_default();
                    let backoff = backoff_seconds(attempts + 1);
                    let pool = state.pool.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Ok(conn) = pool.get() {
                            if let Err(e) = conn.execute(
                                "UPDATE federation_outbox SET attempts = attempts + 1, \
                                 last_error = ?1, \
                                 next_retry_at = datetime('now', ?2), \
                                 updated_at = datetime('now') WHERE id = ?3",
                                rusqlite::params![
                                    format!(
                                        "HTTP {status_code}: {}",
                                        body.chars().take(200).collect::<String>()
                                    ),
                                    format!("+{backoff} seconds"),
                                    id,
                                ],
                            ) {
                                tracing::error!(
                                    row = id,
                                    "outbox: could not record the HTTP failure and backoff, so \
                                     this row will be retried immediately: {e}"
                                );
                            }
                        }
                    })
                    .await;
                }
                Err(e) => {
                    let backoff = backoff_seconds(attempts + 1);
                    let pool = state.pool.clone();
                    let err_str = e.to_string();
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Ok(conn) = pool.get() {
                            if let Err(e) = conn.execute(
                                "UPDATE federation_outbox SET attempts = attempts + 1, \
                                 last_error = ?1, \
                                 next_retry_at = datetime('now', ?2), \
                                 updated_at = datetime('now') WHERE id = ?3",
                                rusqlite::params![err_str, format!("+{backoff} seconds"), id,],
                            ) {
                                tracing::error!(
                                    row = id,
                                    "outbox: could not record the transport failure and backoff, \
                                     so this row will be retried immediately: {e}"
                                );
                            }
                        }
                    })
                    .await;
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }
    Ok(())
}

/// Periodic SQLite maintenance: WAL checkpoint(TRUNCATE), ANALYZE,
/// and optionally VACUUM. Runs only when
/// `Config::storage::maintenance_enabled = true`.
///
/// The worker spawns regardless of the flag so an operator can flip
/// it on without restarting; the loop body is a no-op when the flag
/// is false. VACUUM is gated separately because it rewrites the file
/// and blocks writers for the duration — operators opt in via
/// `maintenance_vacuum = true`.
pub async fn start_db_maintenance_task(state: Arc<AppState>) {
    let interval_secs = state
        .storage_config
        .maintenance_interval_hours
        .saturating_mul(3600);
    let tick = Duration::from_secs(interval_secs.max(60));
    tracing::info!(
        interval_seconds = tick.as_secs(),
        enabled = state.storage_config.maintenance_enabled,
        vacuum = state.storage_config.maintenance_vacuum,
        "starting SQLite maintenance task"
    );

    loop {
        sleep(tick).await;
        if !state.storage_config.maintenance_enabled {
            continue;
        }
        let pool = state.pool.clone();
        let vacuum = state.storage_config.maintenance_vacuum;
        let health = state.storage_health.clone();
        let _ = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let conn = pool.get().map_err(|e| format!("pool: {e}"))?;

            // 1. Checkpoint the WAL into the main DB so the WAL file
            //    stops growing. TRUNCATE shrinks the WAL file back to
            //    zero after the checkpoint succeeds.
            if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
                if crate::storage_health::interpret_sqlite_error(&health, &e) {
                    return Err(format!("checkpoint tripped storage gate: {e}"));
                }
                tracing::warn!("wal_checkpoint failed: {}", e);
            } else {
                tracing::info!("wal_checkpoint(TRUNCATE) completed");
            }

            // 2. ANALYZE rebuilds the query planner statistics. Cheap.
            if let Err(e) = conn.execute_batch("ANALYZE;") {
                tracing::warn!("ANALYZE failed: {}", e);
            }

            // 3. VACUUM only when operator-enabled. Blocks writers for
            //    the duration. Skipped under degraded storage because
            //    VACUUM transiently needs disk equal to the DB size.
            if vacuum {
                if health.writes_blocked() {
                    tracing::warn!(
                        "skipping VACUUM — storage gate is degraded and VACUUM needs free space"
                    );
                } else if let Err(e) = conn.execute_batch("VACUUM;") {
                    crate::storage_health::interpret_sqlite_error(&health, &e);
                    tracing::warn!("VACUUM failed: {}", e);
                } else {
                    tracing::info!("VACUUM completed");
                }
            }

            Ok(())
        })
        .await;
    }
}

/// Proactive half of the storage gate.
///
/// `storage_health`'s module header has always named this task as one
/// of the two ways the gate is promoted — and only the other one, the
/// reactive `SQLITE_FULL` trip, was ever wired. So
/// `ANNEX_STORAGE_BLOCK_FREE_BYTES`, documented as the point where "the
/// server refuses writes with HTTP 507", parsed, validated, reached
/// `AppState`, and was read by nothing: `evaluate_db_file_size` existed,
/// was unit-tested, and had no caller outside its own tests. An operator
/// who set it got the behaviour of one who did not — writes flowed until
/// SQLite itself returned `SQLITE_FULL`, which is the late, ambiguous
/// signal the module was written to replace.
///
/// Returns immediately when `storage.max_db_bytes` is 0, so it is always
/// safe to spawn. That is the default, and it is logged rather than
/// silent: the thresholds are headroom beneath the cap, so without a cap
/// there is nothing to measure them against.
pub async fn start_storage_probe_task(state: Arc<AppState>, db_path: std::path::PathBuf) {
    let max_bytes = state.storage_config.max_db_bytes;
    if max_bytes == 0 {
        tracing::info!(
            "storage probe disabled (storage.max_db_bytes = 0) — the gate can still \
             close reactively on a SQLite write error"
        );
        return;
    }

    let warn_free = state.storage_config.warn_free_bytes;
    let block_free = state.storage_config.block_free_bytes;
    tracing::info!(
        max_db_bytes = max_bytes,
        warn_free_bytes = warn_free,
        block_free_bytes = block_free,
        interval_seconds = STORAGE_PROBE_INTERVAL.as_secs(),
        "starting storage probe task"
    );

    // Check before the first sleep. A server restarted onto a database
    // that is already over the cap should refuse writes immediately,
    // not spend the first interval accepting them.
    let mut last = probe_once(&state, &db_path, warn_free, block_free, max_bytes).await;
    if last != crate::storage_health::StorageState::Healthy {
        tracing::warn!(
            state = last.as_str(),
            reason = %state.storage_health.reason(),
            "storage gate is not healthy at startup"
        );
    }

    loop {
        sleep(STORAGE_PROBE_INTERVAL).await;
        let now = probe_once(&state, &db_path, warn_free, block_free, max_bytes).await;
        // Log transitions only. At one probe a minute, logging every
        // tick would bury the transition that matters in noise.
        if now != last {
            tracing::warn!(
                from = last.as_str(),
                to = now.as_str(),
                reason = %state.storage_health.reason(),
                "storage gate state changed"
            );
            last = now;
        }
    }
}

/// One pass of the probe. `std::fs::metadata` is a blocking syscall, so
/// it goes to the blocking pool like the maintenance task's SQLite work.
async fn probe_once(
    state: &Arc<AppState>,
    db_path: &std::path::Path,
    warn_free: u64,
    block_free: u64,
    max_bytes: u64,
) -> crate::storage_health::StorageState {
    let health = state.storage_health.clone();
    let path = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        crate::storage_health::evaluate_db_file_size(
            &health,
            &path,
            warn_free,
            block_free,
            Some(max_bytes),
        )
    })
    .await
    // A panic in the blocking pool must not be read as "healthy" — that
    // would be the gate reporting the reassuring answer because it
    // failed to look. Report what the gate currently holds instead.
    .unwrap_or_else(|_| state.storage_health.state())
}

/// How often the storage probe stats the database file. Fixed rather
/// than configurable: a `stat` is cheap enough that a minute costs
/// nothing, and frequent enough that the gate closes well inside the
/// window between "filling up" and "full".
const STORAGE_PROBE_INTERVAL: Duration = Duration::from_secs(60);

/// Periodically deactivates federation agreements that have gone silent past
/// the configured TTL (`federation.agreement_ttl_days`).
///
/// Disabled when the TTL is `0`. The underlying SQL keys off `updated_at`,
/// which is refreshed on every re-handshake / policy re-evaluation, so
/// actively-maintained relationships are never reaped — only peers that have
/// stopped talking to us. Checks hourly; the day-granularity threshold is
/// applied in SQL.
pub async fn start_federation_agreement_expiry_task(state: Arc<AppState>) {
    let ttl_days = state.federation_config.agreement_ttl_days;
    if ttl_days == 0 {
        tracing::info!("federation agreement expiry disabled (agreement_ttl_days = 0)");
        return;
    }
    let tick = Duration::from_secs(3600);
    tracing::info!(
        ttl_days,
        "starting federation agreement expiry task (hourly check)"
    );

    loop {
        sleep(tick).await;
        let pool = state.pool.clone();
        let server_id = state.server_id;
        let _ = tokio::task::spawn_blocking(move || {
            let conn = match pool.get() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("agreement expiry: pool error: {e}");
                    return;
                }
            };
            match annex_federation::expire_stale_agreements(&conn, server_id, ttl_days) {
                Ok(0) => {}
                Ok(n) => tracing::info!(expired = n, "expired stale federation agreements"),
                Err(e) => tracing::warn!("agreement expiry failed: {e}"),
            }
        })
        .await;
    }
}

/// Bounded-exponential backoff: 60s, 120s, 240s, … up to a 1-hour cap.
/// The caller passes the *next* attempt count (already incremented),
/// so `backoff_seconds(1)` is the first retry interval.
fn backoff_seconds(next_attempt: u32) -> u64 {
    let base: u64 = 60;
    let cap: u64 = 3600;
    let shift = next_attempt.saturating_sub(1).min(8);
    (base.saturating_mul(1u64 << shift)).min(cap)
}

/// How many attempts of the backoff schedule can still be accepted by a peer,
/// given the receiver's live freshness window.
///
/// The outbox posts to `/api/federation/messages`, which checks freshness in
/// `DeliveryMode::Live` and rejects anything whose `created_at` is older than
/// `freshness_window_seconds`. The envelope is signed at enqueue and its
/// timestamp never moves, so once the cumulative backoff passes that window
/// every remaining retry is guaranteed to be rejected — not "likely to fail",
/// but structurally incapable of succeeding.
///
/// With the shipped defaults that was 12 attempts against a 300s window: the
/// initial send, retries at 60s and 180s, and then nine more at 420s, 900s,
/// 1860s … out to roughly three hours, all of them rejected as too old, after
/// which the row was marked failed and the message dropped. The retries cost
/// the peer a signature verification each and told the operator nothing, and
/// `DeliveryMode::Catchup` — the mode that would accept them — is never
/// constructed anywhere, so there is nothing to fall back to.
///
/// Returns the number of sends (including the first) that land inside the
/// window, floored at 1.
pub(crate) fn attempts_within_freshness_window(freshness_window_seconds: i64) -> u32 {
    if freshness_window_seconds <= 0 {
        return 1;
    }
    let window = freshness_window_seconds as u64;
    let mut elapsed = 0u64;
    // The first send happens immediately, so it always counts.
    let mut sends = 1u32;
    loop {
        let next = backoff_seconds(sends);
        match elapsed.checked_add(next) {
            Some(t) if t <= window => {
                elapsed = t;
                sends += 1;
            }
            _ => return sends,
        }
    }
}

#[cfg(test)]
mod outbox_routing_tests {
    use super::outbox_envelope_endpoint_path;

    #[test]
    fn redaction_envelopes_route_to_redactions_endpoint() {
        let json = r#"{"envelopeKind":"redaction","envelopeVersion":"v1","message_id":"m"}"#;
        assert_eq!(
            outbox_envelope_endpoint_path(json),
            "/api/federation/redactions"
        );
    }

    #[test]
    fn message_envelopes_route_to_messages_endpoint() {
        // Message envelopes carry no envelopeKind field.
        let json = r#"{"envelopeVersion":"v2","message_id":"m","content":"hi"}"#;
        assert_eq!(
            outbox_envelope_endpoint_path(json),
            "/api/federation/messages"
        );
    }

    #[test]
    fn edit_envelopes_route_to_edits_endpoint() {
        let json =
            r#"{"envelopeKind":"edit","envelopeVersion":"v1","edit_id":"e","message_id":"m"}"#;
        assert_eq!(outbox_envelope_endpoint_path(json), "/api/federation/edits");
    }

    #[test]
    fn unknown_kind_and_garbage_fall_back_to_messages_endpoint() {
        assert_eq!(
            outbox_envelope_endpoint_path(r#"{"envelopeKind":"mystery"}"#),
            "/api/federation/messages"
        );
        assert_eq!(
            outbox_envelope_endpoint_path("not json at all"),
            "/api/federation/messages"
        );
    }
}

#[cfg(test)]
mod backoff_tests {
    use super::backoff_seconds;

    #[test]
    fn first_attempt_is_one_minute() {
        assert_eq!(backoff_seconds(1), 60);
    }

    #[test]
    fn schedule_doubles_until_cap() {
        let xs: Vec<_> = (1..=10).map(backoff_seconds).collect();
        // 60, 120, 240, 480, 960, 1920, 3600 (cap), 3600, 3600, 3600
        assert_eq!(xs[0], 60);
        assert_eq!(xs[1], 120);
        assert_eq!(xs[2], 240);
        assert_eq!(xs[3], 480);
        assert_eq!(xs[4], 960);
        assert_eq!(xs[5], 1920);
        assert_eq!(xs[6], 3600);
        assert_eq!(xs[7], 3600);
    }

    #[test]
    fn never_overflows_or_drops_below_one_minute() {
        for n in 0..64u32 {
            let s = backoff_seconds(n);
            assert!(s >= 60, "n={n} → {s}");
            assert!(s <= 3600, "n={n} → {s}");
        }
    }
}

#[cfg(test)]
mod freshness_budget_tests {
    use super::attempts_within_freshness_window;

    /// The shipped pairing. The backoff is 60s, 120s, 240s …, so against a
    /// 300s window the immediate send plus retries at 60s and 180s land, and
    /// the one at 420s does not.
    #[test]
    fn the_default_window_admits_three_sends() {
        assert_eq!(attempts_within_freshness_window(300), 3);
    }

    /// The old default was 12 against that same window — nine sends that the
    /// receiver was obliged to reject as too old, spread over ~3 hours.
    #[test]
    fn twelve_attempts_never_fitted_the_default_window() {
        assert!(
            attempts_within_freshness_window(300) < 12,
            "if this ever passes, the retry budget and the freshness window \
             have been reconciled and the warning at startup is dead code",
        );
    }

    #[test]
    fn a_wider_window_admits_more_sends() {
        // 60 + 120 + 240 + 480 = 900
        assert_eq!(attempts_within_freshness_window(900), 5);
    }

    #[test]
    fn a_window_smaller_than_the_first_backoff_still_admits_the_first_send() {
        // The initial POST happens immediately and is always worth making.
        assert_eq!(attempts_within_freshness_window(30), 1);
        assert_eq!(attempts_within_freshness_window(0), 1);
        assert_eq!(attempts_within_freshness_window(-5), 1);
    }
}
