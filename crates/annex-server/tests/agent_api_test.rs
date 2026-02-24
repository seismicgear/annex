use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::zk::{G1Affine, G2Affine, VerifyingKey};
use annex_server::{app, middleware, AppState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt; // for oneshot

#[tokio::test]
async fn test_get_agent_profile() {
    // 1. Setup App
    // Using tempfile for shared DB across pool connections
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap();
    let pool = create_pool(db_path, DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Insert server
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'default', 'Default Server', '{}')",
        [],
    ).unwrap();

    let vk = VerifyingKey {
        alpha_g1: G1Affine::default(),
        beta_g2: G2Affine::default(),
        gamma_g2: G2Affine::default(),
        delta_g2: G2Affine::default(),
        gamma_abc_g1: vec![],
    };

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(annex_identity::MerkleTree::new(20).unwrap())),
        membership_vkey: Arc::new(vk),
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: std::sync::Arc::new(std::sync::RwLock::new("http://localhost:3000".to_string())),
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

    let app = app(state);

    // 2. Insert Agent Data
    let agent_pseudonym = "agent_007";
    let contract_json = r#"{
        "required_capabilities": ["TEXT"],
        "offered_capabilities": ["TEXT", "VOICE"]
    }"#;

    conn.execute(
        "INSERT INTO agent_registrations (
            server_id, pseudonym_id, alignment_status, transfer_scope,
            capability_contract_json, reputation_score, last_handshake_at
        ) VALUES (?1, ?2, 'ALIGNED', 'FULL_KNOWLEDGE_BUNDLE', ?3, 0.95, datetime('now'))",
        rusqlite::params![1, agent_pseudonym, contract_json],
    )
    .unwrap();

    // 3. Insert Caller Identity (for auth)
    let caller_pseudonym = "user_123";
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
         VALUES (1, ?1, 'HUMAN', 1)",
        rusqlite::params![caller_pseudonym],
    )
    .unwrap();

    // 4. Request Agent Profile
    let uri = format!("/api/agents/{}", agent_pseudonym);
    let req = Request::builder()
        .uri(uri)
        .method("GET")
        .header("X-Annex-Pseudonym", caller_pseudonym)
        .extension(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))))
        .body(Body::empty())
        .unwrap();

    // Clone app for first request
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_json: Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(body_json["pseudonym_id"], agent_pseudonym);
    assert_eq!(body_json["alignment_status"], "Aligned");
    assert_eq!(body_json["transfer_scope"], "FullKnowledgeBundle");
    assert_eq!(body_json["reputation_score"], 0.95);
    assert_eq!(
        body_json["capability_contract"]["required_capabilities"][0],
        "TEXT"
    );

    // 5. Test Not Found
    let uri_404 = "/api/agents/unknown_agent";
    let req_404 = Request::builder()
        .uri(uri_404)
        .method("GET")
        .header("X-Annex-Pseudonym", caller_pseudonym)
        .extension(axum::extract::ConnectInfo(SocketAddr::from((
            [127, 0, 0, 1],
            12345,
        ))))
        .body(Body::empty())
        .unwrap();

    let resp_404 = app.clone().oneshot(req_404).await.unwrap();
    assert_eq!(resp_404.status(), StatusCode::NOT_FOUND);
}
