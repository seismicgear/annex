//! Integration tests for the dequeue-time SSRF gate in the
//! federation-outbox worker. The enqueue-time gate lives in
//! `services::federation_service::relay_message` (covered indirectly by
//! the message-relay tests); the dequeue-time gate is the defence in
//! depth introduced by [F33] for the case where a peer's `base_url`
//! changes to a private host AFTER the outbox row was written.

use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, background::drain_outbox_batch, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use std::sync::{Arc, Mutex, RwLock};

fn load_dummy_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    Arc::new(annex_identity::zk::generate_dummy_vkey())
}

fn build_state(pool: annex_db::DbPool, local_server_id: i64) -> AppState {
    let tree = MerkleTree::new(20).unwrap();
    let signing_key = SigningKey::generate(&mut OsRng);

    AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_dummy_vkey(),
        membership_vkey_v2: None,
        server_id: local_server_id,
        signing_key: Arc::new(signing_key),
        public_url: Arc::new(RwLock::new("http://localhost:3000".to_string())),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
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
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: Arc::new(annex_server::storage_health::StorageHealth::new()),
    }
}

/// Drain skips and terminally-fails outbox rows whose paired peer
/// `base_url` resolves to a private/reserved host at dequeue time.
///
/// Scenario: an admin (or an attacker who compromised an admin
/// account) edits the `instances.base_url` of an existing peer to
/// `http://127.0.0.1:9999` AFTER the outbox row was written. Without
/// the dequeue gate the worker would post the signed federation
/// envelope to localhost.
#[tokio::test]
async fn outbox_worker_refuses_to_post_to_private_peer_url() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    let policy_json = serde_json::to_string(&ServerPolicy::default()).unwrap();
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local Server', ?1)",
        rusqlite::params![policy_json],
    )
    .unwrap();
    let local_server_id = conn.last_insert_rowid();

    // Peer was originally registered with a private URL. We bypass
    // the enqueue-time gate by inserting directly — this models the
    // "URL was changed after enqueue" case without needing to twiddle
    // wall-clock timing.
    let signing_key = SigningKey::generate(&mut OsRng);
    let pubkey_hex = hex::encode(signing_key.verifying_key().as_bytes());
    conn.execute(
        "INSERT INTO instances (base_url, public_key, label, status) \
         VALUES ('http://127.0.0.1:9999', ?1, 'Peer that became local', 'ACTIVE')",
        rusqlite::params![pubkey_hex],
    )
    .unwrap();
    let peer_id = conn.last_insert_rowid();

    // Write a synthetic outbox row that's due now.
    let envelope_json = "{\"message_id\":\"msg-private-1\"}".to_string();
    conn.execute(
        "INSERT INTO federation_outbox \
         (peer_instance_id, message_id, envelope_json, status, attempts, next_retry_at) \
         VALUES (?1, ?2, ?3, 'pending', 0, datetime('now', '-1 minute'))",
        rusqlite::params![peer_id, "msg-private-1", envelope_json],
    )
    .unwrap();
    drop(conn);

    // App is constructed so the test compiles against the same
    // factory used in production; we don't actually issue requests.
    let state = Arc::new(build_state(pool.clone(), local_server_id));
    let _ = app((*state).clone());

    drain_outbox_batch(state, 32).await.expect("drain succeeds");

    // Row must be marked terminally failed with the SSRF gate
    // attribution, not 'delivered' and not still 'pending'.
    let conn = pool.get().unwrap();
    let (status, last_error): (String, Option<String>) = conn
        .query_row(
            "SELECT status, last_error FROM federation_outbox WHERE message_id = ?1",
            rusqlite::params!["msg-private-1"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        status, "failed",
        "private peer URL must trigger terminal failure, not retry"
    );
    let err = last_error.unwrap_or_default();
    assert!(
        err.contains("private/reserved"),
        "last_error should attribute the failure to the SSRF gate; got: {err}"
    );
}

/// Drain leaves outbox rows whose peer URL is genuinely public
/// alone — the only state change is the bookkeeping done by the HTTP
/// POST path. Used as a negative control: it asserts the gate fires
/// for private URLs only, not for *every* row.
///
/// We point the peer at a host that won't resolve to confirm the
/// row was at least attempted (attempts bumped to 1) while not
/// marked failed by the SSRF gate.
#[tokio::test]
async fn outbox_worker_does_not_drop_public_peer_url() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    let policy_json = serde_json::to_string(&ServerPolicy::default()).unwrap();
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local Server', ?1)",
        rusqlite::params![policy_json],
    )
    .unwrap();
    let local_server_id = conn.last_insert_rowid();

    let signing_key = SigningKey::generate(&mut OsRng);
    let pubkey_hex = hex::encode(signing_key.verifying_key().as_bytes());
    // RFC 5737 documentation IP — guaranteed not to resolve.
    conn.execute(
        "INSERT INTO instances (base_url, public_key, label, status) \
         VALUES ('http://203.0.113.1:1', ?1, 'Public peer', 'ACTIVE')",
        rusqlite::params![pubkey_hex],
    )
    .unwrap();
    let peer_id = conn.last_insert_rowid();

    let envelope_json = "{\"message_id\":\"msg-public-1\"}".to_string();
    conn.execute(
        "INSERT INTO federation_outbox \
         (peer_instance_id, message_id, envelope_json, status, attempts, next_retry_at) \
         VALUES (?1, ?2, ?3, 'pending', 0, datetime('now', '-1 minute'))",
        rusqlite::params![peer_id, "msg-public-1", envelope_json],
    )
    .unwrap();
    drop(conn);

    let state = Arc::new(build_state(pool.clone(), local_server_id));
    drain_outbox_batch(state, 32).await.expect("drain succeeds");

    let conn = pool.get().unwrap();
    let (status, last_error): (String, Option<String>) = conn
        .query_row(
            "SELECT status, last_error FROM federation_outbox WHERE message_id = ?1",
            rusqlite::params!["msg-public-1"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    // Status stays 'pending' (HTTP attempt failed transiently) and the
    // SSRF gate's specific error message must NOT be the recorded one.
    assert_eq!(
        status, "pending",
        "public peer URL must not be marked failed by the SSRF gate"
    );
    let err = last_error.unwrap_or_default();
    assert!(
        !err.contains("private/reserved"),
        "public peer URL must not trip the SSRF gate; got: {err}"
    );
}
