//! End-to-end proof of message encryption at rest for non-E2E channels:
//! a message sent through `ChannelService` is stored as ciphertext in SQLite,
//! yet reads back as plaintext through history and is found by search — so the
//! server keeps working while a stolen database file stays unreadable.

mod common;

use std::sync::Arc;

use annex_channels::{add_member, create_channel, CreateChannelParams};
use annex_identity::platform::PlatformIdentity;
use annex_server::services::ChannelService;
use annex_types::{ChannelType, FederationScope, RoleCode};
use axum::http::HeaderMap;

fn member(server_id: i64, pseudonym: &str) -> PlatformIdentity {
    PlatformIdentity {
        id: 0,
        server_id,
        pseudonym_id: pseudonym.to_string(),
        participant_type: RoleCode::Human,
        can_voice: false,
        can_moderate: false,
        can_invite: false,
        can_federate: false,
        can_bridge: false,
        active: true,
        token_epoch: 0,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

async fn seed(pool: &annex_db::DbPool, server_id: i64, channel_id: &str, pseudonym: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO platform_identities \
         (server_id, pseudonym_id, participant_type, active) VALUES (?1, ?2, 'HUMAN', 1)",
        rusqlite::params![server_id, pseudonym],
    )
    .unwrap();
    create_channel(
        &conn,
        &CreateChannelParams {
            server_id,
            channel_id: channel_id.to_string(),
            name: "At Rest".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        },
    )
    .unwrap();
    add_member(&conn, server_id, channel_id, pseudonym).unwrap();
}

#[tokio::test]
async fn message_is_encrypted_at_rest_but_reads_back_plaintext() {
    let (_router, pool) = common::setup_test_app().await;
    let state = Arc::new(common::build_app_state(
        pool.clone(),
        annex_identity::MerkleTree::new(20).unwrap(),
        annex_types::ServerPolicy::default(),
    ));
    seed(&pool, state.server_id, "chan", "psn").await;
    let svc = ChannelService::new(state.clone());

    let secret = "launch codes: 0000 — do not leak";
    let (sent, _fed, _outcome) = svc
        .send_message("psn", "chan", secret.to_string(), None, None)
        .await
        .expect("send");
    // The value returned to the caller (for broadcast/relay) is plaintext.
    assert_eq!(sent.content, secret);

    // 1. The raw SQLite column is ciphertext, not the plaintext.
    {
        let conn = pool.get().unwrap();
        let stored: String = conn
            .query_row(
                "SELECT content FROM messages WHERE channel_id = 'chan'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(stored, secret, "content must not be stored in clear");
        assert!(
            !stored.contains("launch codes"),
            "ciphertext leaked plaintext fragment"
        );
        // And the server's key recovers it.
        assert_eq!(state.message_cipher().decrypt(&stored).unwrap(), secret);
    }

    // 2. History reads back as plaintext (server decrypts transparently).
    let history = svc
        .get_history(
            &member(state.server_id, "psn"),
            &HeaderMap::new(),
            "chan",
            None,
            None,
        )
        .await
        .expect("history");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, secret);

    // 3. Search still works over encrypted-at-rest content (scan + decrypt).
    let hits = svc
        .search_messages(
            &member(state.server_id, "psn"),
            &HeaderMap::new(),
            "launch codes".to_string(),
            Some("chan".to_string()),
            None,
        )
        .await
        .expect("search");
    assert_eq!(hits.results.len(), 1);
    assert_eq!(hits.results[0].content, secret);
    assert!(
        hits.complete,
        "a channel holding one message was read to its end"
    );

    // A non-matching query returns nothing.
    let misses = svc
        .search_messages(
            &member(state.server_id, "psn"),
            &HeaderMap::new(),
            "nonexistent".to_string(),
            Some("chan".to_string()),
            None,
        )
        .await
        .expect("search miss");
    assert!(misses.results.is_empty());
    assert!(
        misses.complete,
        "this miss is a fact about the channel, not about how far we read"
    );
}

#[tokio::test]
async fn edit_history_old_content_is_also_encrypted_at_rest() {
    let (_router, pool) = common::setup_test_app().await;
    let state = Arc::new(common::build_app_state(
        pool.clone(),
        annex_identity::MerkleTree::new(20).unwrap(),
        annex_types::ServerPolicy::default(),
    ));
    seed(&pool, state.server_id, "chan", "psn").await;
    let svc = ChannelService::new(state.clone());

    let (sent, _f, _o) = svc
        .send_message("psn", "chan", "before".to_string(), None, None)
        .await
        .unwrap();
    svc.edit_message("psn", "chan", &sent.message_id, "after")
        .await
        .expect("edit");

    // The audit trail (message_edits.old_content) is stored encrypted.
    {
        let conn = pool.get().unwrap();
        let stored: String = conn
            .query_row(
                "SELECT old_content FROM message_edits WHERE message_id = ?1",
                [&sent.message_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(stored, "before");
        assert_eq!(state.message_cipher().decrypt(&stored).unwrap(), "before");
    }

    // But it reads back as plaintext through the edits API.
    let edits = svc
        .get_message_edits(&member(state.server_id, "psn"), "chan", &sent.message_id)
        .await
        .expect("edits");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].old_content, "before");
}

#[tokio::test]
async fn corrupted_message_refuses_read_search_replay_and_edit_without_mutation() {
    let (_router, pool) = common::setup_test_app().await;
    let state = Arc::new(common::build_app_state(
        pool.clone(),
        annex_identity::MerkleTree::new(20).unwrap(),
        annex_types::ServerPolicy::default(),
    ));
    seed(&pool, state.server_id, "corrupt", "author").await;
    let svc = ChannelService::new(state.clone());
    let (sent, _, _) = svc
        .send_message(
            "author",
            "corrupt",
            "original".into(),
            None,
            Some("request-1".into()),
        )
        .await
        .unwrap();
    let corrupt = "\u{1}ar1:broken-ciphertext";
    pool.get()
        .unwrap()
        .execute(
            "UPDATE messages SET content=?1 WHERE message_id=?2",
            rusqlite::params![corrupt, sent.message_id],
        )
        .unwrap();
    let identity = member(state.server_id, "author");
    assert!(svc
        .get_history(&identity, &HeaderMap::new(), "corrupt", None, None)
        .await
        .is_err());
    assert!(svc
        .search_messages(
            &identity,
            &HeaderMap::new(),
            "broken".into(),
            Some("corrupt".into()),
            None
        )
        .await
        .is_err());
    assert!(svc
        .send_message(
            "author",
            "corrupt",
            "new body".into(),
            None,
            Some("request-1".into())
        )
        .await
        .is_err());
    assert!(svc
        .edit_message("author", "corrupt", &sent.message_id, "replacement")
        .await
        .is_err());
    let conn = pool.get().unwrap();
    let stored: String = conn
        .query_row(
            "SELECT content FROM messages WHERE message_id=?1",
            [&sent.message_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        stored, corrupt,
        "corrupt bytes must remain recoverable, never be overwritten by a successful edit"
    );
    let edits: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_edits WHERE message_id=?1",
            [&sent.message_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(edits, 0);
    let requests: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_request_ids WHERE sender_pseudonym='author'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(requests, 1);
}

#[tokio::test]
async fn corrupted_edit_history_refuses_to_return_a_successful_revision() {
    let (_router, pool) = common::setup_test_app().await;
    let state = Arc::new(common::build_app_state(
        pool.clone(),
        annex_identity::MerkleTree::new(20).unwrap(),
        annex_types::ServerPolicy::default(),
    ));
    seed(&pool, state.server_id, "revisions", "author").await;
    let svc = ChannelService::new(state.clone());
    let (sent, _, _) = svc
        .send_message("author", "revisions", "original".into(), None, None)
        .await
        .unwrap();
    svc.edit_message("author", "revisions", &sent.message_id, "revision")
        .await
        .unwrap();
    pool.get()
        .unwrap()
        .execute(
            "UPDATE message_edits SET old_content=?1 WHERE message_id=?2",
            rusqlite::params!["\u{1}ar1:AAAA", sent.message_id],
        )
        .unwrap();
    assert!(svc
        .get_message_edits(
            &member(state.server_id, "author"),
            "revisions",
            &sent.message_id
        )
        .await
        .is_err());
    let history = svc
        .get_history(
            &member(state.server_id, "author"),
            &HeaderMap::new(),
            "revisions",
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        history[0].content, "revision",
        "unaffected current content remains usable"
    );
}
