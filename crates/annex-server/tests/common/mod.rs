//! Shared test harness for annex-server integration tests.
//!
//! Provides `setup_test_app()` and `load_vkey_or_dummy()` to eliminate
//! duplicated boilerplate across test files.

#![allow(dead_code)]

use annex_db::{create_pool, run_migrations, DbPool, DbRuntimeSettings};
use annex_identity::zk::{Bn254, VerifyingKey};
use annex_identity::MerkleTree;
use annex_server::api_link_preview::PreviewCache;
use annex_server::api_ws::ConnectionManager;
use annex_server::middleware::RateLimiter;
use annex_server::{app, AppState};
use annex_types::ServerPolicy;
use std::sync::{Arc, Mutex, RwLock};

/// Loads the real ZK verification key if available, otherwise falls back to a dummy.
pub fn load_vkey_or_dummy() -> Arc<VerifyingKey<Bn254>> {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest.join("../../zk/keys/membership_vkey.json");

    match std::fs::read_to_string(&path) {
        Ok(json) => {
            let vk =
                annex_identity::zk::parse_verification_key(&json).expect("failed to parse vkey");
            Arc::new(vk)
        }
        Err(_) => Arc::new(annex_identity::zk::generate_dummy_vkey()),
    }
}

/// Creates a test app with in-memory SQLite, default policy, and a seeded server row.
///
/// Returns `(Router, DbPool)` ready for use with `tower::ServiceExt::oneshot()`.
pub async fn setup_test_app() -> (axum::Router, DbPool) {
    setup_test_app_with_policy(ServerPolicy::default()).await
}

/// Creates a test app with a custom `ServerPolicy`.
pub async fn setup_test_app_with_policy(policy: ServerPolicy) -> (axum::Router, DbPool) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        let policy_json = serde_json::to_string(&policy).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
    }

    let tree = MerkleTree::new(20).unwrap();
    let state = build_app_state(pool.clone(), tree, policy);
    (app(state), pool)
}

/// Builds an `AppState` with sensible test defaults.
pub fn build_app_state(pool: DbPool, tree: MerkleTree, policy: ServerPolicy) -> AppState {
    AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey_or_dummy(),
        // Default test harness disables v2; tests that exercise v2 should
        // construct an AppState with this field set explicitly.
        membership_vkey_v2: None,
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: Arc::new(RwLock::new("http://localhost:3000".to_string())),
        policy: Arc::new(RwLock::new(policy)),
        rate_limiter: RateLimiter::new(),
        connection_manager: ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::WebRtcConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper", "bark")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        invite_base_url: "https://monolithannex.com/invite".to_string(),
        ws_token_secret: Arc::new([0u8; 32]),
    }
}
