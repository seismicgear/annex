use annex_db::{create_pool, DbRuntimeSettings};
use annex_federation::FederatedMessageEnvelope;
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

// Mock loading vkey
fn load_dummy_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    Arc::new(annex_identity::zk::generate_dummy_vkey())
}

#[tokio::test]
async fn test_receive_federated_message() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    let policy = ServerPolicy::default();
    let policy_json = serde_json::to_string(&policy).unwrap();

    // 1. Seed Local Server
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local Server', ?1)",
        rusqlite::params![policy_json],
    )
    .unwrap();
    let local_server_id = conn.last_insert_rowid();

    // 2. Seed Remote Instance
    let mut csprng = OsRng;
    let remote_signing_key = SigningKey::generate(&mut csprng);
    let remote_public_key = remote_signing_key.verifying_key();
    let remote_public_key_hex = hex::encode(remote_public_key.as_bytes());
    let remote_origin = "http://remote-server.com";

    conn.execute(
        "INSERT INTO instances (base_url, public_key, label, status) VALUES (?1, ?2, 'Remote Server', 'ACTIVE')",
        rusqlite::params![remote_origin, remote_public_key_hex],
    ).unwrap();
    let remote_instance_id = conn.last_insert_rowid();

    // Seed Active Federation Agreement (Required for relay)
    conn.execute(
        "INSERT INTO federation_agreements (
            local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active
        ) VALUES (?1, ?2, 'ALIGNED', 'REFLECTION_SUMMARIES_ONLY', '{}', 1)",
        rusqlite::params![local_server_id, remote_instance_id],
    ).unwrap();

    // 3. Seed Federated Identity (The sender)
    let commitment = "0000000000000000000000000000000000000000000000000000000000000001";
    let topic = "annex:server:v1";
    let local_pseudonym_id = "user-local-pseudo";

    conn.execute(
        "INSERT INTO federated_identities (server_id, remote_instance_id, commitment_hex, pseudonym_id, vrp_topic, attested_at) VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
        rusqlite::params![local_server_id, remote_instance_id, commitment, local_pseudonym_id, topic],
    ).unwrap();

    // Also need platform_identity for channel membership FK
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (?1, ?2, 'HUMAN', 1)",
        rusqlite::params![local_server_id, local_pseudonym_id],
    ).unwrap();

    // 4. Seed Channel (Federated)
    let channel_id = "chan-fed";
    conn.execute(
        "INSERT INTO channels (
            server_id, channel_id, name, channel_type, federation_scope, created_at
           ) VALUES (?1, ?2, 'Federated Chat', '\"Text\"', '\"Federated\"', datetime('now'))",
        rusqlite::params![local_server_id, channel_id],
    )
    .unwrap();

    // 5. Add Member (The sender must be a member locally)
    conn.execute(
        "INSERT INTO channel_members (server_id, channel_id, pseudonym_id, role, joined_at) VALUES (?1, ?2, ?3, 'MEMBER', datetime('now'))",
        rusqlite::params![local_server_id, channel_id, local_pseudonym_id],
    ).unwrap();

    drop(conn);

    // Setup App State
    let tree = MerkleTree::new(20).unwrap();
    let signing_key = SigningKey::generate(&mut csprng);

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_dummy_vkey(),
        server_id: local_server_id,
        signing_key: Arc::new(signing_key),
        public_url: std::sync::Arc::new(std::sync::RwLock::new(
            "http://localhost:3000".to_string(),
        )),
        policy: Arc::new(RwLock::new(policy)),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::LiveKitConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper", "bark")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
    };

    let app = app(state);

    // 6. Construct Envelope
    let message_id = "msg-remote-123";
    let content = "Hello from federation!";
    let sender_pseudonym = "user-remote-pseudo";
    let attestation_ref = format!("{}:{}", topic, commitment);
    let created_at = "2023-01-01T00:00:00Z";

    let signature_input = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        message_id,
        channel_id,
        content,
        sender_pseudonym,
        remote_origin,
        attestation_ref,
        created_at
    );
    let signature = remote_signing_key.sign(signature_input.as_bytes());
    let signature_hex = hex::encode(signature.to_bytes());

    let envelope = FederatedMessageEnvelope {
        message_id: message_id.to_string(),
        channel_id: channel_id.to_string(),
        content: content.to_string(),
        sender_pseudonym: sender_pseudonym.to_string(),
        originating_server: remote_origin.to_string(),
        attestation_ref: attestation_ref.clone(),
        signature: signature_hex,
        created_at: created_at.to_string(),
    };

    // 7. POST Request
    let request = Request::builder()
        .uri("/api/federation/messages")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&envelope).unwrap()))
        .unwrap();

    // ConnectInfo usually required by RateLimiter
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    // We need to insert ConnectInfo into extensions manually or use `into_make_service_with_connect_info` in real app.
    // In test `oneshot`, we can insert it.
    let mut request = request;
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();

    if response.status() != StatusCode::OK {
        let status = response.status();
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        panic!("Request failed with {}: {}", status, body_str);
    }

    assert_eq!(response.status(), StatusCode::OK);

    // 8. Verify Persistence
    let conn = pool.get().unwrap();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE message_id = ?1 AND sender_pseudonym = ?2 AND content = ?3",
        rusqlite::params![message_id, local_pseudonym_id, content],
        |row| row.get(0),
    ).unwrap();

    assert_eq!(count, 1, "Message should be persisted with local pseudonym");
}
