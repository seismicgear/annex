use annex_db::{create_pool, DbRuntimeSettings};
use annex_graph::{create_edge, ensure_graph_node, GraphProfile};
use annex_identity::zk::{G1Affine, G2Affine, VerifyingKey};
use annex_server::{app, middleware, AppState};
use annex_types::{EdgeKind, NodeType, ServerPolicy, VisibilityLevel};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt; // for oneshot

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

    let tree = annex_identity::MerkleTree::new(20).unwrap();
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
    };

    (app(state), pool, temp_file)
}

#[tokio::test]
async fn test_get_degrees() {
    let (app, pool, _temp) = setup_test_app();
    let conn = pool.get().unwrap();
    let server_id = 1;

    // A -> B -> C
    ensure_graph_node(&conn, server_id, "user_a", NodeType::Human, None).unwrap();
    ensure_graph_node(&conn, server_id, "user_b", NodeType::Human, None).unwrap();
    ensure_graph_node(&conn, server_id, "user_c", NodeType::Human, None).unwrap();

    create_edge(
        &conn,
        server_id,
        "user_a",
        "user_b",
        EdgeKind::Connected,
        1.0,
    )
    .unwrap();
    create_edge(
        &conn,
        server_id,
        "user_b",
        "user_c",
        EdgeKind::Connected,
        1.0,
    )
    .unwrap();

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let uri = "/api/graph/degrees?from=user_a&to=user_c&maxDepth=5";
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let req = {
        let mut r = req;
        r.extensions_mut().insert(ConnectInfo(addr));
        r
    };

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(json["found"], true);
    assert_eq!(json["length"], 2);
    let path = json["path"].as_array().unwrap();
    assert_eq!(path.len(), 3);
    assert_eq!(path[0], "user_a");
    assert_eq!(path[1], "user_b");
    assert_eq!(path[2], "user_c");

    // 5. Test API: A -> C (Depth 1) - Should fail
    let uri = "/api/graph/degrees?from=user_a&to=user_c&maxDepth=1";
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let req = {
        let mut r = req;
        r.extensions_mut().insert(ConnectInfo(addr));
        r
    };

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(json["found"], false);
}

#[tokio::test]
async fn test_get_profile_visibility() {
    let (app, pool, _temp) = setup_test_app();
    let conn = pool.get().unwrap();
    let server_id = 1;

    // Graph: A -> B -> C -> D, and E (disconnected)
    let nodes = vec!["A", "B", "C", "D", "E"];
    for n in &nodes {
        ensure_graph_node(&conn, server_id, n, NodeType::Human, None).unwrap();
    }

    create_edge(&conn, server_id, "A", "B", EdgeKind::Connected, 1.0).unwrap();
    create_edge(&conn, server_id, "B", "C", EdgeKind::Connected, 1.0).unwrap();
    create_edge(&conn, server_id, "C", "D", EdgeKind::Connected, 1.0).unwrap();

    // Add metadata to A so we can check it's visible to Self but hidden from others
    conn.execute(
        "UPDATE graph_nodes SET metadata_json = '{\"foo\":\"bar\"}' WHERE pseudonym_id = 'A'",
        [],
    )
    .unwrap();

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // Helper to make request
    let check_visibility = |target: &str, viewer: &str| {
        let target = target.to_string();
        let viewer = viewer.to_string();
        let app = app.clone();
        async move {
            let uri = format!("/api/graph/profile/{}", target);
            let req = Request::builder()
                .uri(uri)
                .header("X-Annex-Viewer", viewer)
                .body(Body::empty())
                .unwrap();
            let mut req = req;
            req.extensions_mut().insert(ConnectInfo(addr));

            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);

            let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let profile: GraphProfile = serde_json::from_slice(&body_bytes).unwrap();
            profile
        }
    };

    // 1. A views A (Self)
    let p = check_visibility("A", "A").await;
    assert_eq!(p.visibility, VisibilityLevel::Self_);
    assert!(p.last_seen_at.is_some());
    assert!(p.metadata_json.is_some()); // Self sees metadata

    // 2. A views B (Degree 1)
    let p = check_visibility("B", "A").await;
    assert_eq!(p.visibility, VisibilityLevel::Degree1);
    assert!(p.last_seen_at.is_some());
    assert!(p.metadata_json.is_none()); // Degree 1: no metadata

    // 3. A views C (Degree 2)
    let p = check_visibility("C", "A").await;
    assert_eq!(p.visibility, VisibilityLevel::Degree2);
    assert!(p.last_seen_at.is_none()); // Degree 2: no last_seen
    assert!(p.metadata_json.is_none());

    // 4. A views D (Degree 3)
    let p = check_visibility("D", "A").await;
    assert_eq!(p.visibility, VisibilityLevel::Degree3);
    assert!(p.last_seen_at.is_none());
    assert!(p.metadata_json.is_none());

    // 5. A views E (None)
    let p = check_visibility("E", "A").await;
    assert_eq!(p.visibility, VisibilityLevel::None);
    assert!(p.last_seen_at.is_none());
    assert!(p.metadata_json.is_none());

    // 6. Missing Header
    let uri = "/api/graph/profile/A";
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let mut req = req;
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
