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
        assert_eq!(state.message_cipher().decrypt(&stored), secret);
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
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].content, secret);

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
    assert!(misses.is_empty());
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
        assert_eq!(state.message_cipher().decrypt(&stored), "before");
    }

    // But it reads back as plaintext through the edits API.
    let edits = svc
        .get_message_edits(&member(state.server_id, "psn"), "chan", &sent.message_id)
        .await
        .expect("edits");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].old_content, "before");
}
