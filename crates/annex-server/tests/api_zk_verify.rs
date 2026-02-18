use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::{generate_commitment, MerkleTree, RoleCode};
use annex_server::{
    api::{GetPathResponse, VerifyMembershipResponse},
    app,
    middleware::RateLimiter,
    AppState,
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
async fn test_verify_membership_flow() {
    // 1. Setup
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Seed server
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('default', 'Default Server', '{}')",
        [],
    )
    .unwrap();

    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: "http://localhost:3000".to_string(),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::LiveKitConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
    };
    let app = app(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // 2. Generate Identity
    // Use simple values for deterministic testing
    let sk = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    let role_code = RoleCode::Human; // 1
    let node_id = 42;

    let commitment_hex = generate_commitment(sk, role_code, node_id).unwrap();

    // 3. Register Identity
    let register_body = serde_json::json!({
        "commitmentHex": commitment_hex,
        "roleCode": role_code as u8,
        "nodeId": node_id
    });

    let mut reg_req = Request::builder()
        .uri("/api/registry/register")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(register_body.to_string()))
        .unwrap();
    reg_req.extensions_mut().insert(ConnectInfo(addr));

    let reg_resp = app.clone().oneshot(reg_req).await.unwrap();
    assert_eq!(reg_resp.status(), StatusCode::OK);

    // 4. Get Path
    let mut path_req = Request::builder()
        .uri(format!("/api/registry/path/{}", commitment_hex))
        .method("GET")
        .body(Body::empty())
        .unwrap();
    path_req.extensions_mut().insert(ConnectInfo(addr));

    let path_resp = app.clone().oneshot(path_req).await.unwrap();
    assert_eq!(path_resp.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(path_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let path_data: GetPathResponse = serde_json::from_slice(&body_bytes).unwrap();

    // 5. Generate Proof via SnarkJS
    // Construct input JSON
    let zk_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../zk");
    let build_dir = zk_dir.join("build");
    let keys_dir = zk_dir.join("keys");
    let node_modules_bin = zk_dir.join("node_modules/.bin");

    // Ensure paths exist
    assert!(build_dir.exists(), "zk build dir missing");
    assert!(keys_dir.exists(), "zk keys dir missing");

    // Convert hex sk to decimal string for circom input if needed, or pass hex if snarkjs supports it
    // Usually snarkjs expects decimal strings or hex with 0x prefix for big numbers
    // Let's use BigInt to be safe
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

    // Run snarkjs
    // Command: snarkjs groth16 fullprove input.json wasm zkey proof.json public.json
    let snarkjs_cmd = node_modules_bin.join("snarkjs");

    // Using npx if local binary doesn't work directly, but local binary path is safer if predictable
    // Or just "npx snarkjs" from zk dir

    let output = Command::new("node")
        .arg(snarkjs_cmd)
        .arg("groth16")
        .arg("fullprove")
        .arg(&input_path)
        .arg(&wasm_path)
        .arg(&zkey_path)
        .arg(&proof_path)
        .arg(&public_path)
        .current_dir(&zk_dir) // Run from zk dir to ensure node resolution works if needed
        .output()
        .expect("failed to execute snarkjs");

    if !output.status.success() {
        println!(
            "snarkjs stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        println!(
            "snarkjs stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        panic!("snarkjs failed");
    }

    let proof_str = fs::read_to_string(&proof_path).expect("failed to read proof.json");
    let public_str = fs::read_to_string(&public_path).expect("failed to read public.json");

    let proof: Value = serde_json::from_str(&proof_str).unwrap();
    let public_signals: Vec<String> = serde_json::from_str(&public_str).unwrap();

    // Cleanup
    let _ = fs::remove_file(input_path);
    let _ = fs::remove_file(proof_path);
    let _ = fs::remove_file(public_path);

    // 6. Verify Membership API Call
    let verify_body = serde_json::json!({
        "root": path_data.root_hex, // Use root from path response to ensure consistency
        "commitment": commitment_hex,
        "topic": "annex:server:v1",
        "proof": proof,
        "publicSignals": public_signals
    });

    let mut verify_req = Request::builder()
        .uri("/api/zk/verify-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(verify_body.to_string()))
        .unwrap();
    verify_req.extensions_mut().insert(ConnectInfo(addr));

    let verify_resp = app.clone().oneshot(verify_req).await.unwrap();

    let status = verify_resp.status();
    let body_bytes = axum::body::to_bytes(verify_resp.into_body(), usize::MAX)
        .await
        .unwrap();

    if status != StatusCode::OK {
        println!(
            "Verify failed body: {:?}",
            String::from_utf8_lossy(&body_bytes)
        );
    }

    assert_eq!(status, StatusCode::OK);

    let verify_data: VerifyMembershipResponse = serde_json::from_slice(&body_bytes).unwrap();

    assert!(verify_data.ok);
    assert!(!verify_data.pseudonym_id.is_empty());

    // 7. Verify duplicate submission fails
    let mut verify_req_dup = Request::builder()
        .uri("/api/zk/verify-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(verify_body.to_string()))
        .unwrap();
    verify_req_dup.extensions_mut().insert(ConnectInfo(addr));

    let verify_resp_dup = app.oneshot(verify_req_dup).await.unwrap();
    assert_eq!(verify_resp_dup.status(), StatusCode::CONFLICT);
}
