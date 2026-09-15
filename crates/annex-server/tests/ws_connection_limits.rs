//! One valid token must not be able to pin unbounded sockets.
//!
//! `add_session` replaces an existing session for the same pseudonym, which
//! reads as a per-identity cap of one. It was not. Replacement dropped the old
//! session's `Sender`, which ends the *writer* task — but the reader loop in
//! `ws::session` was `while let Some(Ok(msg)) = receiver.next().await`, which
//! ends only when the CLIENT closes. So an evicted socket stayed open, holding
//! a reader task, an ICE relay, a renegotiation relay and a 1024-slot channel,
//! and simply stopped receiving anything.
//!
//! A client reconnecting in a loop with ONE token therefore accumulated
//! sockets without bound. Nothing in the registry grew — `sessions` still held
//! exactly one entry per identity — which is why it did not look like a leak
//! from the inside.
//!
//! Two things fix it and both are tested here: an evicted session is told to
//! close, and the process has a ceiling on admitted sockets independent of how
//! many identities are involved.

use annex_server::api_ws::ConnectionManager;
use std::time::Duration;
use tokio::sync::mpsc;

fn dummy_sender() -> mpsc::Sender<String> {
    mpsc::channel::<String>(1).0
}

/// The property the old code lacked: the evicted session is *told*, rather
/// than merely being cut off from writes.
#[tokio::test]
async fn replacing_a_session_cancels_the_one_it_replaced() {
    let cm = ConnectionManager::new();

    let (first_id, first_cancel) = cm.add_session("alice".to_string(), dummy_sender()).await;
    assert!(
        !first_cancel.is_cancelled(),
        "a session should not start cancelled"
    );

    let (second_id, second_cancel) = cm.add_session("alice".to_string(), dummy_sender()).await;
    assert_ne!(first_id, second_id, "each session gets its own id");

    assert!(
        first_cancel.is_cancelled(),
        "the replaced session must be told to close — without this its socket, \
         reader task and event relays stay alive on a connection nobody can reach"
    );
    assert!(
        !second_cancel.is_cancelled(),
        "replacing a session must not cancel the replacement"
    );
}

/// A reader loop that selects on the token must actually return. Driven with a
/// real `select!` rather than by inspecting the flag, because the flag being
/// set is not the property — leaving the loop is.
#[tokio::test]
async fn a_cancelled_session_loop_returns() {
    let cm = ConnectionManager::new();
    let (_id, cancel) = cm.add_session("alice".to_string(), dummy_sender()).await;

    // Stands in for `ws::session`'s loop: a socket that never speaks again.
    let reader = tokio::spawn(async move {
        tokio::select! {
            _ = cancel.cancelled() => "cancelled",
            _ = std::future::pending::<()>() => "client spoke",
        }
    });

    // The replacement is what cancels it.
    cm.add_session("alice".to_string(), dummy_sender()).await;

    let outcome = tokio::time::timeout(Duration::from_secs(2), reader)
        .await
        .expect("an evicted reader must return promptly")
        .expect("and must not panic");
    assert_eq!(outcome, "cancelled");
}

/// `disconnect_user` cleared the registry and left the socket open. A
/// moderator kicking someone changed what the server would send them and not
/// whether they were connected.
#[tokio::test]
async fn disconnect_user_actually_disconnects() {
    let cm = ConnectionManager::new();
    let (_id, cancel) = cm.add_session("mallory".to_string(), dummy_sender()).await;

    cm.disconnect_user("mallory").await;

    assert!(
        cancel.is_cancelled(),
        "disconnecting a user must close their socket, not just forget about it"
    );
}

/// A stale removal — one that names an id the registry has already replaced —
/// must not cancel the live session. This is the same guard the id check
/// already gave the subscription cleanup, extended to the close signal.
#[tokio::test]
async fn a_stale_removal_does_not_close_the_current_session() {
    let cm = ConnectionManager::new();
    let (stale_id, _stale_cancel) = cm.add_session("alice".to_string(), dummy_sender()).await;
    let (_live_id, live_cancel) = cm.add_session("alice".to_string(), dummy_sender()).await;

    // The evicted session's own cleanup runs late and names its old id.
    cm.remove_session("alice", stale_id).await;

    assert!(
        !live_cancel.is_cancelled(),
        "a late cleanup from a replaced session must not close the live one — \
         otherwise a reconnect races itself off the server"
    );
}

/// The ceiling is on admitted sockets, not on identities. Sizing it off
/// `sessions.len()` would have counted every evicted-but-still-open connection
/// as zero, which is exactly the population the cap needs to see.
#[tokio::test]
async fn the_connection_cap_counts_sockets_not_identities() {
    let cm = ConnectionManager::new();
    assert_eq!(cm.open_connections(), 0);

    // Ten sockets, all for one identity, none of them registered as sessions.
    let slots: Vec<_> = (0..10)
        .map(|_| cm.try_admit().expect("under the cap"))
        .collect();
    assert_eq!(cm.open_connections(), 10);

    drop(slots);
    assert_eq!(
        cm.open_connections(),
        0,
        "dropping a slot must release it — the guard is what makes every early \
         return and every panic give the slot back"
    );
}

/// At the ceiling, admission is refused rather than deferred.
#[tokio::test]
async fn admission_is_refused_at_the_cap() {
    let cm = ConnectionManager::new();
    let cap = annex_server::api_ws::MAX_WS_CONNECTIONS;

    let mut slots = Vec::with_capacity(cap);
    for _ in 0..cap {
        slots.push(cm.try_admit().expect("under the cap"));
    }
    assert_eq!(cm.open_connections(), cap);
    assert!(
        cm.try_admit().is_none(),
        "the cap must refuse, not overshoot"
    );

    // And a single release re-opens exactly one slot. The reclaimed slot is
    // BOUND rather than asserted on directly: `assert!(try_admit().is_some())`
    // drops the guard at the end of the statement, which hands the slot
    // straight back and makes the next call succeed too — a test that would
    // have passed against a cap that did not hold.
    slots.pop();
    let reclaimed = cm.try_admit();
    assert!(reclaimed.is_some(), "releasing one slot should re-open one");
    assert!(
        cm.try_admit().is_none(),
        "releasing one slot must re-open exactly one"
    );
    drop(reclaimed);
}

/// Concurrent admission must not overshoot. `fetch_update` is a compare-and-
/// swap loop; a naive `load`-then-`store` would let two racing upgrades both
/// read `cap - 1` and both proceed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_admission_never_exceeds_the_cap() {
    use std::sync::Arc;

    let cm = Arc::new(ConnectionManager::new());
    let cap = annex_server::api_ws::MAX_WS_CONNECTIONS;

    // Fill to one below the cap, then race many claimants for the last slot.
    let mut held: Vec<_> = (0..cap - 1).map(|_| cm.try_admit().unwrap()).collect();

    let mut tasks = Vec::new();
    for _ in 0..64 {
        let cm = cm.clone();
        tasks.push(tokio::spawn(async move { cm.try_admit() }));
    }
    let granted: Vec<_> = futures_util::future::join_all(tasks)
        .await
        .into_iter()
        .filter_map(|r| r.expect("task should not panic"))
        .collect();

    assert_eq!(
        granted.len(),
        1,
        "exactly one of 64 racing claimants should get the last slot, got {}",
        granted.len()
    );
    assert_eq!(cm.open_connections(), cap);

    held.clear();
    drop(granted);
    assert_eq!(cm.open_connections(), 0);
}
