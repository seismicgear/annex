//! Service-layer tests for `IdentityService` (`crates/annex-server/src/services/identity_service.rs`).
//!
//! These exercise the orchestration directly, without going through axum
//! or HTTP. The companion integration tests in `tests/api_registry*.rs`
//! continue to cover the HTTP contract end-to-end; this file is the
//! finer-grained unit coverage that became possible once the
//! orchestration moved out of `api.rs`.

mod common;

use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::api::RegisterRequest;
use annex_server::services::IdentityService;
use annex_server::AppState;
use annex_types::ServerPolicy;
use std::sync::Arc;

fn build_state(policy: ServerPolicy) -> Arc<AppState> {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        let policy_json = serde_json::to_string(&policy).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
    }
    let tree = MerkleTree::new(20).unwrap();
    Arc::new(common::build_app_state(pool, tree, policy))
}

fn unique_commitment(byte: u8) -> String {
    // Construct a 64-char canonical lowercase hex of `byte` in the last
    // byte slot. Tests only need a value that's structurally valid (< field
    // modulus, fixed width, 0-9a-f) and varies between cases.
    format!("{:062x}{:02x}", 0u128, byte)
}

#[tokio::test]
async fn register_identity_succeeds_in_open_mode() {
    let state = build_state(ServerPolicy::default());
    let svc = IdentityService::new(state);

    let payload = RegisterRequest {
        commitment_hex: unique_commitment(1),
        role_code: 1,
        node_id: 100,
        invite_code: None,
        server_password: None,
    };

    let resp = svc.register_identity(payload).await.expect("register ok");
    assert_eq!(resp.leaf_index, 0);
    assert_eq!(resp.path_indices.len(), 20, "depth-20 tree => 20 path bits");
    assert_eq!(resp.root_hex.len(), 64, "canonical 64-char hex");
}

#[tokio::test]
async fn register_identity_is_idempotent_for_duplicate_commitment() {
    // Round-tripping the same (commitment, role, node_id) must NOT fail
    // — registration is documented as idempotent so a client that retries
    // after a network blip recovers without an error.
    let state = build_state(ServerPolicy::default());
    let svc = IdentityService::new(state);

    let payload = RegisterRequest {
        commitment_hex: unique_commitment(2),
        role_code: 1,
        node_id: 200,
        invite_code: None,
        server_password: None,
    };

    let first = svc
        .register_identity(payload.clone())
        .await
        .expect("first register");
    let second = svc
        .register_identity(payload)
        .await
        .expect("second register (idempotent retry)");

    assert_eq!(first.leaf_index, second.leaf_index);
    assert_eq!(first.identity_id, second.identity_id);
    assert_eq!(first.root_hex, second.root_hex);
    assert_eq!(first.path_elements, second.path_elements);
    assert_eq!(first.path_indices, second.path_indices);
}

#[tokio::test]
async fn register_identity_rejects_invalid_role() {
    let state = build_state(ServerPolicy::default());
    let svc = IdentityService::new(state);

    let payload = RegisterRequest {
        commitment_hex: unique_commitment(3),
        role_code: 250,
        node_id: 1,
        invite_code: None,
        server_password: None,
    };

    let err = svc
        .register_identity(payload)
        .await
        .expect_err("250 is not a valid role code");
    use annex_server::services::IdentityServiceError;
    assert!(
        matches!(err, IdentityServiceError::BadRequest(_)),
        "invalid role => BadRequest, got: {err:?}"
    );
}

#[tokio::test]
async fn invite_only_mode_rejects_missing_invite_code() {
    let mut policy = ServerPolicy::default();
    policy.access_mode = "invite_only".to_string();
    let state = build_state(policy);
    let svc = IdentityService::new(state);

    let payload = RegisterRequest {
        commitment_hex: unique_commitment(4),
        role_code: 1,
        node_id: 1,
        invite_code: None,
        server_password: None,
    };

    let err = svc
        .register_identity(payload)
        .await
        .expect_err("invite_only mode without code should fail");
    use annex_server::services::IdentityServiceError;
    assert!(matches!(err, IdentityServiceError::Forbidden(_)));
}

#[tokio::test]
async fn password_mode_rejects_wrong_password() {
    let mut policy = ServerPolicy::default();
    policy.access_mode = "password".to_string();
    policy.access_password = "correct-horse".to_string();
    let state = build_state(policy);
    let svc = IdentityService::new(state);

    let payload = RegisterRequest {
        commitment_hex: unique_commitment(5),
        role_code: 1,
        node_id: 1,
        invite_code: None,
        server_password: Some("battery-staple".to_string()),
    };

    let err = svc
        .register_identity(payload)
        .await
        .expect_err("wrong password should fail");
    use annex_server::services::IdentityServiceError;
    assert!(matches!(err, IdentityServiceError::Forbidden(_)));
}

#[tokio::test]
async fn get_current_root_reflects_registrations() {
    let state = build_state(ServerPolicy::default());
    let svc = IdentityService::new(state);

    // Empty tree.
    let initial = svc.get_current_root().await.expect("root before");
    assert_eq!(initial.leaf_count, 0);
    assert_eq!(initial.root_hex.len(), 64);

    // Register one identity, current-root must change.
    let payload = RegisterRequest {
        commitment_hex: unique_commitment(6),
        role_code: 1,
        node_id: 1,
        invite_code: None,
        server_password: None,
    };
    let _ = svc.register_identity(payload).await.expect("register");

    let after = svc.get_current_root().await.expect("root after");
    assert_eq!(after.leaf_count, 1);
    assert_ne!(after.root_hex, initial.root_hex);
}

#[tokio::test]
async fn get_merkle_path_404s_unknown_commitment() {
    let state = build_state(ServerPolicy::default());
    let svc = IdentityService::new(state);

    let unknown = "f".repeat(64);
    let err = svc
        .get_merkle_path(unknown)
        .await
        .expect_err("unknown commitment must 404");
    use annex_server::services::IdentityServiceError;
    assert!(
        matches!(err, IdentityServiceError::NotFound(_)),
        "unknown commitment => NotFound, got: {err:?}"
    );
}

#[tokio::test]
async fn get_merkle_path_returns_path_for_registered_commitment() {
    let state = build_state(ServerPolicy::default());
    let svc = IdentityService::new(state);

    let commitment = unique_commitment(7);
    let payload = RegisterRequest {
        commitment_hex: commitment.clone(),
        role_code: 1,
        node_id: 1,
        invite_code: None,
        server_password: None,
    };
    let registered = svc.register_identity(payload).await.expect("register");

    let path = svc
        .get_merkle_path(commitment)
        .await
        .expect("path lookup");
    assert_eq!(path.leaf_index, registered.leaf_index);
    assert_eq!(path.path_indices, registered.path_indices);
    assert_eq!(path.path_elements, registered.path_elements);
    assert_eq!(path.root_hex, registered.root_hex);
}
