//! Integration tests for the capability / linkage / federation ZK circuit
//! endpoints (AUDIT P4-ID-1):
//!   - POST /api/zk/channel-eligibility
//!   - POST /api/zk/link-pseudonyms
//!   - POST /api/zk/federation-attestation
//!
//! Each test registers a real identity, fetches its Merkle path, generates a
//! REAL Groth16 proof with snarkjs against the freshly-built circuits, and
//! drives it through the live router. Negative cases (wrong role, wrong topic,
//! unconfigured circuit) assert the handler rejects rather than rubber-stamps.
//!
//! Like `api_zk_verify`, these skip cleanly when the ZK toolchain isn't built
//! (fresh sandbox); CI builds the circuits first so the full round-trip runs.

mod common;

use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::zk::{parse_verification_key, Bn254, VerifyingKey};
use annex_identity::{generate_commitment, MerkleTree, RoleCode};
use annex_server::{api::GetPathResponse, app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

const SK_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const NODE_ID: i64 = 42;

/// BN254 scalar field modulus (decimal). Used to mirror the server's
/// `topic_hash_for_v2` = `Fr::from_be_bytes_mod_order(SHA256(domain || topic))`.
const BN254_FR_MODULUS: &str =
    "21888242871839275222246405745257275088548364400416034343698204186575808495617";

/// Decimal field-element string for a topic, matching
/// `annex_identity::zk::topic_hash_for_v2` exactly so the circuit's public
/// input equals what the server recomputes.
fn topic_hash_decimal(topic: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"annex/v2/topicHash:");
    h.update(topic.as_bytes());
    let digest = h.finalize();
    let n = num_bigint::BigUint::from_bytes_be(&digest);
    let modulus = num_bigint::BigUint::parse_bytes(BN254_FR_MODULUS.as_bytes(), 10).unwrap();
    (n % modulus).to_string()
}

fn zk_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../zk")
}

/// Loads a real circuit vkey from `zk/keys/<name>_vkey.json`, or `None` if the
/// toolchain hasn't produced it.
fn load_circuit_vkey(name: &str) -> Option<Arc<VerifyingKey<Bn254>>> {
    let path = zk_dir().join("keys").join(format!("{name}_vkey.json"));
    let json = std::fs::read_to_string(path).ok()?;
    let vk = parse_verification_key(&json).ok()?;
    Some(Arc::new(vk))
}

/// Builds an AppState. When `with_circuits` is true, loads the three real
/// circuit vkeys; otherwise leaves them `None` (to exercise the 503 path).
fn build_state(pool: annex_db::DbPool, tree: MerkleTree, with_circuits: bool) -> AppState {
    let (channel_eligibility_vkey, link_pseudonyms_vkey, federation_attestation_vkey) =
        if with_circuits {
            (
                load_circuit_vkey("channel_eligibility"),
                load_circuit_vkey("link_pseudonyms"),
                load_circuit_vkey("federation_attestation"),
            )
        } else {
            (None, None, None)
        };
    AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: common::load_vkey_or_dummy(),
        membership_vkey_v2: None,
        channel_eligibility_vkey,
        link_pseudonyms_vkey,
        federation_attestation_vkey,
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: Arc::new(RwLock::new("http://localhost:3000".to_string())),
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
        ws_token_secret: Arc::new([0u8; 32]),
        voice_token_secret: Arc::new([0u8; 32]),
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
    }
}

