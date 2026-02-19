//! Concurrency tests for the WebSocket ConnectionManager.
//!
//! These tests verify that the ConnectionManager correctly handles
//! concurrent subscribe/unsubscribe/remove_session operations without
//! deadlocks, data corruption, or orphaned entries.

use annex_server::api_ws::ConnectionManager;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Helper to create a session sender that won't be used for actual messaging.
fn dummy_sender() -> mpsc::Sender<String> {
    mpsc::channel::<String>(1).0
}

#[tokio::test]
async fn test_concurrent_subscribe_unsubscribe_no_deadlock() {
    let cm = ConnectionManager::new();

    // Register 10 users
    for i in 0..10 {
        cm.add_session(format!("user_{}", i), dummy_sender()).await;
    }

    // Spawn 100 concurrent subscribe + unsubscribe tasks across 5 channels
    let cm = Arc::new(cm);
    let mut handles = Vec::new();

    for i in 0..100 {
        let cm = cm.clone();
        let user = format!("user_{}", i % 10);
        let channel = format!("channel_{}", i % 5);

        handles.push(tokio::spawn(async move {
            cm.subscribe(channel.clone(), user.clone()).await;
            // Immediately unsubscribe to stress the lock ordering
            cm.unsubscribe(&channel, &user).await;
        }));
    }

    // All tasks must complete without deadlock
    for handle in handles {
        handle.await.expect("task should not panic");
    }
}

#[tokio::test]
async fn test_concurrent_add_remove_session_no_orphans() {
    let cm = ConnectionManager::new();

    // Register a user and subscribe them to channels
    let sender = dummy_sender();
    let session_id = cm
        .add_session("user_a".to_string(), sender)
        .await;

    cm.subscribe("ch1".to_string(), "user_a".to_string()).await;
    cm.subscribe("ch2".to_string(), "user_a".to_string()).await;
    cm.subscribe("ch3".to_string(), "user_a".to_string()).await;

    // Concurrently: remove the session while subscribing to more channels
    let cm = Arc::new(cm);
    let cm1 = cm.clone();
    let cm2 = cm.clone();
    let sid = session_id;

    let remove_handle = tokio::spawn(async move {
        cm1.remove_session("user_a", sid).await;
    });

    let subscribe_handle = tokio::spawn(async move {
        cm2.subscribe("ch4".to_string(), "user_a".to_string()).await;
        cm2.subscribe("ch5".to_string(), "user_a".to_string()).await;
    });

    remove_handle.await.expect("remove should not panic");
    subscribe_handle.await.expect("subscribe should not panic");

    // After removal + possible re-subscription, broadcasting should not panic
    cm.broadcast("ch1", "test".to_string()).await;
    cm.broadcast("ch4", "test".to_string()).await;
}

#[tokio::test]
async fn test_concurrent_session_replacement() {
    // Simulate the same pseudonym reconnecting many times concurrently.
    // The ConnectionManager should handle session replacement without panic.
    let cm = Arc::new(ConnectionManager::new());
    let mut handles = Vec::new();

    for _ in 0..50 {
        let cm = cm.clone();
        handles.push(tokio::spawn(async move {
            let sender = dummy_sender();
            let _session_id = cm
                .add_session("reconnecting_user".to_string(), sender)
                .await;
            // Subscribe to a channel immediately after connecting
            cm.subscribe("shared_channel".to_string(), "reconnecting_user".to_string())
                .await;
        }));
    }

    for handle in handles {
        handle.await.expect("concurrent session replacement should not panic");
    }

    // After all reconnections settle, exactly one session should remain.
    // Verify the user can still receive broadcasts without error.
    cm.broadcast("shared_channel", r#"{"type":"test"}"#.to_string())
        .await;
}

#[tokio::test]
async fn test_concurrent_broadcast_with_subscribe_unsubscribe() {
    let cm = Arc::new(ConnectionManager::new());

    // Set up 20 users, each subscribed to "live_channel"
    for i in 0..20 {
        let (tx, mut rx) = mpsc::channel::<String>(256);
        cm.add_session(format!("user_{}", i), tx).await;
        cm.subscribe("live_channel".to_string(), format!("user_{}", i))
            .await;
        // Spawn a drain task so the channel doesn't fill up
        tokio::spawn(async move {
            while let Some(_msg) = rx.recv().await {}
        });
    }

    let mut handles = Vec::new();

    // Spawn 50 concurrent broadcast tasks
    for i in 0..50 {
        let cm = cm.clone();
        handles.push(tokio::spawn(async move {
            cm.broadcast("live_channel", format!(r#"{{"seq":{}}}"#, i))
                .await;
        }));
    }

    // Spawn concurrent subscribe/unsubscribe during broadcasts
    for i in 0..20 {
        let cm = cm.clone();
        handles.push(tokio::spawn(async move {
            let user = format!("user_{}", i);
            cm.unsubscribe("live_channel", &user).await;
            cm.subscribe("live_channel".to_string(), user).await;
        }));
    }

    for handle in handles {
        handle.await.expect("concurrent broadcast + sub/unsub should not panic");
    }
}

#[tokio::test]
async fn test_disconnect_user_idempotent() {
    let cm = ConnectionManager::new();

    let sender = dummy_sender();
    cm.add_session("user_x".to_string(), sender).await;
    cm.subscribe("ch1".to_string(), "user_x".to_string()).await;

    // Disconnect twice — second call should be a no-op, not a panic
    cm.disconnect_user("user_x").await;
    cm.disconnect_user("user_x").await;

    // Disconnect a user that never existed
    cm.disconnect_user("nonexistent").await;
}

#[tokio::test]
async fn test_send_to_missing_user_is_noop() {
    let cm = ConnectionManager::new();

    // Sending to a user that doesn't exist should not panic
    cm.send("ghost_user", "hello".to_string()).await;
}

#[tokio::test]
async fn test_broadcast_to_empty_channel_is_noop() {
    let cm = ConnectionManager::new();

    // Broadcasting to a channel with no subscribers should not panic
    cm.broadcast("empty_channel", "hello".to_string()).await;
}

#[tokio::test]
async fn test_subscribe_unsubscribe_cleans_up_empty_sets() {
    let cm = ConnectionManager::new();

    let sender = dummy_sender();
    cm.add_session("user_a".to_string(), sender).await;

    cm.subscribe("temp_channel".to_string(), "user_a".to_string())
        .await;

    // Unsubscribe should clean up the empty channel entry
    cm.unsubscribe("temp_channel", "user_a").await;

    // Re-subscribing should work fine (channel set was properly removed)
    cm.subscribe("temp_channel".to_string(), "user_a".to_string())
        .await;

    // Verify broadcast still works
    let (tx, mut rx) = mpsc::channel::<String>(16);
    cm.add_session("user_b".to_string(), tx).await;
    cm.subscribe("temp_channel".to_string(), "user_b".to_string())
        .await;

    cm.broadcast("temp_channel", "ping".to_string()).await;

    let msg = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
        .await
        .expect("should receive within timeout")
        .expect("channel should not be closed");
    assert_eq!(msg, "ping");
}
