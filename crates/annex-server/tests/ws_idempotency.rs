//! Tests that `ChannelService::send_message` is idempotent on
//! `client_request_id` per `(server_id, sender_pseudonym, client_request_id)`.
//!
//! See migration `035_ws_request_idempotency.sql` for the storage
//! contract, and `channel_service::SendOutcome` for the wire-level
//! contract the WS layer relies on to skip federated relay on replay.

mod common;

use std::sync::Arc;

use annex_channels::{add_member, create_channel, CreateChannelParams};
use annex_server::services::channel_service::SendOutcome;
use annex_server::services::ChannelService;
use annex_types::{ChannelType, FederationScope};

async fn seed_channel_and_member(
    pool: &annex_db::DbPool,
    server_id: i64,
    channel_id: &str,
    pseudonym: &str,
) {
    let conn = pool.get().expect("pool");
    // `channel_members` FK-references `platform_identities`, so the
    // pseudonym must be registered before it can join a channel.
    seed_platform_identity(&conn, server_id, pseudonym);
    create_channel(
        &conn,
        &CreateChannelParams {
            server_id,
            channel_id: channel_id.to_string(),
            name: "Idempotency Test".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        },
    )
    .expect("create_channel");
    add_member(&conn, server_id, channel_id, pseudonym).expect("add_member");
}

fn seed_platform_identity(conn: &rusqlite::Connection, server_id: i64, pseudonym: &str) {
    conn.execute(
        "INSERT OR IGNORE INTO platform_identities \
         (server_id, pseudonym_id, participant_type, active) \
         VALUES (?1, ?2, 'HUMAN', 1)",
        rusqlite::params![server_id, pseudonym],
    )
    .expect("seed platform_identity");
}

#[tokio::test]
async fn send_message_without_request_id_inserts_each_time() {
    let (_router, pool) = common::setup_test_app().await;
    let state = Arc::new(common::build_app_state(
        pool.clone(),
        annex_identity::MerkleTree::new(20).unwrap(),
        annex_types::ServerPolicy::default(),
    ));
    seed_channel_and_member(&pool, state.server_id, "chan-noreq", "psn-a").await;

    let svc = ChannelService::new(state.clone());

    let (msg1, _fed1, outcome1) = svc
        .send_message("psn-a", "chan-noreq", "hello".to_string(), None, None)
        .await
        .expect("first send");
    let (msg2, _fed2, outcome2) = svc
        .send_message("psn-a", "chan-noreq", "hello".to_string(), None, None)
        .await
        .expect("second send");

    assert_eq!(outcome1, SendOutcome::Inserted);
    assert_eq!(outcome2, SendOutcome::Inserted);
    assert_ne!(
        msg1.message_id, msg2.message_id,
        "no client_request_id ⇒ each call produces a distinct row"
    );
}

#[tokio::test]
async fn send_message_with_repeated_request_id_returns_original_message() {
    let (_router, pool) = common::setup_test_app().await;
    let state = Arc::new(common::build_app_state(
        pool.clone(),
        annex_identity::MerkleTree::new(20).unwrap(),
        annex_types::ServerPolicy::default(),
    ));
    seed_channel_and_member(&pool, state.server_id, "chan-idem", "psn-b").await;

    let svc = ChannelService::new(state.clone());
    let request_id = "client-req-1".to_string();

    let (msg1, _fed1, outcome1) = svc
        .send_message(
            "psn-b",
            "chan-idem",
            "first".to_string(),
            None,
            Some(request_id.clone()),
        )
        .await
        .expect("first send");
    // Even with different content payload, a repeated request_id MUST
    // return the original message — otherwise content can be silently
    // mutated under a replay.
    let (msg2, _fed2, outcome2) = svc
        .send_message(
            "psn-b",
            "chan-idem",
            "second (should be ignored)".to_string(),
            None,
            Some(request_id.clone()),
        )
        .await
        .expect("second send");

    assert_eq!(outcome1, SendOutcome::Inserted);
    assert_eq!(outcome2, SendOutcome::Replayed);
    assert_eq!(
        msg1.message_id, msg2.message_id,
        "replay returns the original message_id"
    );
    assert_eq!(
        msg2.content, "first",
        "replay does not rewrite the persisted content"
    );

    // Confirm only one row exists.
    let count: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE channel_id = ?1",
            ["chan-idem"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn send_message_request_id_scoped_per_sender() {
    let (_router, pool) = common::setup_test_app().await;
    let state = Arc::new(common::build_app_state(
        pool.clone(),
        annex_identity::MerkleTree::new(20).unwrap(),
        annex_types::ServerPolicy::default(),
    ));
    seed_channel_and_member(&pool, state.server_id, "chan-scope", "sender-1").await;
    {
        let conn = pool.get().unwrap();
        seed_platform_identity(&conn, state.server_id, "sender-2");
        add_member(&conn, state.server_id, "chan-scope", "sender-2").unwrap();
    }

    let svc = ChannelService::new(state.clone());
    let request_id = "shared-rid".to_string();

    let (msg1, _, outcome1) = svc
        .send_message(
            "sender-1",
            "chan-scope",
            "from one".to_string(),
            None,
            Some(request_id.clone()),
        )
        .await
        .expect("sender 1");
    let (msg2, _, outcome2) = svc
        .send_message(
            "sender-2",
            "chan-scope",
            "from two".to_string(),
            None,
            Some(request_id.clone()),
        )
        .await
        .expect("sender 2");

    assert_eq!(outcome1, SendOutcome::Inserted);
    assert_eq!(
        outcome2,
        SendOutcome::Inserted,
        "different senders sharing the same client_request_id MUST both insert — scope is per sender, not global"
    );
    assert_ne!(msg1.message_id, msg2.message_id);
}
