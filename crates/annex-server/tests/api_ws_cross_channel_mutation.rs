//! `IncomingMessage::{EditMessage, DeleteMessage}` — the two identifiers must
//! refer to the same place.
//!
//! Both take a `channel_id` and a `message_id` from the client, independently.
//! `require_membership` was checked against the channel; the mutation was
//! keyed on the message alone. Same shape as the edit-history IDOR, and the
//! second instance of it in this codebase, which is what makes it worth a
//! dedicated test file rather than a case buried in an existing one.
//!
//! Ownership is still enforced inside `annex_channels::edit_message` and
//! `delete_message`, so this never allowed touching somebody else's content.
//! What it allowed was touching your OWN content in a channel you are not in
//! — one you left, or were removed from — by naming a channel you are in. The
//! rule being defeated is "you cannot change anything in a channel you are not
//! a member of", and a removed user editing their history there is exactly the
//! case that rule exists for.
//!
//! The federation half has no ownership mitigation at all: `is_federated` was
//! read from the channel the caller NAMED, so an edit to a message in a
//! federated channel, submitted under a local channel id, was never relayed.
//! Peers kept the old text, the servers diverged, and nothing logged it.

mod common;

use annex_channels::{add_member, create_channel, CreateChannelParams};
use annex_server::services::channel_service::ChannelService;
use annex_types::{ChannelType, FederationScope};
use std::sync::Arc;

async fn seed_channel(
    pool: &annex_db::DbPool,
    server_id: i64,
    channel_id: &str,
    scope: FederationScope,
    members: &[&str],
) {
    let conn = pool.get().unwrap();
    create_channel(
        &conn,
        &CreateChannelParams {
            server_id,
            channel_id: channel_id.to_string(),
            name: channel_id.to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: scope,
        },
    )
    .unwrap();
    for m in members {
        conn.execute(
            "INSERT OR IGNORE INTO platform_identities \
             (server_id, pseudonym_id, participant_type, active) VALUES (?1, ?2, 'HUMAN', 1)",
            rusqlite::params![server_id, m],
        )
        .unwrap();
        add_member(&conn, server_id, channel_id, m).unwrap();
    }
}

async fn service() -> (Arc<annex_server::AppState>, annex_db::DbPool) {
    let (_router, pool) = common::setup_test_app().await;
    let state = Arc::new(common::build_app_state(
        pool.clone(),
        annex_identity::MerkleTree::new(20).unwrap(),
        annex_types::ServerPolicy::default(),
    ));
    (state, pool)
}

#[tokio::test]
async fn an_edit_cannot_name_one_channel_and_target_a_message_in_another() {
    let (state, pool) = service().await;
    let svc = ChannelService::new(state.clone());

    // `alice` posts in `left-behind`, then is removed from it. She remains a
    // member of `current`.
    seed_channel(
        &pool,
        state.server_id,
        "left-behind",
        FederationScope::Local,
        &["alice"],
    )
    .await;
    seed_channel(
        &pool,
        state.server_id,
        "current",
        FederationScope::Local,
        &["alice"],
    )
    .await;

    let (sent, _fed, _out) = svc
        .send_message(
            "alice",
            "left-behind",
            "the original text".to_string(),
            None,
            None,
        )
        .await
        .expect("send");

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "DELETE FROM channel_members WHERE channel_id = 'left-behind' AND pseudonym_id = 'alice'",
            [],
        )
        .unwrap();
    }

    // Membership passes for `current`; the message lives in `left-behind`.
    let result = svc
        .edit_message(
            "alice",
            "current",
            &sent.message_id,
            "rewritten after removal",
        )
        .await;

    assert!(
        result.is_err(),
        "a removed member edited their message in a channel they are not in",
    );

    // `edited_at` rather than the body: content is encrypted at rest, so
    // asserting on the plaintext means asserting on the cipher's prefix,
    // which is not what this test is about.
    let conn = pool.get().unwrap();
    let edited_at: Option<String> = conn
        .query_row(
            "SELECT edited_at FROM messages WHERE message_id = ?1",
            [&sent.message_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(edited_at.is_none(), "the message was edited anyway");
}

#[tokio::test]
async fn a_delete_cannot_name_one_channel_and_target_a_message_in_another() {
    let (state, pool) = service().await;
    let svc = ChannelService::new(state.clone());

    seed_channel(
        &pool,
        state.server_id,
        "left-behind",
        FederationScope::Local,
        &["alice"],
    )
    .await;
    seed_channel(
        &pool,
        state.server_id,
        "current",
        FederationScope::Local,
        &["alice"],
    )
    .await;

    let (sent, _fed, _out) = svc
        .send_message("alice", "left-behind", "still here".to_string(), None, None)
        .await
        .expect("send");

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "DELETE FROM channel_members WHERE channel_id = 'left-behind' AND pseudonym_id = 'alice'",
            [],
        )
        .unwrap();
    }

    let result = svc
        .delete_message("alice", "current", &sent.message_id)
        .await;

    assert!(
        result.is_err(),
        "a removed member deleted their message in a channel they are not in",
    );

    let conn = pool.get().unwrap();
    let deleted_at: Option<String> = conn
        .query_row(
            "SELECT deleted_at FROM messages WHERE message_id = ?1",
            [&sent.message_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(deleted_at.is_none(), "the message was deleted anyway");
}

/// The federation half, which ownership never mitigated.
///
/// `is_federated` decided whether an edit is relayed to peers, and it was read
/// from the channel the caller named rather than the one the message is in. A
/// member of both a local and a federated channel could edit a federated
/// message under the local channel's id and the edit would never leave the
/// server — peers keep the old text and the two diverge silently.
#[tokio::test]
async fn the_federation_decision_follows_the_message_not_the_claim() {
    let (state, pool) = service().await;
    let svc = ChannelService::new(state.clone());

    seed_channel(
        &pool,
        state.server_id,
        "fed",
        FederationScope::Federated,
        &["alice"],
    )
    .await;
    seed_channel(
        &pool,
        state.server_id,
        "local",
        FederationScope::Local,
        &["alice"],
    )
    .await;

    let (sent, fed_on_send, _out) = svc
        .send_message("alice", "fed", "federated content".to_string(), None, None)
        .await
        .expect("send");
    assert!(fed_on_send, "the fixture channel is not federated");

    // Editing the federated message while claiming the local channel must not
    // succeed at all — and therefore cannot report `is_federated = false`.
    let result = svc
        .edit_message("alice", "local", &sent.message_id, "quietly changed")
        .await;
    assert!(
        result.is_err(),
        "an edit to a federated message was accepted under a local channel id, \
         which is what made it skip the relay",
    );

    // The honest path still relays.
    let (_msg, is_federated) = svc
        .edit_message("alice", "fed", &sent.message_id, "openly changed")
        .await
        .expect("editing in the real channel must still work");
    assert!(
        is_federated,
        "a genuine federated edit stopped being relayed"
    );
}
