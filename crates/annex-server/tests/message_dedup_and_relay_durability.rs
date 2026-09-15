//! A message and the obligation to federate it must survive together, and an
//! idempotency key must actually mean "one message".
//!
//! Two defects, both about a window between a read and the write that depends
//! on it:
//!
//! 1. `send_message` looked up the client request id on the bare pooled
//!    connection, 46 lines before it opened a transaction. Two concurrent
//!    sends sharing (sender, request id) both saw nothing and both committed a
//!    message — two rows against one idempotency record, because the ledger
//!    insert is `OR IGNORE` and its rowcount was discarded.
//!
//! 2. The federation outbox row was written by a detached `tokio::spawn` fired
//!    AFTER the message committed, on a different connection, two task hops
//!    later. A crash, a pool failure or a tripped storage gate in that gap left
//!    a message persisted and broadcast locally, the sender told it had sent,
//!    and no peer ever seeing it. A retry did not heal it: the retry returns
//!    `Replayed`, and the caller skipped the relay on `Replayed` by design.
//!
//! The first needs a FILE-BACKED pool with more than one connection. The
//! standard `:memory:` harness has exactly one, so the two `spawn_blocking`
//! closures serialise and the race cannot occur — which is why this went
//! unnoticed. `pool.rs` documents that single-connection behaviour; a test
//! written against it would pass without ever exercising the defect.

use annex_channels::{create_channel, CreateChannelParams};
use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::middleware::RateLimiter;
use annex_server::services::channel_service::{ChannelService, SendOutcome};
use annex_server::{api_ws, AppState};
use annex_types::{AlignmentStatus, ChannelType, FederationScope, ServerPolicy};
use std::sync::{Arc, Mutex, RwLock};

/// A server backed by a real file, with a pool that can hand out more than one
/// connection — the only configuration in which two sends can actually race.
fn file_backed_state(federated: bool) -> (Arc<AppState>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("annex.db");
    let pool = create_pool(
        db_path.to_str().unwrap(),
        DbRuntimeSettings {
            // Large enough that every racer holds its own connection at the
            // same time. With a small pool the racers queue on `pool.get()`
            // and each one acquires a connection only AFTER the previous has
            // committed and released — so its lookup sees the winner's ledger
            // row and takes the replay branch. That is connection scarcity
            // masking the defect, not the defect being absent: measured at
            // pool_max_size = 4 the unfixed code passed this test.
            pool_max_size: 32,
            ..Default::default()
        },
    )
    .expect("pool");

    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [serde_json::to_string(&ServerPolicy::default()).unwrap()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO platform_identities \
             (server_id, pseudonym_id, participant_type, active) VALUES (1, 'sender-1', 'HUMAN', 1)",
            [],
        )
        .unwrap();
        create_channel(
            &conn,
            &CreateChannelParams {
                server_id: 1,
                channel_id: "chan-1".to_string(),
                name: "Chan".to_string(),
                channel_type: ChannelType::Text,
                topic: None,
                vrp_topic_binding: None,
                required_capabilities_json: None,
                agent_min_alignment: Some(AlignmentStatus::Aligned),
                retention_days: None,
                federation_scope: if federated {
                    FederationScope::Federated
                } else {
                    FederationScope::Local
                },
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO channel_members (server_id, channel_id, pseudonym_id) \
             VALUES (1, 'chan-1', 'sender-1')",
            [],
        )
        .unwrap();

        if federated {
            conn.execute(
                "INSERT INTO instances (base_url, public_key, label, status) \
                 VALUES ('https://peer.example', 'aa', 'Peer', 'ACTIVE')",
                [],
            )
            .unwrap();
            let instance_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO federation_agreements (
                    local_server_id, remote_instance_id, alignment_status, transfer_scope,
                    agreement_json, active
                ) VALUES (1, ?1, 'ALIGNED', 'FullKnowledge', '{}', 1)",
                rusqlite::params![instance_id],
            )
            .unwrap();
        }
    }

    let tree = {
        let conn = pool.get().unwrap();
        MerkleTree::restore(&conn, 20).unwrap()
    };

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(annex_identity::zk::generate_dummy_vkey()),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: Arc::new(RwLock::new("http://localhost:3000".to_string())),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::WebRtcConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper", "bark")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        invite_base_url: "https://monolithannex.com/invite".to_string(),
        ws_token_secret: Arc::new([0u8; 32]),
        voice_token_secret: Arc::new([0u8; 32]),
        federation_config: annex_server::config::FederationConfig {
            allow_private_peer_addresses: true,
            ..Default::default()
        },
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
        shutdown: Default::default(),
        metrics: Default::default(),
    };
    (Arc::new(state), dir)
}

