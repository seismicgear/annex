//! Background task for enforcing message retention policies.

use annex_db::DbPool;
use std::time::Duration;
use tokio::time::sleep;

/// Starts a background task that periodically deletes expired messages.
///
/// This task runs indefinitely.
///
/// # Arguments
///
/// * `pool` - Database connection pool.
/// * `interval_seconds` - Time in seconds to wait between retention checks.
pub async fn start_retention_task(pool: DbPool, interval_seconds: u64) {
    let interval = Duration::from_secs(interval_seconds);
    tracing::info!(
        interval_seconds,
        "starting message retention enforcement task"
    );

    loop {
        // Sleep first (or sleep after? Usually sleep first to not hammer DB immediately on startup,
        // but maybe we want to clean up immediately? Let's sleep first to let startup settle.)
        sleep(interval).await;

        // Run blocking DB operation in a separate thread
        let pool_clone = pool.clone();
        let result = tokio::task::spawn_blocking(move || {
            let conn = match pool_clone.get() {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error = %e, "failed to get database connection for retention task");
                    return Ok(0);
                }
            };
            annex_channels::delete_expired_messages(&conn)
        })
        .await;

        match result {
            Ok(Ok(count)) => {
                if count > 0 {
                    tracing::info!(count, "deleted expired messages");
                } else {
                    tracing::debug!("no expired messages to delete");
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
