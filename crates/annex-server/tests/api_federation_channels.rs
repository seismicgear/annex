use annex_channels::Channel;
use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_server::{api_federation::JoinFederatedChannelRequest, app, middleware::RateLimiter, AppState};
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

// Mock loading vkey
fn load_dummy_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    Arc::new(annex_identity::zk::generate_dummy_vkey())
}

async fn setup_app_with_db() -> (axum::Router, annex_db::DbPool) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    run_migrations(&conn).unwrap();

    // Seed server
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local Server', '{}')",
        [],
    )
    .unwrap();

    drop(conn);

    let tree = annex_identity::MerkleTree::new(20).unwrap();
    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_dummy_vkey(),
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

    (app(state), pool)
}

fn generate_signing_key() -> SigningKey {
    let mut csprng = OsRng;
    let mut bytes = [0u8; 32];
    csprng.fill_bytes(&mut bytes);
    SigningKey::from_bytes(&bytes)
}

#[tokio::test]
async fn test_list_federated_channels() {
    let (app, pool) = setup_app_with_db().await;
    let conn = pool.get().unwrap();

    // 1. Create a LOCAL channel
    annex_channels::create_channel(
        &conn,
        &annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "local-chan".to_string(),
            name: "Local Channel".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        },
    )
    .unwrap();

    // 2. Create a FEDERATED channel
    annex_channels::create_channel(
        &conn,
        &annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "fed-chan".to_string(),
            name: "Federated Channel".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Federated,
        },
    )
    .unwrap();
    drop(conn);

    // 3. Call GET /api/federation/channels
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/federation/channels")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let channels: Vec<Channel> = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].channel_id, "fed-chan");
}

#[tokio::test]
async fn test_join_federated_channel() {
    let (app, pool) = setup_app_with_db().await;
    let conn = pool.get().unwrap();

    // 1. Setup Federated Channel
    annex_channels::create_channel(
        &conn,
        &annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "fed-chan-2".to_string(),
            name: "Federated Channel 2".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Federated,
        },
    )
    .unwrap();

    // 2. Setup Remote Instance and Keys
    let signing_key = generate_signing_key();
    let verifying_key = signing_key.verifying_key();
    let public_key_hex = hex::encode(verifying_key.as_bytes());

    conn.execute(
        "INSERT INTO instances (base_url, public_key, label, status) VALUES (?1, ?2, 'Remote Server', 'ACTIVE')",
        rusqlite::params!["http://remote-server.com", public_key_hex],
    ).unwrap();

    let remote_instance_id: i64 = conn.last_insert_rowid();

    // 3. Setup Federated Identity (simulate attestation)
    let pseudonym_id = "remote-user-1";
    conn.execute(
        "INSERT INTO federated_identities (
            server_id, remote_instance_id, commitment_hex, pseudonym_id, vrp_topic
        ) VALUES (1, ?1, 'dummy-commitment', ?2, 'dummy-topic')",
        rusqlite::params![remote_instance_id, pseudonym_id],
    )
    .unwrap();

    // Also need platform identity for FK constraint in channel_members
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
         VALUES (1, ?1, 'HUMAN', 1)",
        rusqlite::params![pseudonym_id],
    ).unwrap();

    drop(conn);

    // 4. Generate Signature
    let channel_id = "fed-chan-2";
    let message = format!("{}{}", channel_id, pseudonym_id);
    let signature = signing_key.sign(message.as_bytes());
    let signature_hex = hex::encode(signature.to_bytes());

    // 5. Call JOIN
    let payload = JoinFederatedChannelRequest {
        originating_server: "http://remote-server.com".to_string(),
        pseudonym_id: pseudonym_id.to_string(),
        signature: signature_hex,
    };

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri(&format!("/api/federation/channels/{}/join", channel_id))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // 6. Verify Membership
    let conn = pool.get().unwrap();
    let is_member = annex_channels::is_member(&conn, channel_id, pseudonym_id).unwrap();
    assert!(is_member, "Remote user should be a member of the channel");
}

#[tokio::test]
async fn test_join_federated_channel_invalid_signature() {
    let (app, pool) = setup_app_with_db().await;
    let conn = pool.get().unwrap();

    // Setup Remote Instance
    let signing_key = generate_signing_key();
    let verifying_key = signing_key.verifying_key();
    let public_key_hex = hex::encode(verifying_key.as_bytes());

    conn.execute(
        "INSERT INTO instances (base_url, public_key, label, status) VALUES (?1, ?2, 'Remote Server', 'ACTIVE')",
        rusqlite::params!["http://remote-server.com", public_key_hex],
    ).unwrap();

    drop(conn);

    // Sign wrong message
    let payload = JoinFederatedChannelRequest {
        originating_server: "http://remote-server.com".to_string(),
        pseudonym_id: "user".to_string(),
        signature: hex::encode(signing_key.sign(b"wrong").to_bytes()),
    };

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/federation/channels/chan/join")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR); // or 500 mapped from InvalidSignature
}
