use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::{generate_commitment, MerkleTree, RoleCode};
use annex_server::{
    api::{GetPathResponse, VerifyMembershipResponse},
    app,
    middleware::RateLimiter,
    AppState,
};
use annex_types::ServerPolicy;
use annex_vrp::{
    VrpAlignmentStatus, VrpAnchorSnapshot, VrpCapabilitySharingContract, VrpFederationHandshake,
    VrpValidationReport,
};
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
    // If running in CI or stripped env, ensure keys exist or skip
    if !vkey_path.exists() {
        panic!(
            "ZK keys not found at {:?}. Run 'npm run setup' in zk/ directory first.",
            vkey_path
        );
    }
    let vkey_json = fs::read_to_string(vkey_path).expect("failed to read vkey");
    let vk = annex_identity::zk::parse_verification_key(&vkey_json).expect("failed to parse vkey");
    Arc::new(vk)
}

#[tokio::test]
async fn test_agent_connection_flow_end_to_end() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_test_writer()
        .try_init();

    // 1. Setup
    // Use file::memory:?cache=shared to ensure connection pooling shares the same DB
    let pool = create_pool("file::memory:?cache=shared", DbRuntimeSettings::default())
        .expect("failed to create pool");
    let conn = pool.get().expect("failed to get connection");
    annex_db::run_migrations(&conn).expect("failed to run migrations");

    // Seed server
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'default', 'Default Server', '{}')",
        [],
    )
    .expect("failed to seed server");

    // Seed VRP roles and topics if not present (migrations should handle this, but checking)
    // 003_vrp_registry.sql seeds them.

    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
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
    };
    let app = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // 2. Step 0: Generate Identity & Pre-calculate Pseudonym
    let sk = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    let role_code = RoleCode::AiAgent; // 2
    let node_id = 99;
    let topic = "annex:server:v1";

    let commitment_hex = generate_commitment(sk, role_code, node_id).unwrap();

    // Calculate pseudonym locally
    let nullifier_hex = annex_identity::derive_nullifier_hex(&commitment_hex, topic)
        .expect("failed to derive nullifier");
    let expected_pseudonym_id = annex_identity::derive_pseudonym_id(topic, &nullifier_hex)
        .expect("failed to derive pseudonym");

    println!("Generated Identity:");
    println!("Commitment: {}", commitment_hex);
    println!("Nullifier:  {}", nullifier_hex);
    println!("Pseudonym:  {}", expected_pseudonym_id);

    // 3. Step 1: VRP Handshake
    println!("--- Step 1: VRP Handshake ---");
    let anchor = VrpAnchorSnapshot::new(&[], &[]); // Empty anchor matches default empty policy (Aligned)
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec!["TEXT".to_string()],
    };
    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };

    let handshake_payload = serde_json::json!({
        "pseudonymId": expected_pseudonym_id,
        "handshake": handshake
    });

    let mut req = Request::builder()
        .uri("/api/vrp/agent-handshake")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(handshake_payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    // Clone app for each request because oneshot consumes it
    let resp = app.clone().oneshot(req).await.unwrap();

    let status = resp.status();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();

    if status != StatusCode::OK {
        println!("Error Response: {}", String::from_utf8_lossy(&body_bytes));
    }

    assert_eq!(status, StatusCode::OK);

    let report: VrpValidationReport = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(report.alignment_status, VrpAlignmentStatus::Aligned);

    // Verify agent_registrations exists
    let conn = pool.get().unwrap();
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_registrations WHERE pseudonym_id = ?1)",
            [&expected_pseudonym_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(exists, "agent registration must exist after handshake");
    drop(conn);

    // 4. Step 2: Identity Registration
    println!("--- Step 2: Registry Registration ---");
    let register_body = serde_json::json!({
        "commitmentHex": commitment_hex,
        "roleCode": role_code as u8,
        "nodeId": node_id
    });

    let mut req = Request::builder()
        .uri("/api/registry/register")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(register_body.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 5. Step 3: Proof Generation
    println!("--- Step 3: Proof Generation ---");
    // Get path first
    let mut req = Request::builder()
        .uri(format!("/api/registry/path/{}", commitment_hex))
        .method("GET")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let path_data: GetPathResponse = serde_json::from_slice(&body_bytes).unwrap();

    // Run snarkjs
    let zk_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../zk");
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

    let wasm_path = zk_dir.join("build/membership_js/membership.wasm");
    let zkey_path = zk_dir.join("keys/membership_final.zkey");
    let snarkjs_cmd = zk_dir.join("node_modules/.bin/snarkjs");

    // Skip proof generation if environment is not set up (e.g. fast check), but roadmap says "Every new module must have unit tests".
    // We assume the environment is set up.

    let output = Command::new("node")
        .arg(snarkjs_cmd)
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
        println!(
            "snarkjs stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        panic!("snarkjs failed");
    }

    let proof_str = fs::read_to_string(&proof_path).expect("failed to read proof.json");
    let public_str = fs::read_to_string(&public_path).expect("failed to read public.json");
    let proof: Value = serde_json::from_str(&proof_str).unwrap();
    let public_signals: Vec<String> = serde_json::from_str(&public_str).unwrap();

    // Cleanup temp files
    let _ = fs::remove_file(input_path);
    let _ = fs::remove_file(proof_path);
    let _ = fs::remove_file(public_path);

    // 6. Step 4: Membership Verification
    println!("--- Step 4: Membership Verification ---");
    let verify_body = serde_json::json!({
        "root": path_data.root_hex,
        "commitment": commitment_hex,
        "topic": topic,
        "proof": proof,
        "publicSignals": public_signals
    });

    let mut req = Request::builder()
        .uri("/api/zk/verify-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(verify_body.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let verify_data: VerifyMembershipResponse = serde_json::from_slice(&body_bytes).unwrap();

    // Verify pseudonym matches
    assert_eq!(
        verify_data.pseudonym_id, expected_pseudonym_id,
        "Pseudonym mismatch between derivation and server verification"
    );

    // Verify platform_identity is active
    let conn = pool.get().unwrap();
    let active: i32 = conn
        .query_row(
            "SELECT active FROM platform_identities WHERE pseudonym_id = ?1",
            [&expected_pseudonym_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active, 1, "Platform identity should be active");
    drop(conn);

    // 7. Step 5 & 6: Join Channel (Simulated via API, bypassing WS for brevity as WS tests are in api_ws.rs)
    // To properly simulate WS connection, we'd need to spin up the actual server on a port.
    // For this integration test, we verify the *logic* of the flow by calling the endpoints directly.
    // The "Step 5" (WS) is about authentication. We can simulate authenticated requests by adding the header.

    println!("--- Step 6: Join Channel ---");
    // Create a channel first
    let conn = pool.get().unwrap();
    // channel_type and federation_scope must be JSON strings
    conn.execute(
        "INSERT INTO channels (server_id, channel_id, name, channel_type, federation_scope) VALUES (1, 'agent-channel', 'Agent Hangout', '\"Text\"', '\"Local\"')",
        [],
    ).unwrap();
    drop(conn);

    let join_body = serde_json::json!({
        "pseudonym": expected_pseudonym_id
    });

    let mut req = Request::builder()
        .uri("/api/channels/agent-channel/join")
        .method("POST")
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", &expected_pseudonym_id) // Simulate authentication
        .body(Body::from(join_body.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify membership
    let conn = pool.get().unwrap();
    let member: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM channel_members WHERE channel_id = 'agent-channel' AND pseudonym_id = ?1)",
        [&expected_pseudonym_id],
        |row| row.get(0),
    ).unwrap();
    assert!(member, "Agent should be a member of the channel");

    println!("Agent flow test completed successfully.");
}