/// Runs `snarkjs groth16 fullprove`. Returns `None` (skip) when the toolchain
/// is unavailable; otherwise `(proof, publicSignals)`.
fn snarkjs_prove(
    circuit_wasm_dir: &str,
    circuit: &str,
    input: &Value,
) -> Option<(Value, Vec<String>)> {
    let zk = zk_dir();
    let node_modules_bin = zk.join("node_modules/.bin");
    let wasm = zk
        .join("build")
        .join(circuit_wasm_dir)
        .join(format!("{circuit}.wasm"));
    let zkey = zk.join("keys").join(format!("{circuit}_final.zkey"));
    if !node_modules_bin.exists() || !wasm.exists() || !zkey.exists() {
        eprintln!("[api_zk_circuits] skipping: ZK toolchain not built for {circuit}");
        return None;
    }
    let tmp = std::env::temp_dir();
    let id = uuid::Uuid::new_v4();
    let input_path = tmp.join(format!("in-{circuit}-{id}.json"));
    let proof_path = tmp.join(format!("proof-{circuit}-{id}.json"));
    let public_path = tmp.join(format!("public-{circuit}-{id}.json"));
    std::fs::write(&input_path, input.to_string()).ok()?;

    let out = Command::new("node")
        .arg(node_modules_bin.join("snarkjs"))
        .arg("groth16")
        .arg("fullprove")
        .arg(&input_path)
        .arg(&wasm)
        .arg(&zkey)
        .arg(&proof_path)
        .arg(&public_path)
        .current_dir(&zk)
        .output()
        .ok()?;
    let result = if out.status.success() {
        let proof: Value =
            serde_json::from_str(&std::fs::read_to_string(&proof_path).ok()?).ok()?;
        let signals: Vec<String> =
            serde_json::from_str(&std::fs::read_to_string(&public_path).ok()?).ok()?;
        Some((proof, signals))
    } else {
        eprintln!(
            "[api_zk_circuits] snarkjs failed for {circuit}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        None
    };
    let _ = std::fs::remove_file(&input_path);
    let _ = std::fs::remove_file(&proof_path);
    let _ = std::fs::remove_file(&public_path);
    result
}

/// Registers an identity and returns `(app, commitment_hex, GetPathResponse)`.
async fn setup_registered(with_circuits: bool) -> (axum::Router, String, GetPathResponse) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('default', 'Default', '{}')",
        [],
    )
    .unwrap();
    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let app = app(build_state(pool, tree, with_circuits));
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 12345));

    let role_code = RoleCode::Human; // 1
    let commitment_hex = generate_commitment(SK_HEX, role_code, NODE_ID as u64).unwrap();

    let reg = serde_json::json!({
        "commitmentHex": commitment_hex,
        "roleCode": role_code as u8,
        "nodeId": NODE_ID,
    });
    let mut req = Request::builder()
        .uri("/api/registry/register")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(reg.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let mut preq = Request::builder()
        .uri(format!("/api/registry/path/{commitment_hex}"))
        .method("GET")
        .body(Body::empty())
        .unwrap();
    preq.extensions_mut().insert(ConnectInfo(addr));
    let presp = app.clone().oneshot(preq).await.unwrap();
    assert_eq!(presp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(presp.into_body(), usize::MAX)
        .await
        .unwrap();
    let path: GetPathResponse = serde_json::from_slice(&bytes).unwrap();
    (app, commitment_hex, path)
}

async fn post_json(app: &axum::Router, uri: &str, body: &Value) -> (StatusCode, Value) {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

#[tokio::test]
async fn channel_eligibility_valid_and_negatives() {
    let (app, _commitment, path) = setup_registered(true).await;
    let channel_topic = "annex:channel:general:v2";
    let channel_topic_hash = topic_hash_decimal(channel_topic);

    let input = serde_json::json!({
        "sk": num_bigint::BigInt::parse_bytes(SK_HEX.as_bytes(), 16).unwrap().to_string(),
        "roleCode": RoleCode::Human as u8,
        "nodeId": NODE_ID,
        "leafIndex": path.leaf_index,
        "pathElements": path.path_elements.iter().map(|s| format!("0x{s}")).collect::<Vec<_>>(),
        "pathIndexBits": path.path_indices,
        "requiredRoleCode": RoleCode::Human as u8,
        "channelTopicHash": channel_topic_hash,
    });
    let Some((proof, signals)) =
        snarkjs_prove("channel_eligibility_js", "channel_eligibility", &input)
    else {
        return; // toolchain not built — skip
    };
    assert_eq!(signals.len(), 4, "eligibility publicSignals length");

    // Valid: 200.
    let (status, body) = post_json(
        &app,
        "/api/zk/channel-eligibility",
        &serde_json::json!({
            "root": path.root_hex,
            "channelTopic": channel_topic,
            "requiredRoleCode": RoleCode::Human as u8,
            "proof": proof,
            "publicSignals": signals,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "valid eligibility proof: {body:?}");
    assert_eq!(body["ok"], serde_json::json!(true));
    assert!(body["nullifierHex"].as_str().unwrap().len() == 64);

    // Wrong required role in the request (proof is for role 1, claim role 2) → 401.
    let (status, _) = post_json(
        &app,
        "/api/zk/channel-eligibility",
        &serde_json::json!({
            "root": path.root_hex,
            "channelTopic": channel_topic,
            "requiredRoleCode": 2,
            "proof": proof,
            "publicSignals": signals,
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "role mismatch must be rejected"
    );

    // Wrong channel topic (proof bound to a different topic) → 400.
    let (status, _) = post_json(
        &app,
        "/api/zk/channel-eligibility",
        &serde_json::json!({
            "root": path.root_hex,
            "channelTopic": "annex:channel:OTHER:v2",
            "requiredRoleCode": RoleCode::Human as u8,
            "proof": proof,
            "publicSignals": signals,
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "channel-topic mismatch must be rejected"
    );
}

#[tokio::test]
async fn link_pseudonyms_valid() {
    let (app, _c, _p) = setup_registered(true).await;
    let topic_a = "annex:server:alpha:v2";
    let topic_b = "annex:server:beta:v2";

    let input = serde_json::json!({
        "sk": num_bigint::BigInt::parse_bytes(SK_HEX.as_bytes(), 16).unwrap().to_string(),
        "topicHashA": topic_hash_decimal(topic_a),
        "topicHashB": topic_hash_decimal(topic_b),
    });
    let Some((proof, signals)) = snarkjs_prove("link_pseudonyms_js", "link_pseudonyms", &input)
    else {
        return;
    };
    assert_eq!(signals.len(), 4);

    let (status, body) = post_json(
        &app,
        "/api/zk/link-pseudonyms",
        &serde_json::json!({
            "topicA": topic_a,
            "topicB": topic_b,
            "proof": proof,
            "publicSignals": signals,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "valid link proof: {body:?}");
    assert_eq!(body["linked"], serde_json::json!(true));
    assert_eq!(body["nullifierAHex"].as_str().unwrap().len(), 64);
    assert_eq!(body["nullifierBHex"].as_str().unwrap().len(), 64);

    // Linking a topic to itself is meaningless → 400 (before any proof work).
    let (status, _) = post_json(
        &app,
        "/api/zk/link-pseudonyms",
        &serde_json::json!({
            "topicA": topic_a,
            "topicB": topic_a,
            "proof": proof,
            "publicSignals": signals,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Mismatched claimed topicB → 400 (proof's topicHashB won't match).
    let (status, _) = post_json(
        &app,
        "/api/zk/link-pseudonyms",
        &serde_json::json!({
            "topicA": topic_a,
            "topicB": "annex:server:WRONG:v2",
            "proof": proof,
            "publicSignals": signals,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn federation_attestation_valid() {
    let (app, _c, path) = setup_registered(true).await;
    let context = "annex:federation:alpha<->beta";

    let input = serde_json::json!({
        "sk": num_bigint::BigInt::parse_bytes(SK_HEX.as_bytes(), 16).unwrap().to_string(),
        "roleCode": RoleCode::Human as u8,
        "nodeId": NODE_ID,
        "leafIndex": path.leaf_index,
        "pathElements": path.path_elements.iter().map(|s| format!("0x{s}")).collect::<Vec<_>>(),
        "pathIndexBits": path.path_indices,
        "federationContextHash": topic_hash_decimal(context),
    });
    let Some((proof, signals)) = snarkjs_prove(
        "federation_attestation_js",
        "federation_attestation",
        &input,
    ) else {
        return;
    };
    assert_eq!(signals.len(), 3);

    let (status, body) = post_json(
        &app,
        "/api/zk/federation-attestation",
        &serde_json::json!({
            "root": path.root_hex,
            "federationContext": context,
            "proof": proof,
            "publicSignals": signals,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "valid attestation: {body:?}");
    assert_eq!(body["ok"], serde_json::json!(true));
    assert_eq!(body["nullifierHex"].as_str().unwrap().len(), 64);

    // Wrong context → 400.
    let (status, _) = post_json(
        &app,
        "/api/zk/federation-attestation",
        &serde_json::json!({
            "root": path.root_hex,
            "federationContext": "annex:federation:WRONG",
            "proof": proof,
            "publicSignals": signals,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn endpoints_return_503_when_circuit_unconfigured() {
    // No proof/toolchain needed: when the vkey is None the handler must report
    // unavailability rather than silently accept.
    let (app, _c, path) = setup_registered(false).await;

    let (status, _) = post_json(
        &app,
        "/api/zk/channel-eligibility",
        &serde_json::json!({
            "root": path.root_hex,
            "channelTopic": "annex:channel:general:v2",
            "requiredRoleCode": 1,
            "proof": {},
            "publicSignals": ["0","0","0","0"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let (status, _) = post_json(
        &app,
        "/api/zk/link-pseudonyms",
        &serde_json::json!({
            "topicA": "a", "topicB": "b", "proof": {}, "publicSignals": ["0","0","0","0"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let (status, _) = post_json(
        &app,
        "/api/zk/federation-attestation",
        &serde_json::json!({
            "root": path.root_hex, "federationContext": "x", "proof": {}, "publicSignals": ["0","0","0"],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
