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

/// Like [`setup_test_app`], but also hands back the storage gate.
///
/// `app(state)` consumes the `AppState`, so a test that needs to drive a
/// runtime condition — the storage gate blocking writes, say — has no way to
/// reach it afterwards. `storage_health` is an `Arc`, so cloning the handle
/// out before the state is consumed costs nothing and keeps the app identical
/// to the one every other test builds.
#[allow(dead_code)]
pub async fn setup_test_app_with_storage_health() -> (
    axum::Router,
    DbPool,
    std::sync::Arc<annex_server::storage_health::StorageHealth>,
) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        let policy_json = serde_json::to_string(&ServerPolicy::default()).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
    }

    let tree = MerkleTree::new(20).unwrap();
    let state = build_app_state(pool.clone(), tree, ServerPolicy::default());
    let storage_health = state.storage_health.clone();
    (app(state), pool, storage_health)
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
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
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
        voice_token_secret: Arc::new([0u8; 32]),
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
        shutdown: Default::default(),
    }
}

/// Fixture for the session-revocation tests.
///
/// It differs from [`setup_test_app`] in exactly one way that matters:
/// `enforce_zk_proofs` is **on**. With it off, `auth_middleware` reads a
/// `Bearer` value as a raw pseudonym, no token is parsed, and there is no
/// epoch to check — so revocation is inert by construction. That is not a
/// defect; a deployment that accepts a bare pseudonym as a credential has
/// nothing revocable in the first place. It is the reason the production
/// profile refuses to boot with `enforce_zk_proofs=false`
/// (`config::validate_zk_enforcement_for_build_profile`), and the reason
/// these tests would pass vacuously against the default harness.
pub mod revocation {
    use super::*;
    use annex_db::DbPool;

    pub struct Fixture {
        pub app: axum::Router,
        /// The same database the router serves from, so a test can revoke
        /// out-of-band and see the effect on the next request.
        pub pool: DbPool,
        pub server_id: i64,
        /// A member who is registered, active and unrevoked.
        pub pseudonym: String,
        /// A session token for `pseudonym`, minted at epoch 0.
        pub token: String,
        /// Copied out of the `AppState` before `app()` consumes it, so the
        /// tests sign with the server's real secret rather than a hard-coded
        /// copy that could drift from `build_app_state`.
        pub ws_token_secret: Arc<[u8; 32]>,
    }

    /// Builds the app, registers one member, and mints them a token.
    pub async fn fixture() -> Fixture {
        let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
        {
            let conn = pool.get().unwrap();
            run_migrations(&conn).unwrap();
            let policy_json = serde_json::to_string(&ServerPolicy::default()).unwrap();
            conn.execute(
                "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
                [policy_json],
            )
            .unwrap();
        }

        let tree = MerkleTree::new(20).unwrap();
        let mut state = build_app_state(pool.clone(), tree, ServerPolicy::default());
        state.enforce_zk_proofs = true;
        let server_id = state.server_id;
        let ws_token_secret = state.ws_token_secret.clone();

        let pseudonym = add_member(&pool, server_id, "aaaaaaaaaaaa");
        let token = annex_server::api_ws::generate_session_token(
            &pseudonym,
            &ws_token_secret,
            annex_server::api_ws::SESSION_TOKEN_TTL_SECS,
            0,
        );

        Fixture {
            app: app(state),
            pool,
            server_id,
            pseudonym,
            token,
            ws_token_secret,
        }
    }

    /// Registers an active human member and returns their pseudonym.
    ///
    /// Takes the pseudonym as a parameter rather than generating one: a test
    /// that asserts two identities are independent needs to name both.
    pub fn add_member(pool: &DbPool, server_id: i64, pseudonym: &str) -> String {
        let conn = pool.get().unwrap();
        annex_identity::create_platform_identity(
            &conn,
            server_id,
            pseudonym,
            annex_types::RoleCode::Human,
        )
        .expect("member should register");
        pseudonym.to_string()
    }

    /// Bumps the identity's token epoch, returning its new value.
    ///
    /// Calls the same function the admin route does, rather than issuing the
    /// `UPDATE` inline — a test that writes its own SQL would keep passing if
    /// `revoke_identity_sessions` were deleted.
    pub fn revoke(pool: &DbPool, server_id: i64, pseudonym: &str) -> i64 {
        let mut conn = pool.get().unwrap();
        annex_identity::platform::revoke_identity_sessions(&mut conn, server_id, pseudonym)
            .expect("revoke should succeed for a registered identity")
    }
}
