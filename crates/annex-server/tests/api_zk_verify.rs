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
        membership_vkey: Arc::new(annex_identity::zk::parse_verification_key(
            &fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../zk/keys/membership_vkey.json"))
                .expect("real membership verification key is required; install the pinned ceremony artifacts"),
        ).expect("real membership verification key must be valid")),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: std::sync::Arc::new(std::sync::RwLock::new(
            "http://localhost:3000".to_string(),
        )),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::WebRtcConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper", "bark")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        invite_base_url: "https://monolithannex.com/invite".to_string(),
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
        voice_token_secret: std::sync::Arc::new([0u8; 32]),
        federation_transport: Default::default(),
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: std::sync::Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
        shutdown: Default::default(),
        metrics: Default::default(),
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
        .uri(format!("/api/registry/path/{commitment_hex}"))
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

    // Real proof generation is required; absent artifacts cannot count as a pass.
    let zk_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../zk");
    let wasm_path = zk_dir.join("build/membership_js/membership.wasm");
    let zkey_path = zk_dir.join("keys/membership_final.zkey");
    let snarkjs_cmd = zk_dir.join("node_modules/snarkjs/build/cli.cjs");
    for path in [&wasm_path, &zkey_path, &snarkjs_cmd] {
        assert!(path.is_file(), "real proof prerequisite missing: {}; install the pinned ZK artifacts and npm dependencies", path.display());
    }

    let sk_bigint = num_bigint::BigInt::parse_bytes(sk.as_bytes(), 16).unwrap();

    let input_json = serde_json::json!({
        "sk": sk_bigint.to_string(),
        "roleCode": role_code as u8,
        "nodeId": node_id,
        "leafIndex": path_data.leaf_index,
        "pathElements": path_data.path_elements.iter().map(|s| format!("0x{s}")).collect::<Vec<_>>(),
        "pathIndexBits": path_data.path_indices
    });

    let temp_dir = tempfile::tempdir().expect("create isolated proof directory");
    let input_path = temp_dir.path().join("input.json");
    let proof_path = temp_dir.path().join("proof.json");
    let public_path = temp_dir.path().join("public.json");

    fs::write(&input_path, input_json.to_string()).expect("failed to write input.json");
    // .bin/snarkjs is a shell shim on Windows. The package entry point is portable.
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
        .expect("node must execute the real snarkjs proof generator");
    assert!(
        output.status.success(),
        "real snarkjs proof generation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

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

    // 7. Re-verification by the SAME identity must succeed idempotently.
    //
    // This assertion previously expected 409 CONFLICT, which looked like it
    // was enforcing the single-use nullifier. In practice it locked real
    // members out permanently: a client whose cached proof no longer matches
    // the current Merkle root (any time someone else registers) re-proves and
    // re-submits here, and a 409 left them with no way back into a server
    // they had already joined. On a multi-user server that meant everyone
    // except the most recent joiner.
    //
    // Re-presenting the nullifier confers nothing: the caller has already
    // produced a valid Groth16 proof against an acceptable root with public
    // signals cross-checked against the claimed commitment. Because the
    // pseudonym derives deterministically from (topic, nullifier), a
    // re-verification necessarily resolves to the SAME pseudonym — so the
    // double-join property the nullifier exists to enforce is preserved.
    let mut verify_req_dup = Request::builder()
        .uri("/api/zk/verify-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(verify_body.to_string()))
        .unwrap();
    verify_req_dup.extensions_mut().insert(ConnectInfo(addr));

    let verify_resp_dup = app.oneshot(verify_req_dup).await.unwrap();
    assert_eq!(
        verify_resp_dup.status(),
        StatusCode::OK,
        "an existing member re-authenticating must not be locked out"
    );

    let dup_bytes = axum::body::to_bytes(verify_resp_dup.into_body(), usize::MAX)
        .await
        .unwrap();
    let dup_data: VerifyMembershipResponse = serde_json::from_slice(&dup_bytes).unwrap();

    assert!(dup_data.ok);
    assert_eq!(
        dup_data.pseudonym_id, verify_data.pseudonym_id,
        "re-authentication must resolve to the same pseudonym — otherwise one \
         identity could accumulate multiple pseudonyms in a single topic, which \
         is exactly what the nullifier exists to prevent"
    );
    assert!(
        !dup_data.session_token.is_empty(),
        "re-authentication must issue a usable session token"
    );
}
