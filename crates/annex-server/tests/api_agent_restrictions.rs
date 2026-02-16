use annex_channels::{create_channel, CreateChannelParams};
use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::{AlignmentStatus, ChannelType, FederationScope, ServerPolicy};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

// Helper to load verification key
fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // If running from crates/annex-server
    let path1 = manifest.join("../../zk/keys/membership_vkey.json");
    // If running from root
    let path2 = std::path::Path::new("zk/keys/membership_vkey.json");

    let vkey_json = std::fs::read_to_string(&path1)
        .or_else(|_| std::fs::read_to_string(path2))
        .unwrap_or_else(|_| panic!("failed to read vkey from {:?} or {:?}", path1, path2));

    let vk = annex_identity::zk::parse_verification_key(&vkey_json).expect("failed to parse vkey");
    Arc::new(vk)
}

async fn setup_app() -> (axum::Router, annex_db::DbPool) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        // Create server
        let policy = ServerPolicy::default();
        let policy_json = serde_json::to_string(&policy).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
    }

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(annex_voice::LiveKitConfig::default())),
    };

    (app(state), pool)
}

#[tokio::test]
async fn test_join_channel_conflict_agent_implicit_allow() {
    let (app, pool) = setup_app().await;

    // Seed data
    {
        let conn = pool.get().unwrap();
        // Agent Identity
        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'agent-conflict', 'AI_AGENT', 1)", []).unwrap();

        // Agent Registration (Conflict)
        conn.execute(
            "INSERT INTO agent_registrations (server_id, pseudonym_id, alignment_status, transfer_scope, capability_contract_json, last_handshake_at)
             VALUES (1, 'agent-conflict', '\"Conflict\"', '\"NoTransfer\"', '{}', datetime('now'))",
            []
        ).unwrap();

        // Create Channel with NO min alignment specified (should default to allowing if not checked)
        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-any".to_string(),
            name: "General".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None, // No restriction specified
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/chan-any/join")
        .method("POST")
        .header("X-Annex-Pseudonym", "agent-conflict")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();

    // Expect failure because "Conflict agents cannot join any channel"
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_join_voice_channel_partial_agent() {
    let (app, pool) = setup_app().await;

    {
        let conn = pool.get().unwrap();
        // Agent Identity
        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'agent-partial', 'AI_AGENT', 1)", []).unwrap();

        // Agent Registration (Partial)
        conn.execute(
            "INSERT INTO agent_registrations (server_id, pseudonym_id, alignment_status, transfer_scope, capability_contract_json, last_handshake_at)
             VALUES (1, 'agent-partial', '\"Partial\"', '\"ReflectionSummariesOnly\"', '{}', datetime('now'))",
            []
        ).unwrap();

        // Create Voice Channel
        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-voice".to_string(),
            name: "Voice Chat".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: Some(AlignmentStatus::Partial), // Explicitly allows Partial
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/chan-voice/join")
        .method("POST")
        .header("X-Annex-Pseudonym", "agent-partial")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();

    // Expect failure because "Partial agents are restricted to TEXT channels only"
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_join_text_channel_partial_agent() {
    let (app, pool) = setup_app().await;

    {
        let conn = pool.get().unwrap();
        // Agent Identity
        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'agent-partial-ok', 'AI_AGENT', 1)", []).unwrap();

        // Agent Registration (Partial)
        conn.execute(
            "INSERT INTO agent_registrations (server_id, pseudonym_id, alignment_status, transfer_scope, capability_contract_json, last_handshake_at)
             VALUES (1, 'agent-partial-ok', '\"Partial\"', '\"ReflectionSummariesOnly\"', '{}', datetime('now'))",
            []
        ).unwrap();

        // Create Text Channel
        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-text".to_string(),
            name: "Text Chat".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: Some(AlignmentStatus::Partial), // Allows Partial
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/chan-text/join")
        .method("POST")
        .header("X-Annex-Pseudonym", "agent-partial-ok")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();

    // Expect success
    assert_eq!(response.status(), StatusCode::OK);
}
