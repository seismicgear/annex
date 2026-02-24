use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::{
    zk::{G1Affine, G2Affine, VerifyingKey},
    MerkleTree,
};
use annex_server::{api_ws::ConnectionManager, app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use annex_vrp::{VrpAnchorSnapshot, VrpCapabilitySharingContract, VrpFederationHandshake};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tempfile::TempDir;
use tower::ServiceExt;

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
async fn test_update_policy_and_recalculate() {
    // 1. Setup
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let pool = create_pool(db_path.to_str().unwrap(), DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Insert Server
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'test', 'Test', '{}')",
        [],
    )
    .unwrap();

    // Insert Moderator Identity
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active)
         VALUES (1, 'mod_user', 'HUMAN', 1, 1)",
        [],
    ).unwrap();

    // Insert Instance (Federation Peer)
    conn.execute(
        "INSERT INTO instances (id, base_url, public_key, label, status) VALUES (10, 'http://remote.com', 'pubkey', 'Remote', 'ACTIVE')",
        [],
    ).unwrap();

    // Insert Federation Agreement (ALIGNED initially)
    let anchor = VrpAnchorSnapshot::new(&[], &[]).unwrap();
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec!["federation".to_string()],
        redacted_topics: vec![],
    };
    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };
    let handshake_json = serde_json::to_string(&handshake).unwrap();

    conn.execute(
        "INSERT INTO federation_agreements (
            local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, remote_handshake_json, active
        ) VALUES (1, 10, 'ALIGNED', 'REFLECTION_SUMMARIES_ONLY', '{}', ?1, 1)",
        [&handshake_json],
    ).unwrap();

    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let initial_policy = ServerPolicy {
        federation_enabled: true,
        ..Default::default()
    };
    let policy_lock = Arc::new(RwLock::new(initial_policy));

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_dummy_vkey(),
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: std::sync::Arc::new(std::sync::RwLock::new("http://localhost:3000".to_string())),
        policy: policy_lock,
        rate_limiter: RateLimiter::new(),
        connection_manager: ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::LiveKitConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
    };
    let app = app(state);

    // 2. Prepare Request (Update Policy to conflict)
    // We require a capability that the peer does not offer.
    let new_policy = ServerPolicy {
        agent_required_capabilities: vec!["MUST_HAVE_THIS".to_string()],
        federation_enabled: true,
        ..Default::default()
    };

    let body_json = serde_json::to_string(&new_policy).unwrap();

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/admin/policy")
        .method("PUT")
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", "mod_user") // Auth
        .body(Body::from(body_json))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    // 3. Execute
    let response = app.oneshot(request).await.unwrap();

    // 4. Verify Response
    assert_eq!(response.status(), StatusCode::OK);

    // 5. Verify DB State
    let conn = pool.get().unwrap();

    // Check Agreement Status
    let (status, active): (String, bool) = conn
        .query_row(
            "SELECT alignment_status, active FROM federation_agreements WHERE remote_instance_id = 10",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(status, "CONFLICT");
    assert!(!active);

    // Check Policy Version
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM server_policy_versions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}
