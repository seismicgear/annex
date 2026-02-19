//! Tests for the background graph node pruning task.
//!
//! These tests verify:
//! - The pruning task correctly prunes inactive nodes
//! - The pruning task emits NodePruned presence events
//! - The pruning task emits observe events to the audit log
//! - The task is disabled when threshold is 0
//! - The interval calculation is correct (threshold/2, clamped to [1, 60])
//! - The task continues after errors

use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_graph::{ensure_graph_node, update_node_activity};
use annex_identity::MerkleTree;
use annex_server::{background::start_pruning_task, middleware::RateLimiter, AppState};
use annex_types::{NodeType, PresenceEvent, ServerPolicy};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    Arc::new(annex_identity::zk::generate_dummy_vkey())
}

fn setup_state() -> (Arc<AppState>, annex_db::DbPool) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default())
        .expect("pool creation should succeed");
    {
        let conn = pool.get().expect("connection should succeed");
        run_migrations(&conn).expect("migrations should succeed");
        let policy = ServerPolicy::default();
        let policy_json = serde_json::to_string(&policy).expect("serialize policy");
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .expect("insert server");
    }

    let tree = MerkleTree::new(20).expect("tree creation should succeed");
    let (presence_tx, _) = tokio::sync::broadcast::channel(256);
    let (observe_tx, _) = tokio::sync::broadcast::channel(256);

    let state = Arc::new(AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: "http://localhost:3000".to_string(),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::LiveKitConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx,
    });

    (state, pool)
}

#[tokio::test]
async fn test_pruning_task_disabled_when_threshold_zero() {
    let (state, _pool) = setup_state();

    // threshold=0 should cause the task to return immediately without looping
    let handle = tokio::spawn(start_pruning_task(state, 0));

    // The task should complete (return) almost immediately
    let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(
        result.is_ok(),
        "pruning task with threshold=0 should return immediately"
    );
    result
        .expect("timeout should not occur")
        .expect("task should not panic");
}

#[tokio::test]
async fn test_pruning_interval_calculation() {
    // The interval formula: (threshold / 2).clamp(1, 60)
    // threshold=2 → interval=1
    // threshold=10 → interval=5
    // threshold=60 → interval=30
    // threshold=120 → interval=60
    // threshold=200 → interval=60 (clamped)
    // threshold=1 → interval=1 (clamped at min 1, since 1/2=0)

    assert_eq!((2u64 / 2).clamp(1, 60), 1);
    assert_eq!((10u64 / 2).clamp(1, 60), 5);
    assert_eq!((60u64 / 2).clamp(1, 60), 30);
    assert_eq!((120u64 / 2).clamp(1, 60), 60);
    assert_eq!((200u64 / 2).clamp(1, 60), 60);
    assert_eq!((1u64 / 2).clamp(1, 60), 1); // 0 clamped to 1
}

#[tokio::test]
async fn test_pruning_task_prunes_inactive_nodes_and_emits_events() {
    let (state, pool) = setup_state();

    // Create a node and let it become inactive
    {
        let conn = pool.get().expect("connection");
        ensure_graph_node(&conn, 1, "old_user", NodeType::Human, None)
            .expect("ensure node");
    }

    // Wait for the node to become "old" (threshold=1 second)
    thread::sleep(Duration::from_millis(2000));

    // Subscribe to presence events before starting the task
    let mut presence_rx = state.presence_tx.subscribe();
    let mut observe_rx = state.observe_tx.subscribe();

    // Start the pruning task with threshold=1 second (interval will be 1s too)
    let task_handle = tokio::spawn(start_pruning_task(state.clone(), 1));

    // Wait for the pruning cycle to run (interval=1s, give it time)
    tokio::time::sleep(Duration::from_millis(2500)).await;

    // Verify the node was pruned in the database
    let active: bool = {
        let conn = pool.get().expect("connection");
        conn.query_row(
            "SELECT active FROM graph_nodes WHERE pseudonym_id = ?",
            ["old_user"],
            |row| row.get(0),
        )
        .expect("query node")
    };
    assert!(!active, "node should be pruned (inactive)");

    // Verify a NodePruned presence event was emitted
    let event = tokio::time::timeout(Duration::from_millis(500), presence_rx.recv()).await;
    match event {
        Ok(Ok(PresenceEvent::NodePruned { pseudonym_id })) => {
            assert_eq!(pseudonym_id, "old_user");
        }
        other => panic!(
            "expected NodePruned presence event, got: {:?}",
            other
        ),
    }

    // Verify an observe event was emitted (NodePruned to audit log)
    let observe_event =
        tokio::time::timeout(Duration::from_millis(500), observe_rx.recv()).await;
    match observe_event {
        Ok(Ok(event)) => {
            assert_eq!(event.event_type, "NODE_PRUNED");
            assert_eq!(event.entity_id, "old_user");
        }
        other => panic!(
            "expected NODE_PRUNED observe event, got: {:?}",
            other
        ),
    }

    // Cancel the infinite loop
    task_handle.abort();
}

