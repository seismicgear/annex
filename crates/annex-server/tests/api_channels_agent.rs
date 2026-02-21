use annex_db::{create_pool, DbRuntimeSettings};
use annex_graph::get_edges;
use annex_identity::zk::{G1Affine, G2Affine, VerifyingKey};
use annex_identity::MerkleTree;
use annex_server::{app, middleware, AppState};
use annex_types::{ChannelType, EdgeKind, FederationScope, NodeType, ServerPolicy};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

fn setup_test_app() -> (axum::Router, annex_db::DbPool, tempfile::NamedTempFile) {
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap();
    let pool = create_pool(db_path, DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Insert dummy server
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'default', 'Default Server', '{}')",
        [],
    )
    .unwrap();

    let tree = MerkleTree::new(20).unwrap();
    let vk = VerifyingKey {
        alpha_g1: G1Affine::default(),
        beta_g2: G2Affine::default(),
        gamma_g2: G2Affine::default(),
        delta_g2: G2Affine::default(),
        gamma_abc_g1: vec![G1Affine::default()],
    };

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(vk),
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: "http://localhost:3000".to_string(),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: middleware::RateLimiter::new(),
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
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
    };

    (app(state), pool, temp_file)
}

#[tokio::test]
async fn test_agent_join_creates_edge() {
    let (app, pool, _temp) = setup_test_app();
    let conn = pool.get().unwrap();
    let server_id = 1;
    let pseudonym = "agent_007";
    let channel_id = "general";

    // 1. Setup Data
    // Create Agent Registration
    conn.execute(
        "INSERT INTO agent_registrations (server_id, pseudonym_id, alignment_status, transfer_scope, capability_contract_json, reputation_score, last_handshake_at)
         VALUES (?1, ?2, 'Aligned', 'FullKnowledgeBundle', '{}', 1.0, datetime('now'))",
        rusqlite::params![server_id, pseudonym],
    ).unwrap();

    // Create Channel
    let channel_type_json = serde_json::to_string(&ChannelType::Text).unwrap();
    let fed_scope_json = serde_json::to_string(&FederationScope::Local).unwrap();
    conn.execute(
        "INSERT INTO channels (server_id, channel_id, name, channel_type, federation_scope)
         VALUES (?1, ?2, 'General', ?3, ?4)",
        rusqlite::params![server_id, channel_id, channel_type_json, fed_scope_json],
    )
    .unwrap();

    // Ensure Graph Node for Agent
    annex_graph::ensure_graph_node(
        &conn,
        server_id,
        pseudonym,
        NodeType::AiAgent,
        Some("{}".to_string()),
    )
    .unwrap();

    // Insert Platform Identity (needed for auth middleware)
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
         VALUES (?1, ?2, 'AI_AGENT', 1)",
        rusqlite::params![server_id, pseudonym],
    )
    .unwrap();

    // 2. Prepare Request
    let uri = format!("/api/channels/{}/join", channel_id);
    let req = Request::builder()
        .uri(uri)
        .method("POST")
        .header("X-Annex-Pseudonym", pseudonym)
        .body(Body::empty())
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));

    // 3. Call Join
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 4. Verify Edge Created
    let edges = get_edges(&conn, server_id, pseudonym).unwrap();
    assert_eq!(edges.len(), 1);
    let edge = &edges[0];
    assert_eq!(edge.to_node, channel_id);
    assert_eq!(edge.kind, EdgeKind::AgentServing);

    // 5. Call Leave
    let uri = format!("/api/channels/{}/leave", channel_id);
    let req = Request::builder()
        .uri(uri)
        .method("POST")
        .header("X-Annex-Pseudonym", pseudonym)
        .body(Body::empty())
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 6. Verify Edge Removed
    let edges = get_edges(&conn, server_id, pseudonym).unwrap();
    assert!(edges.is_empty());
}
