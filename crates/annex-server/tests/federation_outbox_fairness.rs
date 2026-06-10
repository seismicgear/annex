//! Integration test for the per-peer fairness cap in the federation
//! outbox worker (ADR-0008 amendment). One unreachable peer with a deep
//! backlog of due rows must not occupy the whole drain batch and starve
//! delivery to other peers.

use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{background::drain_outbox_batch, middleware::RateLimiter, AppState};
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
        trusted_proxy_depth: 0,
    }
}

/// A peer with a deep, older backlog (peer A) must not fill the whole
/// batch: peer B's single due row must be attempted in the same tick.
///
/// Both peers use RFC 5737 documentation IPs (public per the SSRF gate,
/// guaranteed unroutable), so every attempted row fails with a connect
/// error and records `attempts = 1`. Rows the batch never selected keep
/// `attempts = 0`, which is the observable this test asserts on.
///
/// Pre-fix behaviour: the batch SELECT was a global
/// `ORDER BY next_retry_at LIMIT batch`, so peer A's 12 older rows
/// monopolised a batch of 10 and peer B's row kept `attempts = 0`.
#[tokio::test]
async fn outbox_drain_does_not_let_one_peer_starve_others() {
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

    conn.execute(
        "INSERT INTO instances (base_url, public_key, label, status) \
         VALUES ('http://203.0.113.1:9', ?1, 'Peer A (down, deep backlog)', 'ACTIVE')",
        rusqlite::params![pubkey_hex],
    )
    .unwrap();
    let peer_a = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO instances (base_url, public_key, label, status) \
         VALUES ('http://203.0.113.2:9', ?1, 'Peer B (healthy-ish)', 'ACTIVE')",
        rusqlite::params![pubkey_hex],
    )
    .unwrap();
    let peer_b = conn.last_insert_rowid();

    // Peer A: 12 due rows, all older than peer B's row so a purely
    // global ORDER BY would select them first.
    for i in 0..12 {
        conn.execute(
            "INSERT INTO federation_outbox \
                 (peer_instance_id, message_id, envelope_json, status, next_retry_at) \
             VALUES (?1, ?2, '{}', 'pending', datetime('now', '-10 minutes'))",
            rusqlite::params![peer_a, format!("peer-a-msg-{i}")],
        )
        .unwrap();
    }

    // Peer B: a single due row, newer than all of peer A's.
    conn.execute(
        "INSERT INTO federation_outbox \
             (peer_instance_id, message_id, envelope_json, status, next_retry_at) \
         VALUES (?1, 'peer-b-msg-0', '{}', 'pending', datetime('now', '-1 minute'))",
        rusqlite::params![peer_b],
    )
    .unwrap();
    drop(conn);

    let state = Arc::new(build_state(pool.clone(), local_server_id));
    assert_eq!(
        state.federation_config.outbox_per_peer_batch, 8,
        "test assumes the default per-peer cap of 8"
    );

    // Batch of 10 < peer A's 12 due rows: without the per-peer cap the
    // batch would be 10 × peer A and 0 × peer B.
    drain_outbox_batch(state, 10)
        .await
        .expect("drain_outbox_batch failed");

    let conn = pool.get().unwrap();

    let peer_b_attempts: u32 = conn
        .query_row(
            "SELECT attempts FROM federation_outbox WHERE peer_instance_id = ?1",
            rusqlite::params![peer_b],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        peer_b_attempts, 1,
        "peer B's row must be attempted in the same tick despite peer A's older backlog"
    );

    let peer_a_attempted: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM federation_outbox \
             WHERE peer_instance_id = ?1 AND attempts > 0",
            rusqlite::params![peer_a],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        peer_a_attempted <= 8,
        "peer A must be capped at outbox_per_peer_batch rows per tick, got {peer_a_attempted}"
    );
    assert!(
        peer_a_attempted > 0,
        "the cap must not starve peer A entirely"
    );

    // Every attempted row stays pending (transient connect error, not
    // the SSRF gate) so the retry rotation continues normally.
    let failed_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM federation_outbox WHERE status != 'pending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(failed_rows, 0, "no row should be terminally failed");
}