/// The race, driven against a pool that can actually serve the racers.
///
/// `RACERS` concurrent sends rather than two, and the run is repeated: the
/// window between the old lookup and the old `BEGIN IMMEDIATE` is microseconds
/// wide, so a single pair frequently serialises by luck and a two-caller test
/// passed against the defective code. Sixteen racers reproduce it: measured on
/// the unfixed version at **2 message rows against 1 idempotency record** in
/// round 0, with two callers each reporting `Inserted` — the exact state the
/// report describes. On the fixed version every round is 1 and 1.
///
/// The pool size matters as much as the racer count, and for a reason that is
/// easy to get backwards — see `file_backed_state`.
///
/// Stated plainly because a probabilistic reproducer is a real cost: this test
/// can only ever say "the race did not happen this time", and the assertion it
/// makes is one the fix satisfies deterministically (the second racer blocks on
/// RESERVED and reads a snapshot containing the winner's ledger row). The
/// determinism is in the mechanism, not in the harness.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_sends_with_one_request_id_produce_one_message() {
    const RACERS: usize = 16;
    const ROUNDS: usize = 4;

    for round in 0..ROUNDS {
        let (state, _dir) = file_backed_state(false);
        let rid = format!("req-{round}");

        let sends: Vec<_> = (0..RACERS)
            .map(|_| {
                let svc = ChannelService::new(state.clone());
                let rid = rid.clone();
                tokio::spawn(async move {
                    svc.send_message("sender-1", "chan-1", "hello".to_string(), None, Some(rid))
                        .await
                })
            })
            .collect();

        let mut ids = std::collections::HashSet::new();
        let mut inserted = 0usize;
        for handle in sends {
            let (msg, _, outcome) = handle.await.expect("task").expect("send");
            ids.insert(msg.message_id);
            if outcome == SendOutcome::Inserted {
                inserted += 1;
            }
        }

        assert_eq!(
            ids.len(),
            1,
            "round {round}: every caller must be told about the SAME message — \
             an idempotency key that yields {} message ids has deduplicated \
             nothing",
            ids.len()
        );
        assert_eq!(
            inserted, 1,
            "round {round}: exactly one send may report Inserted"
        );

        let conn = state.pool.get().unwrap();
        let messages: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE channel_id = 'chan-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let ledger: i64 = conn
            .query_row("SELECT COUNT(*) FROM message_request_ids", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            (messages, ledger),
            (1, 1),
            "round {round}: N message rows against one idempotency record is \
             the exact state the lookup-outside-the-transaction produced"
        );
    }
}

/// Sequential replay still works — the transaction move must not have turned
/// the ordinary retry path into a second insert.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sequential_retry_returns_the_original_message() {
    let (state, _dir) = file_backed_state(false);
    let svc = ChannelService::new(state.clone());

    let (first, _, outcome_first) = svc
        .send_message(
            "sender-1",
            "chan-1",
            "hello".to_string(),
            None,
            Some("req-2".to_string()),
        )
        .await
        .expect("first send");
    let (second, _, outcome_second) = svc
        .send_message(
            "sender-1",
            "chan-1",
            "hello".to_string(),
            None,
            Some("req-2".to_string()),
        )
        .await
        .expect("retry");

    assert_eq!(first.message_id, second.message_id);
    assert_eq!(outcome_first, SendOutcome::Inserted);
    assert_eq!(outcome_second, SendOutcome::Replayed);
    assert_eq!(second.content, "hello", "the replay must return plaintext");
}

/// The outbox row exists the instant the message does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_federated_message_is_queued_in_the_same_transaction_that_stores_it() {
    let (state, _dir) = file_backed_state(true);
    let svc = ChannelService::new(state.clone());

    let (msg, is_federated, _) = svc
        .send_message(
            "sender-1",
            "chan-1",
            "federated hello".to_string(),
            None,
            None,
        )
        .await
        .expect("send");
    assert!(is_federated);

    // No sleep, no spawn, no polling: if the enqueue were still a detached
    // task this read would race it, and a test that slept would be asserting
    // the opposite of what matters.
    let conn = state.pool.get().unwrap();
    let queued: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM federation_outbox WHERE message_id = ?1",
            [&msg.message_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        queued, 1,
        "the federation obligation must commit with the message; a window \
         between them is a window in which the sender is told it sent and no \
         peer ever sees it"
    );
}

/// And the queued copy must not be a cleartext copy of the message body.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_queued_envelope_is_encrypted_at_rest() {
    let (state, _dir) = file_backed_state(true);
    let svc = ChannelService::new(state.clone());

    let secret = "the-body-that-must-not-be-readable";
    let (msg, _, _) = svc
        .send_message("sender-1", "chan-1", secret.to_string(), None, None)
        .await
        .expect("send");

    let conn = state.pool.get().unwrap();
    let stored: String = conn
        .query_row(
            "SELECT envelope_json FROM federation_outbox WHERE message_id = ?1",
            [&msg.message_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        !stored.contains(secret),
        "`messages.content` is encrypted at rest but `send_message` returns the \
         PLAINTEXT, and the relay copied that straight into the outbox — so the \
         at-rest guarantee held for every federated message except the copy \
         queued beside it, which also outlives the retention sweep"
    );

    // …and it must still be readable by the delivery worker, or encrypting it
    // has simply broken federation.
    let mut roundtrip = stored.clone();
    state.message_cipher().decrypt_in_place(&mut roundtrip);
    assert!(
        roundtrip.contains(secret),
        "the worker decrypts this before routing and POSTing it; if it does not \
         round-trip, no federated message is deliverable"
    );
}
