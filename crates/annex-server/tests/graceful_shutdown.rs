//! Background workers must stop when told.
//!
//! `axum::serve(..).with_graceful_shutdown(..)` drains in-flight HTTP requests
//! and nothing else. Every background worker was a detached `tokio::spawn`
//! with no handle and no stop signal, so on SIGTERM they kept running — six
//! timers, a federation outbox mid-delivery among them — until the runtime was
//! dropped out from under them or the container runtime escalated to SIGKILL.
//!
//! The visible cost is a slow stop. The expensive one is the outbox worker
//! being killed between marking a row attempted and actually sending it: the
//! peer never receives a message this server believes it delivered.
//!
//! These drive the real worker functions, not a stand-in, because the property
//! under test is "this loop observes the token" — and a mock loop observes it
//! by construction.

mod common;

use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A worker must return promptly after cancellation, from inside its sleep.
///
/// The interval is far longer than the assertion's patience on purpose: if the
/// worker were waking on its timer rather than on the token, this would time
/// out rather than pass slowly.
async fn assert_stops_when_cancelled<F>(name: &str, shutdown: CancellationToken, worker: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let handle = tokio::spawn(worker);

    // Let the worker reach its first await point.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !handle.is_finished(),
        "{name} exited before it was asked to — this test would pass for the wrong reason"
    );

    shutdown.cancel();
    match tokio::time::timeout(Duration::from_secs(2), handle).await {
        Ok(joined) => joined.unwrap_or_else(|e| panic!("{name} panicked on shutdown: {e}")),
        Err(_) => panic!(
            "{name} did not stop within 2s of cancellation — it is sleeping on its own timer \
             rather than selecting on the shutdown token, which is exactly the state that \
             turns SIGTERM into SIGKILL"
        ),
    }
}

#[tokio::test]
async fn the_rate_limit_cleanup_worker_stops() {
    let shutdown = CancellationToken::new();
    let limiter = annex_server::middleware::RateLimiter::new();
    assert_stops_when_cancelled(
        "rate limit cleanup worker",
        shutdown.clone(),
        annex_server::background::start_rate_limit_cleanup_task(limiter, shutdown),
    )
    .await;
}

#[tokio::test]
async fn the_federation_outbox_worker_stops() {
    let (_app, pool) = common::setup_test_app().await;
    let shutdown = CancellationToken::new();
    let state = Arc::new(common::build_app_state(
        pool,
        annex_identity::MerkleTree::new(20).unwrap(),
        annex_types::ServerPolicy::default(),
    ));
    // `build_app_state` hands out a token nothing cancels; swap in ours.
    let state = Arc::new(annex_server::AppState {
        shutdown: shutdown.clone(),
        ..(*state).clone()
    });

    assert_stops_when_cancelled(
        "federation outbox worker",
        shutdown,
        annex_server::background::start_federation_outbox_task(state),
    )
    .await;
}

#[tokio::test]
async fn the_storage_probe_worker_stops() {
    let (_app, pool) = common::setup_test_app().await;
    let shutdown = CancellationToken::new();
    let mut state = common::build_app_state(
        pool,
        annex_identity::MerkleTree::new(20).unwrap(),
        annex_types::ServerPolicy::default(),
    );
    state.shutdown = shutdown.clone();
    // A non-zero budget, or the worker returns immediately by design and the
    // assertion above would (correctly) refuse to call that a pass.
    state.storage_config.max_db_bytes = 1_000_000_000;

    assert_stops_when_cancelled(
        "storage probe worker",
        shutdown,
        annex_server::background::start_storage_probe_task(
            Arc::new(state),
            std::path::PathBuf::from(":memory:"),
        ),
    )
    .await;
}

#[tokio::test]
async fn the_graph_pruning_worker_stops() {
    let (_app, pool) = common::setup_test_app().await;
    let shutdown = CancellationToken::new();
    let mut state = common::build_app_state(
        pool,
        annex_identity::MerkleTree::new(20).unwrap(),
        annex_types::ServerPolicy::default(),
    );
    state.shutdown = shutdown.clone();

    assert_stops_when_cancelled(
        "graph pruning worker",
        shutdown,
        // A large threshold means a 60s tick, so a pass here cannot be the
        // worker simply finishing a short sleep.
        annex_server::background::start_pruning_task(Arc::new(state), 86_400),
    )
    .await;
}

#[tokio::test]
async fn the_agreement_expiry_worker_stops() {
    let (_app, pool) = common::setup_test_app().await;
    let shutdown = CancellationToken::new();
    let mut state = common::build_app_state(
        pool,
        annex_identity::MerkleTree::new(20).unwrap(),
        annex_types::ServerPolicy::default(),
    );
    state.shutdown = shutdown.clone();
    state.federation_config.agreement_ttl_days = 30;

    assert_stops_when_cancelled(
        "federation agreement expiry worker",
        shutdown,
        annex_server::background::start_federation_agreement_expiry_task(Arc::new(state)),
    )
    .await;
}

/// Cancelling twice, or cancelling before the worker starts, must not hang or
/// panic. Shutdown paths get exercised exactly once per process in production
/// and are the easiest place for a latent panic to hide.
#[tokio::test]
async fn cancelling_before_the_worker_starts_is_not_a_hang() {
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    shutdown.cancel();

    let limiter = annex_server::middleware::RateLimiter::new();
    let worker = tokio::spawn(annex_server::background::start_rate_limit_cleanup_task(
        limiter, shutdown,
    ));
    tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .expect("a worker started under an already-cancelled token must return immediately")
        .expect("and must not panic");
}
