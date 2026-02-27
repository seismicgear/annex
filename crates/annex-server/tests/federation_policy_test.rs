use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::{
    zk::{G1Affine, G2Affine, VerifyingKey},
    MerkleTree,
};
use annex_server::{
    api_ws::ConnectionManager, middleware::RateLimiter, policy::recalculate_federation_agreements,
    AppState,
};
use annex_types::ServerPolicy;
use annex_vrp::{VrpAnchorSnapshot, VrpCapabilitySharingContract, VrpFederationHandshake};
use std::sync::{Arc, Mutex, RwLock};
use tempfile::NamedTempFile;
use tokio::sync::broadcast;

fn load_dummy_vkey() -> Arc<VerifyingKey<annex_identity::zk::Bn254>> {
    let g1 = G1Affine::default();
    let g2 = G2Affine::default();

    let vk = VerifyingKey {
        alpha_g1: g1,
        beta_g2: g2,
        gamma_g2: g2,
        delta_g2: g2,
        gamma_abc_g1: vec![g1; 1],
    };

    Arc::new(vk)
}

#[tokio::test]
async fn test_recalculate_federation_agreements() {
    let temp_file = NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap().to_string();

    let pool = create_pool(&db_path, DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // 1. Create Server and Instance
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'test', 'Test', '{}')",
        [],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO instances (id, base_url, public_key, label, status) VALUES (10, 'http://remote.com', 'pubkey', 'Remote', 'ACTIVE')",
        [],
    )
    .unwrap();

    // 2. Prepare Initial State (Aligned)
    let anchor = VrpAnchorSnapshot::new(&[], &[]).unwrap(); // Matches default policy (empty)
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec!["federation".to_string()],
        redacted_topics: vec![],
    };

    let initial_policy = ServerPolicy {
        federation_enabled: true,
        ..Default::default()
    };

    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };
    let handshake_json = serde_json::to_string(&handshake).unwrap();

    // Insert Agreement
    conn.execute(
        "INSERT INTO federation_agreements (
            local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, remote_handshake_json, active
        ) VALUES (1, 10, 'ALIGNED', 'REFLECTION_SUMMARIES_ONLY', '{}', ?1, 1)",
        [&handshake_json],
    )
    .unwrap();

    drop(conn);

    let policy_lock = Arc::new(RwLock::new(initial_policy.clone()));

    let state = Arc::new(AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(MerkleTree::new(20).unwrap())),
        membership_vkey: load_dummy_vkey(),
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: std::sync::Arc::new(std::sync::RwLock::new(
            "http://localhost:3000".to_string(),
        )),
        policy: policy_lock.clone(),
        rate_limiter: RateLimiter::new(),
        connection_manager: ConnectionManager::new(),
        presence_tx: broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::LiveKitConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper", "bark")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
    });

    // 3. Verify Initial State (No change expected)
    recalculate_federation_agreements(state.clone())
        .await
        .unwrap();

    let conn = pool.get().unwrap();
    let status: String = conn
        .query_row(
            "SELECT alignment_status FROM federation_agreements WHERE remote_instance_id = 10",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "ALIGNED");

    // 4. Change Policy to introduce Conflict
    {
        let mut p = policy_lock.write().unwrap();
        p.principles.push("We love Rust".to_string());
    }

    drop(conn);

    // 5. Recalculate
    recalculate_federation_agreements(state.clone())
        .await
        .unwrap();

    // 6. Verify Conflict
    let conn = pool.get().unwrap();
    let (status, active): (String, bool) = conn
        .query_row(
            "SELECT alignment_status, active FROM federation_agreements WHERE remote_instance_id = 10",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(status, "CONFLICT");
    assert!(!active); // Should be deactivated
}
