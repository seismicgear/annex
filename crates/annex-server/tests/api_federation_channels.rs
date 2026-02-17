use annex_db::run_migrations;
use annex_identity::{zk, MerkleTree};
use annex_server::{api_ws, app, AppState};
use annex_types::ServerPolicy;
use annex_voice::{LiveKitConfig, SttService, TtsService, VoiceService};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use ed25519_dalek::{Signer, SigningKey};
use rand::{rngs::OsRng, RngCore};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tempfile::TempDir;
use tower::ServiceExt; // for `oneshot`

async fn setup_app() -> (axum::Router, Arc<AppState>, TempDir) {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    let db_path_str = db_path.to_str().expect("invalid db path");

    // 1. Setup DB
    let pool = annex_db::create_pool(
        db_path_str,
        annex_db::DbRuntimeSettings {
            busy_timeout_ms: 5000,
            pool_max_size: 5,
        },
    )
    .expect("failed to create pool");

    let conn = pool.get().expect("failed to get conn");
    run_migrations(&conn).expect("failed to run migrations");

    // 2. Setup Server & Policy
    let policy = ServerPolicy::default();
    let policy_json = serde_json::to_string(&policy).unwrap();
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('test-server', 'Test Server', ?1)",
        [policy_json],
    )
    .expect("failed to insert server");
    let server_id: i64 = conn.last_insert_rowid();

    // 3. Setup Merkle Tree
    let tree = MerkleTree::restore(&conn, 20).expect("failed to restore tree");

    // 4. Setup Services (Dummy)
    let membership_vkey = zk::generate_dummy_vkey();
    let (presence_tx, _) = tokio::sync::broadcast::channel(100);

    let voice_config = LiveKitConfig {
        url: "http://localhost:7880".to_string(),
        api_key: "devkey".to_string(),
        api_secret: "secret".to_string(),
    };
    let voice_service = VoiceService::new(voice_config);
    let tts_service = TtsService::new("dummy/voices", "dummy/piper");
    let stt_service = SttService::new("dummy/model.bin", "dummy/whisper");

    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(membership_vkey),
        server_id,
        policy: Arc::new(RwLock::new(policy)),
        rate_limiter: annex_server::middleware::RateLimiter::new(),
        connection_manager: api_ws::ConnectionManager::new(),
        presence_tx,
        voice_service: Arc::new(voice_service),
        tts_service: Arc::new(tts_service),
        stt_service: Arc::new(stt_service),
        voice_sessions: Arc::new(RwLock::new(HashMap::new())),
    };

    let router = app(state.clone());
    (router, Arc::new(state), temp_dir)
}

#[tokio::test]
async fn test_list_federated_channels() {
    let (app, state, _temp_dir) = setup_app().await;
    {
        let conn = state.pool.get().unwrap();

        // Insert a local channel
        conn.execute(
            r#"INSERT INTO channels (
                server_id, channel_id, name, channel_type, federation_scope
            ) VALUES (?1, 'local-1', 'Local Only', '"Text"', '"Local"')"#,
            rusqlite::params![state.server_id],
        )
        .unwrap();

        // Insert a federated channel
        conn.execute(
            r#"INSERT INTO channels (
                server_id, channel_id, name, channel_type, federation_scope
            ) VALUES (?1, 'fed-1', 'Global Chat', '"Text"', '"Federated"')"#,
            rusqlite::params![state.server_id],
        )
        .unwrap();
    }

    // Call GET /api/federation/channels
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/federation/channels")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 8080))))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    if response.status() != StatusCode::OK {
        let status = response.status();
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8_lossy(&body_bytes);
        panic!("Request failed with status {}: {}", status, body_str);
    }

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let channels: Vec<serde_json::Value> = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0]["channel_id"], "fed-1");
    assert_eq!(channels[0]["name"], "Global Chat");
}

#[tokio::test]
async fn test_join_federated_channel() {
    let (app, state, _temp_dir) = setup_app().await;
    let channel_id = "fed-join-test";
    let pseudonym_id = "remote-user-1";

    let payload = {
        let conn = state.pool.get().unwrap();

        // Insert a federated channel
        conn.execute(
            r#"INSERT INTO channels (
                server_id, channel_id, name, channel_type, federation_scope
            ) VALUES (?1, ?2, 'Federated Join', '"Text"', '"Federated"')"#,
            rusqlite::params![state.server_id, channel_id],
        )
        .unwrap();

        // Setup remote instance keypair
        let mut csprng = OsRng;
        let mut key_bytes = [0u8; 32];
        csprng.fill_bytes(&mut key_bytes);
        let signing_key = SigningKey::from_bytes(&key_bytes);
        let verifying_key = signing_key.verifying_key();
        let public_key_hex = hex::encode(verifying_key.as_bytes());

        // Insert remote instance
        let remote_base_url = "https://remote.example.com";
        conn.execute(
            "INSERT INTO instances (base_url, public_key, label, status) VALUES (?1, ?2, 'Remote', 'ACTIVE')",
            rusqlite::params![remote_base_url, public_key_hex],
        )
        .unwrap();
        let remote_instance_id = conn.last_insert_rowid();

        // Insert federated identity (simulate prior attestation)
        conn.execute(
            "INSERT INTO federated_identities (server_id, remote_instance_id, commitment_hex, pseudonym_id, vrp_topic) VALUES (?1, ?2, 'commit-hex', ?3, 'topic')",
            rusqlite::params![state.server_id, remote_instance_id, pseudonym_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (?1, ?2, 'HUMAN', 1)",
            rusqlite::params![state.server_id, pseudonym_id],
        ).unwrap();

        // Generate signature: SHA256(channel_id + pseudonym_id) -> signed
        let message = format!("{}{}", channel_id, pseudonym_id);

        let signature = signing_key.sign(message.as_bytes());
        let signature_hex = hex::encode(signature.to_bytes());

        json!({
            "originating_server": remote_base_url,
            "pseudonym_id": pseudonym_id,
            "signature": signature_hex
        })
    };

    // Call POST /api/federation/channels/:id/join
    let uri = format!("/api/federation/channels/{}/join", channel_id);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("Content-Type", "application/json")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 8080))))
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    if response.status() != StatusCode::OK {
        let status = response.status();
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8_lossy(&body_bytes);
        panic!("Request failed with status {}: {}", status, body_str);
    }

    assert_eq!(response.status(), StatusCode::OK);

    // Verify member added
    let conn = state.pool.get().unwrap();
    let is_member: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM channel_members WHERE channel_id = ?1 AND pseudonym_id = ?2)",
        rusqlite::params![channel_id, pseudonym_id],
        |row| row.get(0),
    ).unwrap();

    assert!(is_member, "Remote user should be added to channel members");
}
