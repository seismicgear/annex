use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::{create_platform_identity, RoleCode};
use annex_server::{
    api::{GetCapabilitiesResponse, GetIdentityResponse},
    app, AppState,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

#[tokio::test]
async fn test_get_identity_endpoints() {
    // 1. Setup
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();

    // Seed server
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('default', 'Default Server', '{}')",
            [],
        )
        .unwrap();
    } // Drop conn

    let tree = annex_identity::MerkleTree::new(20).unwrap();
    // Use dummy vkey since app() requires it
    let vkey_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_vkey.json");

    // Check if key exists (integration tests might run where keys are generated)
    // If not, we might fail. The memory says keys are regenerated if missing by annex-identity tests,
    // but we are in annex-server.
    // Ideally we should use a shared helper or ensure keys exist.
    // For now, assume keys exist as per existing tests.
    let vkey_json = std::fs::read_to_string(&vkey_path).expect("failed to read vkey");
    let vk = annex_identity::zk::parse_verification_key(&vkey_json).expect("failed to parse vkey");

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(vk),
        server_id: 1,
    };
    let app = app(state);

    // 2. Insert a Platform Identity directly
    let server_id = 1;
    let pseudonym_id = "test-pseudonym-123";
    let role = RoleCode::Human;

    {
        let conn = pool.get().unwrap();
        create_platform_identity(&conn, server_id, pseudonym_id, role).unwrap();

        // Update capabilities to something non-default to verify
        conn.execute(
            "UPDATE platform_identities SET can_voice = 1, can_moderate = 1 WHERE pseudonym_id = ?1",
            [pseudonym_id],
        ).unwrap();
    }

    // 3. Test GET /api/identity/:pseudonymId
    let req = Request::builder()
        .uri(format!("/api/identity/{}", pseudonym_id))
        .body(Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let identity: GetIdentityResponse = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(identity.pseudonym_id, pseudonym_id);
    assert_eq!(identity.participant_type, role);
    assert!(identity.active);
    assert!(identity.capabilities.can_voice);
    assert!(identity.capabilities.can_moderate);
    assert!(!identity.capabilities.can_invite); // Default

    // 4. Test GET /api/identity/:pseudonymId/capabilities
    let req_caps = Request::builder()
        .uri(format!("/api/identity/{}/capabilities", pseudonym_id))
        .body(Body::empty())
        .unwrap();

    let resp_caps = app.clone().oneshot(req_caps).await.unwrap();
    assert_eq!(resp_caps.status(), StatusCode::OK);

    let body_bytes_caps = axum::body::to_bytes(resp_caps.into_body(), usize::MAX)
        .await
        .unwrap();
    let caps_resp: GetCapabilitiesResponse = serde_json::from_slice(&body_bytes_caps).unwrap();

    assert!(caps_resp.capabilities.can_voice);
    assert!(caps_resp.capabilities.can_moderate);
    assert!(!caps_resp.capabilities.can_invite);

    // 5. Test Not Found
    let req_nf = Request::builder()
        .uri("/api/identity/non-existent")
        .body(Body::empty())
        .unwrap();

    let resp_nf = app.oneshot(req_nf).await.unwrap();
    assert_eq!(resp_nf.status(), StatusCode::NOT_FOUND);
}
