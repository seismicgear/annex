use annex_channels::{add_member, create_channel, CreateChannelParams};
use annex_db::{create_pool, DbRuntimeSettings};
use annex_federation::FederatedMessageEnvelope;
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::{ChannelType, FederationScope, ServerPolicy};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use ed25519_dalek::{Signer, SigningKey};
use rand::{rngs::OsRng, RngCore};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

fn load_dummy_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    Arc::new(annex_identity::zk::generate_dummy_vkey())
}

async fn setup_app_with_db(signing_key: SigningKey) -> (axum::Router, annex_db::DbPool) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    let policy_json = serde_json::to_string(&ServerPolicy::default()).unwrap();
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local Server', ?1)",
        rusqlite::params![policy_json],
    )
    .unwrap();

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_dummy_vkey(),
        server_id: 1,
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        signing_key: Arc::new(signing_key),
        public_url: "http://localhost:3000".to_string(),
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

    (app(state), pool)
}

#[tokio::test]
async fn test_receive_federated_message() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();

    let mut csprng = OsRng;
    let mut bytes = [0u8; 32];
    csprng.fill_bytes(&mut bytes);
    let local_key = SigningKey::from_bytes(&bytes);

    let (app, pool) = setup_app_with_db(local_key).await;
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // 1. Setup Remote Instance
    let mut remote_bytes = [0u8; 32];
    csprng.fill_bytes(&mut remote_bytes);
    let remote_key = SigningKey::from_bytes(&remote_bytes);

    let remote_pub = remote_key.verifying_key();
    let remote_pub_hex = hex::encode(remote_pub.as_bytes());
    let remote_url = "http://remote.com";

    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO instances (base_url, public_key, label, status) VALUES (?1, ?2, 'Remote', 'ACTIVE')",
        rusqlite::params![remote_url, remote_pub_hex],
    )
    .unwrap();
    let remote_id = conn.last_insert_rowid();

    // 2. Attest Remote User
    let sender = "user-remote";
    conn.execute(
        "INSERT INTO federated_identities (server_id, remote_instance_id, commitment_hex, pseudonym_id, vrp_topic, attested_at)
         VALUES (1, ?1, 'dummy-commitment', ?2, 'annex:server:v1', datetime('now'))",
        rusqlite::params![remote_id, sender],
    ).unwrap();

    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type) VALUES (1, ?1, 'HUMAN')",
        rusqlite::params![sender],
    ).unwrap();

    // 3. Create Federated Channel
    let chan_params = CreateChannelParams {
        server_id: 1,
        channel_id: "chan-fed".to_string(),
        name: "Federated Chat".to_string(),
        channel_type: ChannelType::Text,
        topic: None,
        vrp_topic_binding: None,
        required_capabilities_json: None,
        agent_min_alignment: None,
        retention_days: None,
        federation_scope: FederationScope::Federated,
    };
    create_channel(&conn, &chan_params).unwrap();

    // 4. Add Member
    add_member(&conn, 1, "chan-fed", sender).unwrap();

    drop(conn);

    // 5. Construct Envelope
    let envelope = FederatedMessageEnvelope {
        message_id: "msg-1".to_string(),
        channel_id: "chan-fed".to_string(),
        content: "Hello Federation".to_string(),
        sender_pseudonym: sender.to_string(),
        originating_server: remote_url.to_string(),
        attestation_ref: "dummy".to_string(),
        signature: "".to_string(),
        created_at: "2023-01-01T00:00:00Z".to_string(),
    };

    let payload_string = format!(
        "{}{}{}{}{}{}{}",
        envelope.message_id,
        envelope.channel_id,
        envelope.content,
        envelope.sender_pseudonym,
        envelope.originating_server,
        envelope.attestation_ref,
        envelope.created_at
    );
    let signature = remote_key.sign(payload_string.as_bytes());
    let mut envelope = envelope;
    envelope.signature = hex::encode(signature.to_bytes());

    // 6. Send Request
    let mut request = Request::builder()
        .uri("/api/federation/messages")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&envelope).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    if response.status() != StatusCode::OK {
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        println!("Error response: {:?}", String::from_utf8_lossy(&body));
        panic!("Expected 200 OK, got {}", status);
    }

    // 7. Verify Message in DB
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE channel_id = 'chan-fed' AND content = 'Hello Federation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}
