//! A message send must not fail because another connection wrote first.
//!
//! `channel_service::send_message` opened its transaction with
//! `conn.transaction()` — `BEGIN DEFERRED`. Inside it, `create_message`
//! reads (resolving the channel's retention days) before it writes. Under
//! WAL that read takes a snapshot, and the INSERT that follows has to
//! upgrade to a writer; if any other connection committed in between,
//! SQLite answers `SQLITE_BUSY_SNAPSHOT` *immediately*. The busy handler is
//! never called — waiting cannot resolve a snapshot conflict — so
//! `busy_timeout` is irrelevant and the send fails outright.
//!
//! The user was told "Failed to send message: internal error" and the
//! message sat in the column marked failed. `edit_message` and
//! `delete_message` were fixed for exactly this (the [F31] regression test in
//! annex-channels); sending was missed, and it is the one of the three that
//! every user does constantly.
//!
//! Found by the UI audit: a capture recorded a failed bubble and a composer
//! error as though they were the normal state, because the harness could not
//! tell a sent message from an optimistic one.

mod common;

use annex_channels::{add_member, create_channel, CreateChannelParams};
use annex_db::{create_pool, DbRuntimeSettings};
use annex_server::services::channel_service::ChannelService;
use annex_types::{ChannelType, FederationScope, ServerPolicy};
use rusqlite::Connection;
use std::sync::Arc;
use std::time::Duration;

/// A real file-backed database with room for two connections. The shared
/// `:memory:` harness clamps the pool to one connection, which serialises
/// everything and makes the conflict impossible to reproduce.
fn file_backed_state() -> (
    Arc<annex_server::AppState>,
    std::path::PathBuf,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("annex-send-immediate.sqlite");
    let pool = create_pool(
        db_path.to_str().unwrap(),
        DbRuntimeSettings {
            busy_timeout_ms: 5_000,
            pool_max_size: 4,
        },
    )
    .unwrap();

    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        let policy_json = serde_json::to_string(&ServerPolicy::default()).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
        create_channel(
            &conn,
            &CreateChannelParams {
                server_id: 1,
                channel_id: "chan-send".to_string(),
                name: "Send".to_string(),
                channel_type: ChannelType::Text,
                topic: None,
                vrp_topic_binding: None,
                required_capabilities_json: None,
                agent_min_alignment: None,
                retention_days: Some(7),
                federation_scope: FederationScope::Local,
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) \
             VALUES (1, 'alice', 'HUMAN', 1)",
            [],
        )
        .unwrap();
        add_member(&conn, 1, "chan-send", "alice").unwrap();
        // A message for the external writer to update, so its transaction
        // has actually written something the sender's snapshot would read.
        annex_channels::create_message(
            &conn,
            &annex_channels::CreateMessageParams {
                channel_id: "chan-send".to_string(),
                message_id: "seed-msg".to_string(),
                sender_pseudonym: "alice".to_string(),
                content: "seed".to_string(),
                reply_to_message_id: None,
            },
        )
        .unwrap();
    }

    let state = Arc::new(common::build_app_state(
        pool,
        annex_identity::MerkleTree::new(20).unwrap(),
        ServerPolicy::default(),
    ));
    (state, db_path, dir)
}

#[tokio::test]
async fn a_send_waits_for_a_concurrent_writer_instead_of_failing() {
    let (state, db_path, _dir) = file_backed_state();
    let svc = ChannelService::new(state.clone());

    // An external connection holds an IMMEDIATE transaction that has written.
    // A DEFERRED sender reads its pre-write snapshot and then cannot upgrade;
    // an IMMEDIATE sender simply waits at BEGIN.
    let outsider = Connection::open(&db_path).unwrap();
    outsider.busy_timeout(Duration::from_secs(5)).unwrap();
    outsider.execute_batch("BEGIN IMMEDIATE").unwrap();
    outsider
        .execute(
            "UPDATE messages SET content = 'touched' WHERE message_id = 'seed-msg'",
            [],
        )
        .unwrap();

    // Release it shortly, well inside the 5s busy timeout.
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        outsider.execute_batch("COMMIT").unwrap();
    });

    let result = svc
        .send_message("alice", "chan-send", "hello".to_string(), None, None)
        .await;

    releaser.join().unwrap();

    let (message, _federated, _outcome) = result.expect(
        "the send must wait for the other writer and then succeed — under BEGIN DEFERRED this \
         is SQLITE_BUSY_SNAPSHOT and the user is told \"internal error\"",
    );
    assert_eq!(message.content, "hello");
}
