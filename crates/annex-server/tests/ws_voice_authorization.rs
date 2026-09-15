//! The WebSocket signalling path must enforce the same voice rules as the
//! HTTP join, because it opens the same thing.
//!
//! `POST /api/channels/:id/voice/join` checks the server's `voice_enabled`
//! policy, whether the voice service is configured at all, whether the channel
//! is a Voice or Hybrid channel, and the identity's `can_voice` capability —
//! and then mints a join grant. The `webrtc_offer` WebSocket frame checked
//! channel membership and called `handle_sdp_offer` directly. So a client that
//! skipped the HTTP call got a live SFU peer connection with none of those
//! applied: an operator who set `voice_enabled = false` got a 403 on the HTTP
//! route and no effect whatsoever on the frame, and a member of a Text channel
//! could open a peer connection in it.
//!
//! `can_voice` is the sharpest of the four. It defaults to 1 for every member,
//! exists so that `PATCH /api/admin/members/{id}/capabilities` can revoke voice
//! from one person, and was consulted at voice-join time on NEITHER path — only
//! as a channel-level `required_capabilities` entry. Clearing it reported
//! success and changed nothing.
//!
//! These drive a real WebSocket against a real server. The refusals are
//! asserted through the wire, not through the helper, because the defect was
//! precisely that the helper was never reached.

use annex_channels::{create_channel, CreateChannelParams};
use annex_db::run_migrations;
use annex_identity::MerkleTree;
use annex_server::middleware::RateLimiter;
use annex_server::{api_ws, app, AppState};
use annex_types::{AlignmentStatus, ChannelType, FederationScope, ServerPolicy};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

const SECRET: [u8; 32] = [7u8; 32];

/// A minimal SDP offer. Never reaches the voice service in these tests — every
/// one of them asserts a refusal that happens before `handle_sdp_offer`.
const OFFER_SDP: &str = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n\
                         m=audio 9 UDP/TLS/RTP/SAVPF 111\r\nc=IN IP4 0.0.0.0\r\n\
                         a=rtpmap:111 opus/48000/2\r\na=mid:0\r\na=sendrecv\r\n";

struct Harness {
    addr: SocketAddr,
    pool: annex_db::DbPool,
    policy: Arc<RwLock<ServerPolicy>>,
}

/// A server with one member of one Voice channel, and voice configured.
///
/// Every knob this file exercises starts in the PERMISSIVE position, so each
/// test turns exactly one thing off and the refusal it sees can only be that
/// thing.
async fn harness(voice_url: &str) -> Harness {
    let db_file = tempfile::NamedTempFile::new().unwrap();
    let db_path = db_file.path().to_str().unwrap().to_string();
    std::mem::forget(db_file);

    let pool = annex_db::create_pool(&db_path, annex_db::DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        let policy_json = serde_json::to_string(&ServerPolicy::default()).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO platform_identities \
             (server_id, pseudonym_id, participant_type, active, can_voice) \
             VALUES (1, 'user-1', 'HUMAN', 1, 1)",
            [],
        )
        .unwrap();

        for (id, kind) in [
            ("voice-1", ChannelType::Voice),
            ("text-1", ChannelType::Text),
        ] {
            create_channel(
                &conn,
                &CreateChannelParams {
                    server_id: 1,
                    channel_id: id.to_string(),
                    name: id.to_string(),
                    channel_type: kind,
                    topic: None,
                    vrp_topic_binding: None,
                    required_capabilities_json: None,
                    agent_min_alignment: Some(AlignmentStatus::Aligned),
                    retention_days: None,
                    federation_scope: FederationScope::Local,
                },
            )
            .unwrap();
            conn.execute(
                "INSERT INTO channel_members (server_id, channel_id, pseudonym_id) \
                 VALUES (1, ?1, 'user-1')",
                [id],
            )
            .unwrap();
        }
    }

    let tree = {
        let conn = pool.get().unwrap();
        MerkleTree::restore(&conn, 20).unwrap()
    };
    let policy = Arc::new(RwLock::new(ServerPolicy::default()));

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(annex_identity::zk::generate_dummy_vkey()),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: Arc::new(RwLock::new("http://localhost:3000".to_string())),
        policy: policy.clone(),
        rate_limiter: RateLimiter::new(),
        connection_manager: api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(annex_voice::WebRtcConfig {
            url: voice_url.to_string(),
            ..Default::default()
        })),
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
        voice_token_secret: Arc::new(SECRET),
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
        shutdown: Default::default(),
        metrics: Default::default(),
    };

    let app = app(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    Harness { addr, pool, policy }
}

