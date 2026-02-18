use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use annex_vrp::{
    VrpAlignmentStatus, VrpAnchorSnapshot, VrpCapabilitySharingContract, VrpFederationHandshake,
    VrpValidationReport,
};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

// Mock or load vkey
fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    Arc::new(annex_identity::zk::generate_dummy_vkey())
}

async fn setup_app() -> (axum::Router, annex_db::DbPool) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Insert a server row
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'test-server', 'Test Server', '{}')",
        [],
    )
    .unwrap();

    // Insert a remote instance for federation handshake
    conn.execute(
        "INSERT INTO instances (id, base_url, public_key, label, status) VALUES (10, 'https://remote.example.com', 'pubkey', 'Remote Instance', 'ACTIVE')",
        [],
    ).unwrap();

    drop(conn); // Return connection to pool

    let tree = MerkleTree::new(20).unwrap();
    let policy = ServerPolicy::default();

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: "http://localhost:3000".to_string(),
        policy: Arc::new(RwLock::new(policy)),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::LiveKitConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
    };

    (app(state), pool)
}

#[tokio::test]
async fn test_federation_handshake_success() {
    let (app, pool) = setup_app().await;

    // 1. Prepare Payload
    let anchor = VrpAnchorSnapshot::new(&[], &[]); // Matches default policy
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec![],
        redacted_topics: vec![],
    };
    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };

    let payload = serde_json::json!({
        "base_url": "https://remote.example.com",
        "anchor_snapshot": handshake.anchor_snapshot,
        "capability_contract": handshake.capability_contract
    });

    // 2. Send Request
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/federation/handshake")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();

    // 3. Verify Response
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let report: VrpValidationReport = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(report.alignment_status, VrpAlignmentStatus::Aligned);

    // 4. Verify DB
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM federation_agreements WHERE remote_instance_id = 10",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_federation_handshake_unknown_instance() {
    let (app, _) = setup_app().await;

    // 1. Prepare Payload with unknown URL
    let anchor = VrpAnchorSnapshot::new(&[], &[]);
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec![],
        redacted_topics: vec![],
    };
    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };

    let payload = serde_json::json!({
        "base_url": "https://unknown.example.com",
        "anchor_snapshot": handshake.anchor_snapshot,
        "capability_contract": handshake.capability_contract
    });

    // 2. Send Request
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/federation/handshake")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();

    // 3. Verify Response
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
