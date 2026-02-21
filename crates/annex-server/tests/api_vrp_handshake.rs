use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use annex_vrp::{
    VrpAlignmentStatus, VrpAnchorSnapshot, VrpCapabilitySharingContract, VrpFederationHandshake,
    VrpTransferScope, VrpValidationReport,
};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt; // for oneshot

fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    // This path assumes running from workspace root or crate root correctly resolved by cargo
    // The test in api_registry uses this path:
    let vkey_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_vkey.json");

    // If the file doesn't exist (e.g. fresh clone), we might need to regenerate or mock.
    // However, existing tests rely on it. We assume it exists or is generated.
    // If it fails, CI/dev env needs to run `npm run setup`.
    if !vkey_path.exists() {
        // Fallback or panic? Existing tests panic.
        panic!("vkey not found at {:?}", vkey_path);
    }

    let vkey_json = std::fs::read_to_string(vkey_path).expect("failed to read vkey");
    annex_identity::zk::parse_verification_key(&vkey_json)
        .map(Arc::new)
        .expect("failed to parse vkey")
}

async fn setup_app() -> (axum::Router, annex_db::DbPool) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Insert a server row for FK constraints
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'test-server', 'Test Server', '{}')",
        [],
    )
    .unwrap();

    drop(conn);

    let tree = MerkleTree::new(20).unwrap();

    // Use default policy
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
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
    };

    (app(state), pool)
}

#[tokio::test]
async fn test_agent_handshake_aligned() {
    let (app, pool) = setup_app().await;

    // 1. Create Handshake Payload (Aligned)
    // ServerPolicy default has empty principles/prohibitions.
    // We match that for Aligned status.
    let anchor = VrpAnchorSnapshot::new(&[], &[]).unwrap();

    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec!["TEXT".to_string(), "VRP".to_string()],
        redacted_topics: vec![],
    };

    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };

    let payload = serde_json::json!({
        "pseudonymId": "agent-123",
        "handshake": handshake
    });

    // 2. Send Request
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/vrp/agent-handshake")
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
    // Default transfer config allows reflection summaries for agents (hardcoded in handler for now)
    assert_eq!(
        report.transfer_scope,
        VrpTransferScope::ReflectionSummariesOnly
    );

    // 4. Verify DB State
    let conn = pool.get().unwrap();

    // Check agent_registrations
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_registrations WHERE pseudonym_id = 'agent-123')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(exists, "agent registration should be created");

    // Check handshake log
    let log_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM vrp_handshake_log WHERE peer_pseudonym = 'agent-123'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(log_count, 1, "handshake should be logged");
}

#[tokio::test]
async fn test_agent_handshake_conflict() {
    let (app, pool) = setup_app().await;

    // 1. Create Handshake Payload (Conflict)
    // Server has empty principles. Agent has conflicting principles.
    // Wait, simple comparison: if hashes differ -> Conflict.
    let anchor = VrpAnchorSnapshot::new(&["some-principle".to_string()], &[]).unwrap();

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
        "pseudonymId": "agent-conflict",
        "handshake": handshake
    });

    // 2. Send Request
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/vrp/agent-handshake")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();

    // 3. Verify Response
    assert_eq!(response.status(), StatusCode::OK); // 200 OK with Conflict status

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let report: VrpValidationReport = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(report.alignment_status, VrpAlignmentStatus::Conflict);
    assert_eq!(report.transfer_scope, VrpTransferScope::NoTransfer);

    // 4. Verify DB State
    let conn = pool.get().unwrap();

    // Check agent_registrations - should NOT exist
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_registrations WHERE pseudonym_id = 'agent-conflict')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        !exists,
        "agent registration should NOT be created on conflict"
    );

    // Check handshake log - should exist
    let log_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM vrp_handshake_log WHERE peer_pseudonym = 'agent-conflict'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(log_count, 1, "handshake should be logged even on conflict");
}
