use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::{
    derive_nullifier_hex, derive_pseudonym_id, generate_commitment, MerkleTree, RoleCode,
};
use annex_server::{
    api::GetPathResponse, api_ws::ConnectionManager, app, middleware::RateLimiter, AppState,
};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use serde_json::Value;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    let vkey_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../zk/keys/membership_vkey.json");
    let vkey_json = fs::read_to_string(vkey_path).expect("failed to read vkey");
    let vk = annex_identity::zk::parse_verification_key(&vkey_json).expect("failed to parse vkey");
    Arc::new(vk)
}

#[tokio::test]
async fn test_graph_node_creation_on_verification() {
    // ... (Existing setup code)
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('default', 'Default Server', '{}')",
        [],
    )
    .unwrap();
    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: ConnectionManager::new(), presence_tx: tokio::sync::broadcast::channel(100).0,
    };
    let app = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // Generate Identity
    let sk = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    let role_code = RoleCode::Human;
    let node_id = 99;
    let commitment_hex = generate_commitment(sk, role_code, node_id).unwrap();

    // Register
    let register_body = serde_json::json!({
        "commitmentHex": commitment_hex,
        "roleCode": role_code as u8,
        "nodeId": node_id
    });
    let req = Request::builder()
        .uri("/api/registry/register")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(register_body.to_string()))
        .unwrap();
    let req = {
        let mut r = req;
        r.extensions_mut().insert(ConnectInfo(addr));
        r
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Get Path
    let req = Request::builder()
        .uri(format!("/api/registry/path/{}", commitment_hex))
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let req = {
        let mut r = req;
        r.extensions_mut().insert(ConnectInfo(addr));
        r
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let path_data: GetPathResponse = serde_json::from_slice(&body_bytes).unwrap();

    // ZK Proof Generation (Shell out)
    let zk_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../zk");
    let build_dir = zk_dir.join("build");
    let keys_dir = zk_dir.join("keys");
    if !build_dir.exists() || !keys_dir.exists() {
        println!("ZK artifacts missing, skipping test");
        return;
    }

    let sk_bigint = num_bigint::BigInt::parse_bytes(sk.as_bytes(), 16).unwrap();
    let input_json = serde_json::json!({
        "sk": sk_bigint.to_string(),
        "roleCode": role_code as u8,
        "nodeId": node_id,
        "leafIndex": path_data.leaf_index,
        "pathElements": path_data.path_elements.iter().map(|s| format!("0x{}", s)).collect::<Vec<_>>(),
        "pathIndexBits": path_data.path_indices
    });

    let temp_dir = std::env::temp_dir();
    let unique_id = uuid::Uuid::new_v4();
    let input_path = temp_dir.join(format!("input-{}.json", unique_id));
    let proof_path = temp_dir.join(format!("proof-{}.json", unique_id));
    let public_path = temp_dir.join(format!("public-{}.json", unique_id));

    fs::write(&input_path, input_json.to_string()).expect("failed to write input.json");
    let wasm_path = build_dir.join("membership_js/membership.wasm");
    let zkey_path = keys_dir.join("membership_final.zkey");
    let snarkjs_cmd = if zk_dir.join("node_modules/.bin/snarkjs").exists() {
        zk_dir.join("node_modules/.bin/snarkjs")
    } else {
        PathBuf::from("snarkjs")
    };
    let output = Command::new("node")
        .arg(&snarkjs_cmd)
        .arg("groth16")
        .arg("fullprove")
        .arg(&input_path)
        .arg(&wasm_path)
        .arg(&zkey_path)
        .arg(&proof_path)
        .arg(&public_path)
        .current_dir(&zk_dir)
        .output()
        .expect("failed to execute snarkjs");

    if !output.status.success() {
        panic!("snarkjs failed");
    }

    let proof_str = fs::read_to_string(&proof_path).expect("failed to read proof.json");
    let public_str = fs::read_to_string(&public_path).expect("failed to read public.json");
    let proof: Value = serde_json::from_str(&proof_str).unwrap();
    let public_signals: Vec<String> = serde_json::from_str(&public_str).unwrap();
    let _ = fs::remove_file(input_path);
    let _ = fs::remove_file(proof_path);
    let _ = fs::remove_file(public_path);

    // --- TEST IDEMPOTENCY / ROBUSTNESS ---

    // 1. Manually insert the graph node to simulate it already exists.
    // We need to calculate pseudonym_id first.
    let topic = "annex:server:v1";
    let nullifier_hex = derive_nullifier_hex(&commitment_hex, topic).unwrap();
    let pseudonym_id = derive_pseudonym_id(topic, &nullifier_hex).unwrap();

    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO graph_nodes (server_id, pseudonym_id, node_type, active, last_seen_at)
         VALUES (1, ?1, 'HUMAN', 0, '2000-01-01T00:00:00Z')",
        [&pseudonym_id],
    )
    .unwrap();
    drop(conn);

    // 2. Verify Membership
    // This should proceed (because nullifier is not in DB yet) and "upsert" the graph node,
    // setting active=1 and last_seen_at=now.
    let verify_body = serde_json::json!({
        "root": path_data.root_hex,
        "commitment": commitment_hex,
        "topic": topic,
        "proof": proof,
        "publicSignals": public_signals
    });

    let req = Request::builder()
        .uri("/api/zk/verify-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(verify_body.to_string()))
        .unwrap();
    let req = {
        let mut r = req;
        r.extensions_mut().insert(ConnectInfo(addr));
        r
    };

    let resp = app.clone().oneshot(req).await.unwrap();

    // If it wasn't robust, this would be 500. Robust means 200.
    assert_eq!(resp.status(), StatusCode::OK);

    // 3. Verify graph node was updated
    let conn = pool.get().unwrap();
    let (active, last_seen_at): (bool, String) = conn
        .query_row(
            "SELECT active, last_seen_at FROM graph_nodes WHERE pseudonym_id = ?1",
            [&pseudonym_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert!(active, "Node should be active");
    assert_ne!(
        last_seen_at, "2000-01-01T00:00:00Z",
        "last_seen_at should be updated"
    );
}