/// Send one `webrtc_offer` frame and return the server's reply text.
///
/// Returns `None` if the server said nothing within the timeout, which is how
/// "the offer was accepted and is being negotiated" would look — an outcome
/// every test here treats as a failure.
async fn offer(addr: SocketAddr, channel: &str, token: Option<&str>) -> Option<String> {
    let (mut ws, _) = connect_async(format!("ws://{addr}/ws?pseudonym=user-1"))
        .await
        .expect("connect");
    let mut frame = json!({
        "type": "webrtc_offer",
        "channelId": channel,
        "sdp": OFFER_SDP,
    });
    if let Some(t) = token {
        frame["voiceToken"] = json!(t);
    }
    ws.send(Message::Text(frame.to_string().into()))
        .await
        .unwrap();

    let deadline = tokio::time::Duration::from_secs(5);
    while let Ok(Some(Ok(msg))) = tokio::time::timeout(deadline, ws.next()).await {
        if let Message::Text(text) = msg {
            // Presence and subscription chatter arrives on this socket too.
            if text.contains("\"error\"") || text.contains("webrtc_answer") {
                return Some(text.to_string());
            }
        }
    }
    None
}

fn a_valid_grant(channel: &str) -> String {
    annex_voice::generate_join_token(channel, "user-1", &SECRET, 300).expect("token generation")
}

/// The defect, stated directly: no grant, no call.
#[tokio::test]
async fn an_offer_without_a_join_grant_is_refused() {
    let h = harness("http://127.0.0.1:7880").await;
    let reply = offer(h.addr, "voice-1", None).await.expect("a reply");
    assert!(
        reply.contains("join grant"),
        "an offer with no grant must be refused and say why, got: {reply}"
    );
    drop(h.pool);
}

/// A grant is bound to one room and one person. Neither binding may be
/// optional, or the grant is just a bearer token for the whole server.
#[tokio::test]
async fn a_grant_for_another_channel_is_refused() {
    let h = harness("http://127.0.0.1:7880").await;
    let wrong = a_valid_grant("text-1");
    let reply = offer(h.addr, "voice-1", Some(&wrong))
        .await
        .expect("a reply");
    assert!(
        reply.contains("not valid for this channel"),
        "a grant minted for a different channel must not admit, got: {reply}"
    );
    drop(h.pool);
}

#[tokio::test]
async fn a_forged_grant_is_refused() {
    let h = harness("http://127.0.0.1:7880").await;
    let forged = annex_voice::generate_join_token("voice-1", "user-1", &[9u8; 32], 300)
        .expect("token generation");
    let reply = offer(h.addr, "voice-1", Some(&forged))
        .await
        .expect("a reply");
    assert!(
        reply.contains("not valid for this channel"),
        "a grant signed with another key must not admit, got: {reply}"
    );
    drop(h.pool);
}

/// The operator's kill switch has to reach this path. It did not.
#[tokio::test]
async fn the_server_voice_policy_switch_reaches_the_signalling_path() {
    let h = harness("http://127.0.0.1:7880").await;
    h.policy.write().unwrap().voice_enabled = false;

    let reply = offer(h.addr, "voice-1", Some(&a_valid_grant("voice-1")))
        .await
        .expect("a reply");
    assert!(
        reply.to_lowercase().contains("voice"),
        "with voice_enabled = false the offer must be refused, got: {reply}"
    );
    assert!(
        !reply.contains("webrtc_answer"),
        "the server answered an offer it had been configured to refuse: {reply}"
    );
    drop(h.pool);
}

/// Revoking one person's voice must actually revoke it.
#[tokio::test]
async fn a_member_without_can_voice_is_refused() {
    let h = harness("http://127.0.0.1:7880").await;
    {
        let conn = h.pool.get().unwrap();
        conn.execute(
            "UPDATE platform_identities SET can_voice = 0 WHERE pseudonym_id = 'user-1'",
            [],
        )
        .unwrap();
    }

    let reply = offer(h.addr, "voice-1", Some(&a_valid_grant("voice-1")))
        .await
        .expect("a reply");
    assert!(
        reply.contains("voice is not enabled for this identity"),
        "clearing can_voice must stop this identity opening a peer connection, got: {reply}"
    );
    drop(h.pool);
}

/// A Text channel is not a call.
#[tokio::test]
async fn a_text_channel_cannot_host_a_peer_connection() {
    let h = harness("http://127.0.0.1:7880").await;
    let reply = offer(h.addr, "text-1", Some(&a_valid_grant("text-1")))
        .await
        .expect("a reply");
    assert!(
        reply.contains("does not support voice"),
        "a Text channel must refuse an offer, got: {reply}"
    );
    drop(h.pool);
}

/// With voice unconfigured the refusal must name that, rather than the
/// generic membership message a caller would go chasing.
#[tokio::test]
async fn an_unconfigured_voice_service_refuses_rather_than_half_working() {
    let h = harness("").await;
    let reply = offer(h.addr, "voice-1", Some(&a_valid_grant("voice-1")))
        .await
        .expect("a reply");
    assert!(
        reply.to_lowercase().contains("not configured")
            || reply.to_lowercase().contains("unavailable"),
        "an unconfigured voice service must say so, got: {reply}"
    );
    drop(h.pool);
}
