use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::zk::{G1Affine, G2Affine, VerifyingKey};
use annex_server::{app, middleware, AppState};
use annex_types::ServerPolicy;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tokio::net::TcpListener;

#[tokio::test]
async fn test_sse_presence_stream() {
    // 1. Setup DB
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'default', 'Default', '{}')",
            [],
        )
        .unwrap();
    }

    // 2. Setup AppState
    let tree = annex_identity::MerkleTree::new(20).unwrap();
    let vk = VerifyingKey {
        alpha_g1: G1Affine::default(),
        beta_g2: G2Affine::default(),
        gamma_g2: G2Affine::default(),
        delta_g2: G2Affine::default(),
        gamma_abc_g1: vec![G1Affine::default()],
    };
    let (presence_tx, _) = tokio::sync::broadcast::channel(100);

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(vk),
        server_id: 1,
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: middleware::RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: presence_tx.clone(),
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::LiveKitConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
    };

    // 3. Start Server
    let app = app(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    // 4. Connect to SSE Stream
    let client = reqwest::Client::new();
    let mut response = client
        .get(format!("{}/events/presence", server_url))
        .send()
        .await
        .expect("Failed to connect to SSE stream");

    assert!(response.status().is_success());

    // Wait a bit for connection to be established
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 5. Trigger Event
    let event = annex_types::PresenceEvent::NodeUpdated {
        pseudonym_id: "test-node".to_string(),
        active: true,
    };
    presence_tx.send(event).unwrap();

    // 6. Receive Event
    // We expect "data: {...}\n\n"
    let chunk = response
        .chunk()
        .await
        .expect("Failed to read chunk")
        .expect("Stream closed");
    let chunk_str = String::from_utf8(chunk.to_vec()).unwrap();

    println!("Received chunk: {}", chunk_str);

    assert!(chunk_str.starts_with("data:"));
    assert!(chunk_str.contains("NodeUpdated"));
    assert!(chunk_str.contains("test-node"));
}
