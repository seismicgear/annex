use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{
    api::{GetPathResponse, RegisterResponse},
    app, AppState,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[tokio::test]
async fn test_get_path_success() {
    // 1. Setup
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();
    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
    };
    let app = app(state);

    // 2. Register identity
    let commitment = "0000000000000000000000000000000000000000000000000000000000000001";
    let body_json = serde_json::json!({
        "commitmentHex": commitment,
        "roleCode": 1,
        "nodeId": 100
    });

    let request = Request::builder()
        .uri("/api/registry/register")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body_json.to_string()))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Parse response to check expectations
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let reg_resp: RegisterResponse = serde_json::from_slice(&body_bytes).unwrap();

    // 3. Get Path
    let request_path = Request::builder()
        .uri(format!("/api/registry/path/{}", commitment))
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response_path = app.oneshot(request_path).await.unwrap();
    assert_eq!(response_path.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response_path.into_body(), usize::MAX)
        .await
        .unwrap();
    let path_resp: GetPathResponse = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(path_resp.leaf_index, reg_resp.leaf_index);
    assert_eq!(path_resp.root_hex, reg_resp.root_hex);
    assert_eq!(path_resp.path_elements, reg_resp.path_elements);
    assert_eq!(path_resp.path_indices, reg_resp.path_indices);
}

#[tokio::test]
async fn test_get_path_not_found() {
    // 1. Setup
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();
    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
    };
    let app = app(state);

    // 2. Get Path for non-existent commitment
    let commitment = "0000000000000000000000000000000000000000000000000000000000000099";
    let request_path = Request::builder()
        .uri(format!("/api/registry/path/{}", commitment))
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let response_path = app.oneshot(request_path).await.unwrap();
    assert_eq!(response_path.status(), StatusCode::NOT_FOUND);
}
