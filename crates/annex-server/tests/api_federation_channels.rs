use annex_db::run_migrations;
use annex_identity::{zk, MerkleTree};
use annex_server::{api_ws, app, AppState};
use annex_types::ServerPolicy;
use annex_voice::{SttService, TtsService, VoiceService, WebRtcConfig};
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

    let voice_config = WebRtcConfig::new("http://localhost:7880", "devkey", "secret");
    let voice_service = VoiceService::new(voice_config);
    let tts_service = TtsService::new("dummy/voices", "dummy/piper", "dummy/bark");
    let stt_service = SttService::new("dummy/model.bin", "dummy/whisper");

    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(membership_vkey),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: std::sync::Arc::new(std::sync::RwLock::new(
            "http://localhost:3000".to_string(),
        )),
        policy: Arc::new(RwLock::new(policy)),
        rate_limiter: annex_server::middleware::RateLimiter::new(),
        connection_manager: api_ws::ConnectionManager::new(),
        presence_tx,
        voice_service: Arc::new(voice_service),
        tts_service: Arc::new(tts_service),
        stt_service: Arc::new(stt_service),
        voice_sessions: Arc::new(RwLock::new(HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        invite_base_url: "https://monolithannex.com/invite".to_string(),
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
        voice_token_secret: std::sync::Arc::new([0u8; 32]),
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: std::sync::Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
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
        panic!("Request failed with status {status}: {body_str}");
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

        // Insert Active Federation Agreement (Required for join)
        conn.execute(
            "INSERT INTO federation_agreements (
                local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active
            ) VALUES (?1, ?2, 'ALIGNED', 'REFLECTION_SUMMARIES_ONLY', '{}', 1)",
            rusqlite::params![state.server_id, remote_instance_id],
        ).unwrap();

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

        // Generate signature (newline-delimited to match server)
        let message = format!("{channel_id}\n{pseudonym_id}");

        let signature = signing_key.sign(message.as_bytes());
        let signature_hex = hex::encode(signature.to_bytes());

        json!({
            "originating_server": remote_base_url,
            "pseudonym_id": pseudonym_id,
            "signature": signature_hex
        })
    };

    // Call POST /api/federation/channels/:id/join
    let uri = format!("/api/federation/channels/{channel_id}/join");
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
        panic!("Request failed with status {status}: {body_str}");
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