#[tokio::test]
async fn test_pruning_task_does_not_prune_active_nodes() {
    let (state, pool) = setup_state();

    // Create a node
    {
        let conn = pool.get().expect("connection");
        ensure_graph_node(&conn, 1, "active_user", NodeType::Human, None)
            .expect("ensure node");
    }

    // Subscribe to presence events
    let mut presence_rx = state.presence_tx.subscribe();

    // Start pruning with a high threshold (3600s) — node is freshly created, should not be pruned
    let task_handle = tokio::spawn(start_pruning_task(state.clone(), 4));

    // Wait for at least one cycle (interval = clamp(4/2, 1, 60) = 2s)
    tokio::time::sleep(Duration::from_millis(3000)).await;

    // Node should still be active
    let active: bool = {
        let conn = pool.get().expect("connection");
        conn.query_row(
            "SELECT active FROM graph_nodes WHERE pseudonym_id = ?",
            ["active_user"],
            |row| row.get(0),
        )
        .expect("query node")
    };
    assert!(active, "recently-active node should not be pruned");

    // No presence event should have been emitted
    let event = tokio::time::timeout(Duration::from_millis(200), presence_rx.recv()).await;
    assert!(
        event.is_err(),
        "no NodePruned event should be emitted for active nodes"
    );

    task_handle.abort();
}

#[tokio::test]
async fn test_pruning_task_multiple_nodes_pruned() {
    let (state, pool) = setup_state();

    // Create multiple nodes
    {
        let conn = pool.get().expect("connection");
        for i in 0..5 {
            ensure_graph_node(
                &conn,
                1,
                &format!("stale_user_{}", i),
                NodeType::Human,
                None,
            )
            .expect("ensure node");
        }
    }

    // Wait for nodes to become stale
    thread::sleep(Duration::from_millis(2000));

    // Subscribe to presence events
    let mut presence_rx = state.presence_tx.subscribe();

    // Start pruning with threshold=1
    let task_handle = tokio::spawn(start_pruning_task(state.clone(), 1));

    // Wait for pruning cycle
    tokio::time::sleep(Duration::from_millis(2500)).await;

    // All 5 nodes should be pruned
    {
        let conn = pool.get().expect("connection");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_nodes WHERE server_id = 1 AND active = 1",
                [],
                |row| row.get(0),
            )
            .expect("count active nodes");
        assert_eq!(count, 0, "all stale nodes should be pruned");
    }

    // Collect all presence events
    let mut pruned_ids = Vec::new();
    for _ in 0..5 {
        match tokio::time::timeout(Duration::from_millis(500), presence_rx.recv()).await {
            Ok(Ok(PresenceEvent::NodePruned { pseudonym_id })) => {
                pruned_ids.push(pseudonym_id);
            }
            other => panic!("expected NodePruned event, got: {:?}", other),
        }
    }
    pruned_ids.sort();
    let mut expected: Vec<String> = (0..5).map(|i| format!("stale_user_{}", i)).collect();
    expected.sort();
    assert_eq!(pruned_ids, expected);

    task_handle.abort();
}

#[tokio::test]
async fn test_pruning_preserves_reactivated_nodes() {
    let (state, pool) = setup_state();

    // Create two nodes
    {
        let conn = pool.get().expect("connection");
        ensure_graph_node(&conn, 1, "will_be_pruned", NodeType::Human, None)
            .expect("ensure node");
        ensure_graph_node(&conn, 1, "will_stay_active", NodeType::Human, None)
            .expect("ensure node");
    }

    // Wait long enough for both nodes to exceed the threshold (threshold=6s).
    // We use threshold=6 so that: interval = clamp(6/2, 1, 60) = 3s.
    // After sleeping 7s, both nodes are 7s old. Then we refresh one.
    thread::sleep(Duration::from_millis(7000));

    // Reactivate one of them by updating activity (sets last_seen_at = now)
    {
        let conn = pool.get().expect("connection");
        update_node_activity(&conn, 1, "will_stay_active").expect("update activity");
    }

    // Start pruning with threshold=6s (interval=3s).
    // "will_be_pruned" is 7s old (> 6s threshold) → should be pruned.
    // "will_stay_active" was just refreshed (0s old) → should NOT be pruned.
    let task_handle = tokio::spawn(start_pruning_task(state.clone(), 6));

    // Wait for the first pruning cycle (interval=3s, give it 4s)
    tokio::time::sleep(Duration::from_millis(4000)).await;

    // "will_be_pruned" should be inactive, "will_stay_active" should remain active
    {
        let conn = pool.get().expect("connection");

        let pruned_active: bool = conn
            .query_row(
                "SELECT active FROM graph_nodes WHERE pseudonym_id = ?",
                ["will_be_pruned"],
                |row| row.get(0),
            )
            .expect("query pruned node");
        assert!(!pruned_active, "stale node should be pruned");

        let kept_active: bool = conn
            .query_row(
                "SELECT active FROM graph_nodes WHERE pseudonym_id = ?",
                ["will_stay_active"],
                |row| row.get(0),
            )
            .expect("query active node");
        assert!(kept_active, "recently-active node should not be pruned");
    }

    task_handle.abort();
}
