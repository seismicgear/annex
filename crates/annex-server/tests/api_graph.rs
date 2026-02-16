use annex_db::{create_pool, DbRuntimeSettings};
use annex_graph::{create_edge, ensure_graph_node};
use annex_server::{app, AppState};
use annex_types::{EdgeKind, NodeType, ServerPolicy};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt; // for oneshot
use annex_identity::zk::{G1Affine, G2Affine, VerifyingKey};

#[tokio::test]
async fn test_get_degrees() {
    // 1. Setup DB
    let temp_file = tempfile::NamedTempFile::new().unwrap();
    let db_path = temp_file.path().to_str().unwrap();
    let pool = create_pool(db_path, DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Insert dummy server
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('default', 'Default Server', '{}')",
        [],
    )
    .unwrap();

    // 2. Setup App State
    let tree = annex_identity::MerkleTree::new(20).unwrap();
    // Construct dummy VK
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
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: annex_server::middleware::RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
    };
    let app = app(state);

    // 3. Create Graph
    // A -> B -> C
    let server_id = 1;
    ensure_graph_node(&conn, server_id, "user_a", NodeType::Human).unwrap();
    ensure_graph_node(&conn, server_id, "user_b", NodeType::Human).unwrap();
    ensure_graph_node(&conn, server_id, "user_c", NodeType::Human).unwrap();

    create_edge(&conn, server_id, "user_a", "user_b", EdgeKind::Connected, 1.0).unwrap();
    create_edge(&conn, server_id, "user_b", "user_c", EdgeKind::Connected, 1.0).unwrap();

    // 4. Test API: A -> C (Depth 5)
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let uri = "/api/graph/degrees?from=user_a&to=user_c&maxDepth=5";
    let req = Request::builder()
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let req = {
        let mut r = req;
        r.extensions_mut().insert(ConnectInfo(addr));
        r
    };

    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let body_bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    if parts.status != StatusCode::OK {
        let err_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
        println!("Error: {:?}", err_json);
        panic!("Status: {}", parts.status);
    }
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
    let req = Request::builder()
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let req = {
        let mut r = req;
        r.extensions_mut().insert(ConnectInfo(addr));
        r
    };

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(json["found"], false);
}