/// A peer must not be able to join a channel the operator kept local.
///
/// `receive_federated_message` verifies the channel exists AND that its
/// `federation_scope` is `Federated`, refusing anything else. The join path
/// next to it did neither: it checked the instance, the agreement, the
/// signature and the attestation, then called `add_member` on whatever
/// `channel_id` was in the URL. So `federation_scope` — the operator's
/// declaration of which channels leave this server — governed messages and
/// not membership, and a peer could enrol its users in a `LOCAL_ONLY`
/// channel. Those pseudonyms then show up in the member list, in
/// `list_members`, and in the trust graph.
///
/// The message path's scope check still blocked injected content, so this
/// was contained rather than catastrophic — but a gate that only half the
/// sibling paths apply is exactly the shape that eventually lets something
/// through.
#[tokio::test]
async fn a_peer_cannot_join_a_local_only_channel() {
    let (app, state, _temp_dir) = setup_app().await;
    let channel_id = "local-only-chan";
    let pseudonym_id = "remote-user-x";

    let payload = {
        let conn = state.pool.get().unwrap();

        // The operator marked this channel local. That is the whole point.
        conn.execute(
            r#"INSERT INTO channels (
                server_id, channel_id, name, channel_type, federation_scope
            ) VALUES (?1, ?2, 'Private Ops', '"Text"', '"Local"')"#,
            rusqlite::params![state.server_id, channel_id],
        )
        .unwrap();

        let mut csprng = OsRng;
        let mut key_bytes = [0u8; 32];
        csprng.fill_bytes(&mut key_bytes);
        let signing_key = SigningKey::from_bytes(&key_bytes);
        let public_key_hex = hex::encode(signing_key.verifying_key().as_bytes());

        let remote_base_url = "https://remote.example.com";
        conn.execute(
            "INSERT INTO instances (base_url, public_key, label, status) VALUES (?1, ?2, 'Remote', 'ACTIVE')",
            rusqlite::params![remote_base_url, public_key_hex],
        )
        .unwrap();
        let remote_instance_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO federation_agreements (
                local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active
            ) VALUES (?1, ?2, 'ALIGNED', 'REFLECTION_SUMMARIES_ONLY', '{}', 1)",
            rusqlite::params![state.server_id, remote_instance_id],
        ).unwrap();

        conn.execute(
            "INSERT INTO federated_identities (server_id, remote_instance_id, commitment_hex, pseudonym_id, vrp_topic) VALUES (?1, ?2, 'commit-hex', ?3, 'topic')",
            rusqlite::params![state.server_id, remote_instance_id, pseudonym_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (?1, ?2, 'HUMAN', 1)",
            rusqlite::params![state.server_id, pseudonym_id],
        ).unwrap();

        // Everything else about this request is legitimate — a real peer,
        // a real agreement, a valid signature, an attested identity. Only
        // the channel's scope should stop it.
        let message = format!("{channel_id}\n{pseudonym_id}");
        let signature_hex = hex::encode(signing_key.sign(message.as_bytes()).to_bytes());

        json!({
            "originating_server": remote_base_url,
            "pseudonym_id": pseudonym_id,
            "signature": signature_hex
        })
    };

    let uri = format!("/api/federation/channels/{channel_id}/join");
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("Content-Type", "application/json")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 8080))))
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        response.status(),
        StatusCode::OK,
        "a peer joined a LOCAL_ONLY channel",
    );

    let conn = state.pool.get().unwrap();
    let members: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM channel_members WHERE channel_id = ?1",
            [channel_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(members, 0, "a membership row was written anyway");
}

/// And a channel that does not exist at all must not gain a member.
#[tokio::test]
async fn a_peer_cannot_join_a_channel_that_does_not_exist() {
    let (app, state, _temp_dir) = setup_app().await;
    let channel_id = "no-such-channel";
    let pseudonym_id = "remote-user-y";

    let payload = {
        let conn = state.pool.get().unwrap();
        let mut csprng = OsRng;
        let mut key_bytes = [0u8; 32];
        csprng.fill_bytes(&mut key_bytes);
        let signing_key = SigningKey::from_bytes(&key_bytes);
        let public_key_hex = hex::encode(signing_key.verifying_key().as_bytes());

        let remote_base_url = "https://remote.example.com";
        conn.execute(
            "INSERT INTO instances (base_url, public_key, label, status) VALUES (?1, ?2, 'Remote', 'ACTIVE')",
            rusqlite::params![remote_base_url, public_key_hex],
        )
        .unwrap();
        let remote_instance_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO federation_agreements (
                local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active
            ) VALUES (?1, ?2, 'ALIGNED', 'REFLECTION_SUMMARIES_ONLY', '{}', 1)",
            rusqlite::params![state.server_id, remote_instance_id],
        ).unwrap();
        conn.execute(
            "INSERT INTO federated_identities (server_id, remote_instance_id, commitment_hex, pseudonym_id, vrp_topic) VALUES (?1, ?2, 'commit-hex', ?3, 'topic')",
            rusqlite::params![state.server_id, remote_instance_id, pseudonym_id],
        )
        .unwrap();

        let message = format!("{channel_id}\n{pseudonym_id}");
        let signature_hex = hex::encode(signing_key.sign(message.as_bytes()).to_bytes());
        json!({
            "originating_server": remote_base_url,
            "pseudonym_id": pseudonym_id,
            "signature": signature_hex
        })
    };

    let uri = format!("/api/federation/channels/{channel_id}/join");
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("Content-Type", "application/json")
                .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 8080))))
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        response.status(),
        StatusCode::OK,
        "joined a phantom channel"
    );
}
