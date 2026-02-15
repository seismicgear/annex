use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::{MerkleTree, VrpRoleEntry, VrpTopic};
use annex_server::{app, AppState};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::{Arc, Mutex};
use tower::ServiceExt; // for oneshot

fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    let vkey_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_vkey.json");

    // Ensure the key exists or panic with helpful message
    if !vkey_path.exists() {
        panic!(
            "ZK key not found at {:?}. Run setup scripts in zk/ directory.",
            vkey_path
        );
    }

    let vkey_json = std::fs::read_to_string(vkey_path).expect("failed to read vkey");
    let vk = annex_identity::zk::parse_verification_key(&vkey_json).expect("failed to parse vkey");
    Arc::new(vk)
}

#[tokio::test]
async fn test_get_topics() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();
    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
    };
    let app = app(state);

    let request = Request::builder()
        .uri("/api/registry/topics")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let topics: Vec<VrpTopic> = serde_json::from_slice(&body_bytes).unwrap();

    // Default seeded topics
    assert!(topics.len() >= 3);
    assert!(topics.iter().any(|t| t.topic == "annex:server:v1"));
    assert!(topics.iter().any(|t| t.topic == "annex:channel:v1"));
    assert!(topics.iter().any(|t| t.topic == "annex:federation:v1"));
}

#[tokio::test]
async fn test_get_roles() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();
    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
    };
    let app = app(state);

    let request = Request::builder()
        .uri("/api/registry/roles")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let roles: Vec<VrpRoleEntry> = serde_json::from_slice(&body_bytes).unwrap();

    // Default seeded roles
    assert!(roles.len() >= 5);
    assert!(roles.iter().any(|r| r.label == "HUMAN" && r.role_code == 1));
    assert!(roles
        .iter()
        .any(|r| r.label == "AI_AGENT" && r.role_code == 2));
    assert!(roles
        .iter()
        .any(|r| r.label == "COLLECTIVE" && r.role_code == 3));
    assert!(roles
        .iter()
        .any(|r| r.label == "BRIDGE" && r.role_code == 4));
    assert!(roles
        .iter()
        .any(|r| r.label == "SERVICE" && r.role_code == 5));
}
