use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{api::GetRootResponse, app, AppState};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    let vkey_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_vkey.json");
    let vkey_json = std::fs::read_to_string(vkey_path).expect("failed to read vkey");
    let vk = annex_identity::zk::parse_verification_key(&vkey_json).expect("failed to parse vkey");
    Arc::new(vk)
}

#[tokio::test]
async fn test_get_current_root_empty_tree() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();
    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
    };
    let app = app(state);

    let request = Request::builder()
        .uri("/api/registry/current-root")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: GetRootResponse = serde_json::from_slice(&body_bytes).unwrap();

    // Empty tree
    assert_eq!(resp.leaf_count, 0);
    // Initial root for depth 20 is deterministic, but checking hex length is good
    assert_eq!(resp.root_hex.len(), 64);
    // No registration, so no update timestamp yet
    assert!(resp.updated_at.is_none());
}

#[tokio::test]
async fn test_get_current_root_after_registration() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();
    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
    };
    let app = app(state);

    // Register
    let commitment = "0000000000000000000000000000000000000000000000000000000000000001";
    let register_body = serde_json::json!({
        "commitmentHex": commitment,
        "roleCode": 1,
        "nodeId": 100
    });

    let reg_req = Request::builder()
        .uri("/api/registry/register")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(register_body.to_string()))
        .unwrap();

    let _ = app.clone().oneshot(reg_req).await.unwrap();

    // Get Root
    let request = Request::builder()
        .uri("/api/registry/current-root")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: GetRootResponse = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(resp.leaf_count, 1);
    assert_eq!(resp.root_hex.len(), 64);
    assert!(resp.updated_at.is_some());
}
