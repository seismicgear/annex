//! Background tasks for the Annex server.
//!
//! Includes:
//! - Pruning inactive graph nodes.

use crate::AppState;
use annex_graph::prune_inactive_nodes;
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

        let res = tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| e.to_string())?;
            prune_inactive_nodes(&conn, server_id, threshold_seconds).map_err(|e| e.to_string())
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
