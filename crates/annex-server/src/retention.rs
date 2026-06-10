//! Background task for enforcing message retention policies and the
//! WS-idempotency ledger TTL (ADR-0010).

use annex_db::DbPool;
use std::time::Duration;
use tokio::time::sleep;

/// Starts a background task that periodically deletes expired messages
/// and prunes idempotency-ledger rows older than the configured TTL.
///
/// This task runs indefinitely.
///
/// # Arguments
///
/// * `pool` - Database connection pool.
/// * `interval_seconds` - Time in seconds to wait between retention checks.
/// * `idempotency_ttl_seconds` - Age in seconds past which
///   `message_request_ids` rows are evicted. After eviction a replayed
///   `clientRequestId` is treated as a new send, so this must comfortably
///   exceed any client retry window (default: 7 days).
pub async fn start_retention_task(
    pool: DbPool,
    interval_seconds: u64,
    idempotency_ttl_seconds: u64,
) {
    let interval = Duration::from_secs(interval_seconds);
    tracing::info!(
        interval_seconds,
        idempotency_ttl_seconds,
        "starting message retention enforcement task"
    );

    loop {
        // Sleep first (or sleep after? Usually sleep first to not hammer DB immediately on startup,
        // but maybe we want to clean up immediately? Let's sleep first to let startup settle.)
        sleep(interval).await;

        // Run blocking DB operation in a separate thread.
        // Both sweeps are batched, so we loop until fewer rows are
        // deleted than the batch limit, indicating all expired rows have
        // been removed.
        let pool_clone = pool.clone();
        let result = tokio::task::spawn_blocking(move || {
            let conn = pool_clone.get().map_err(|e| {
                // Convert pool error to a rusqlite error so callers see
                // this as a real failure rather than silently returning Ok(0).
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                    Some(format!("pool connection error: {e}")),
                )
            })?;
            let mut total: usize = 0;
            loop {
                let deleted = annex_channels::delete_expired_messages(&conn)?;
                total += deleted;
                if deleted < 5_000 {
                    break;
                }
            }
            let mut ledger_total: usize = 0;
            loop {
                let pruned =
                    annex_channels::prune_expired_request_ids(&conn, idempotency_ttl_seconds)?;
                ledger_total += pruned;
                if pruned < 5_000 {
                    break;
                }
            }
            Ok::<(usize, usize), annex_channels::ChannelError>((total, ledger_total))
        })
        .await;

        match result {
            Ok(Ok((count, ledger_count))) => {
                if count > 0 {
                    tracing::info!(count, "deleted expired messages");
                } else {
                    tracing::debug!("no expired messages to delete");
                }
                if ledger_count > 0 {
                    tracing::info!(
                        count = ledger_count,
                        "pruned expired idempotency-ledger rows"
                    );
                }
            }
            Ok(Err(e)) => {
                tracing::error!(error = %e, "failed to delete expired messages");
            }
            Err(e) => {
                tracing::error!(error = %e, "retention task panicked or was cancelled");
            }
        }
    }
}
