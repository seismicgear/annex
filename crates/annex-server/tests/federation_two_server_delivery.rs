//! Two real servers, one real HTTP hop.
//!
//! Every other federation test in this suite is one-sided: it hand-builds a
//! signed envelope and POSTs it into the receiving handler. That covers the
//! receive half well and the send half not at all — nothing exercised an
//! outbox row actually leaving server A and arriving at server B, which is the
//! boundary the whole feature is.
//!
//! It could not be exercised, either. `is_url_private_or_reserved` was applied
//! unconditionally to peer `base_url`s at both enqueue and dequeue, so a peer
//! on `127.0.0.1` — the only kind a test can start — had every row dropped.
//! The same rule made a LAN pair, a Docker Compose pair addressing each other
//! by service name, and a VPN pair (Tailscale's 100.64/10 is explicitly
//! rejected) all silently undeliverable in production.
//!
//! `federation.allow_private_peer_addresses` relaxes the private-address half
//! of that check for peers only. These tests pin both sides of it: with the
//! flag on the message crosses, and with it off the row is still refused.

use annex_db::{create_pool, DbPool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};

fn dummy_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    Arc::new(annex_identity::zk::generate_dummy_vkey())
}

const CHANNEL_ID: &str = "chan-fed";
const SENDER: &str = "user-a-pseudo";
const TOPIC: &str = "annex:server:v1";
const COMMITMENT: &str = "0000000000000000000000000000000000000000000000000000000000000001";

/// Build an `AppState` over a fresh in-memory database with one server row.
fn build_state(
    slug: &str,
    signing_key: SigningKey,
    public_url: &str,
    allow_private_peers: bool,
) -> (Arc<AppState>, DbPool, i64) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    let policy = ServerPolicy::default();
    let policy_json = serde_json::to_string(&policy).unwrap();
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES (?1, ?1, ?2)",
        rusqlite::params![slug, policy_json],
    )
    .unwrap();
    let server_id = conn.last_insert_rowid();
    drop(conn);

    let federation_config = annex_server::config::FederationConfig {
        allow_private_peer_addresses: allow_private_peers,
        ..Default::default()
    };

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(MerkleTree::new(20).unwrap())),
        membership_vkey: dummy_vkey(),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id,
        signing_key: Arc::new(signing_key),
        public_url: Arc::new(RwLock::new(public_url.to_string())),
        policy: Arc::new(RwLock::new(policy)),
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
        federation_config,
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
    };
    (Arc::new(state), pool, server_id)
}

/// Register `peer_url`/`peer_pubkey` as an ACTIVE peer with an active
/// agreement, and return the `instances.id`.
fn add_peer(
    pool: &DbPool,
    server_id: i64,
    peer_url: &str,
    peer_pubkey_hex: &str,
    transfer_scope: &str,
) -> i64 {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO instances (base_url, public_key, label, status) \
         VALUES (?1, ?2, 'Peer', 'ACTIVE')",
        rusqlite::params![peer_url, peer_pubkey_hex],
    )
    .unwrap();
    let instance_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO federation_agreements ( \
            local_server_id, remote_instance_id, alignment_status, transfer_scope, \
            agreement_json, active \
         ) VALUES (?1, ?2, 'ALIGNED', ?3, '{}', 1)",
        rusqlite::params![server_id, instance_id, transfer_scope],
    )
    .unwrap();
    instance_id
}

/// A federated channel the sender belongs to.
fn add_federated_channel(pool: &DbPool, server_id: i64) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO channels (server_id, channel_id, name, channel_type, federation_scope, created_at) \
         VALUES (?1, ?2, 'Federated Chat', '\"Text\"', '\"Federated\"', datetime('now'))",
        rusqlite::params![server_id, CHANNEL_ID],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) \
         VALUES (?1, ?2, 'HUMAN', 1)",
        rusqlite::params![server_id, SENDER],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO channel_members (server_id, channel_id, pseudonym_id, role, joined_at) \
         VALUES (?1, ?2, ?3, 'MEMBER', datetime('now'))",
        rusqlite::params![server_id, CHANNEL_ID, SENDER],
    )
    .unwrap();
}

/// The receiver needs a federated identity for the sender so the envelope's
/// attestation ref resolves.
fn add_federated_identity(pool: &DbPool, server_id: i64, remote_instance_id: i64) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO federated_identities \
         (server_id, remote_instance_id, commitment_hex, pseudonym_id, vrp_topic, attested_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
        rusqlite::params![server_id, remote_instance_id, COMMITMENT, SENDER, TOPIC],
    )
    .unwrap();
}

/// The sender's ZK nullifier row, which is what `relay_message` reads to build
/// the envelope's attestation ref. Without it the ref is
/// `annex:server:v1:unknown` and the receiver answers 403.
fn add_sender_nullifier(pool: &DbPool) {
    pool.get()
        .unwrap()
        .execute(
            "INSERT INTO zk_nullifiers (nullifier_hex, topic, pseudonym_id, commitment_hex) \
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["nullifier-a", TOPIC, SENDER, COMMITMENT],
        )
        .unwrap();
}

fn a_message(id: &str, content: &str) -> annex_channels::Message {
    annex_channels::Message {
        id: 0,
        server_id: 0,
        channel_id: CHANNEL_ID.to_string(),
        message_id: id.to_string(),
        sender_pseudonym: SENDER.to_string(),
        content: content.to_string(),
        reply_to_message_id: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        expires_at: None,
        edited_at: None,
        deleted_at: None,
    }
}

fn count_messages(pool: &DbPool, message_id: &str) -> i64 {
    pool.get()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE message_id = ?1",
            rusqlite::params![message_id],
            |r| r.get(0),
        )
        .unwrap()
}

fn outbox_row(pool: &DbPool, message_id: &str) -> Option<(String, i64, Option<String>)> {
    pool.get()
        .unwrap()
        .query_row(
            "SELECT status, attempts, last_error FROM federation_outbox WHERE message_id = ?1",
            rusqlite::params![message_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
}

/// Start `app` on an OS-assigned loopback port and return its base URL.
async fn serve(router: axum::Router) -> (String, SocketAddr) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (format!("http://{addr}"), addr)
}

/// The whole loop: A relays a message, the outbox worker posts it to B over
/// real HTTP, and B stores it.
#[tokio::test]
async fn a_message_relayed_on_server_a_arrives_in_server_bs_database() {
    let mut csprng = OsRng;
    let key_a = SigningKey::generate(&mut csprng);
    let key_b = SigningKey::generate(&mut csprng);
    let pub_a = hex::encode(key_a.verifying_key().as_bytes());
    let pub_b = hex::encode(key_b.verifying_key().as_bytes());

    // --- Server B: the receiver, listening on a real port ---
    let (state_b, pool_b, server_b_id) = build_state("b", key_b, "http://placeholder", true);
    let (url_b, _) = serve(app((*state_b).clone())).await;
    add_federated_channel(&pool_b, server_b_id);

    // --- Server A: the sender ---
    let (state_a, pool_a, server_a_id) = build_state("a", key_a, "http://placeholder", true);
    let (url_a, _) = serve(app((*state_a).clone())).await;
    *state_a.public_url.write().unwrap() = url_a.clone();
    *state_b.public_url.write().unwrap() = url_b.clone();

    add_federated_channel(&pool_a, server_a_id);
    // A knows B as a peer, and relays full message bodies to it.
    add_peer(&pool_a, server_a_id, &url_b, &pub_b, "FULL_TRANSFER");
    // B knows A as a peer, and has an attested identity for A's sender.
    let a_on_b = add_peer(&pool_b, server_b_id, &url_a, &pub_a, "FULL_TRANSFER");
    add_federated_identity(&pool_b, server_b_id, a_on_b);
    // A resolves its own sender's commitment from `zk_nullifiers`.
    add_sender_nullifier(&pool_a);

    let msg = a_message("msg-crosses-the-wire", "hello from A");
    annex_server::services::federation_service::relay_message(
        state_a.clone(),
        CHANNEL_ID.to_string(),
        msg,
    )
    .await;

    // The row is enqueued...
    let (status, _, _) = outbox_row(&pool_a, "msg-crosses-the-wire")
        .expect("relay_message should enqueue an outbox row for the peer");
    assert_eq!(status, "pending");

    // ...and the worker delivers it.
    annex_server::background::drain_outbox_batch(state_a.clone(), 32)
        .await
        .expect("outbox drain should not error");

    assert_eq!(
        count_messages(&pool_b, "msg-crosses-the-wire"),
        1,
        "server B should have stored the message A relayed to it; \
         outbox row was {:?}",
        outbox_row(&pool_a, "msg-crosses-the-wire")
    );

    let (status, _, err) = outbox_row(&pool_a, "msg-crosses-the-wire").unwrap();
    assert_eq!(
        status, "delivered",
        "outbox row should settle; last_error={err:?}"
    );
}

/// The default is unchanged: a loopback peer is still refused at enqueue.
#[tokio::test]
async fn a_private_peer_is_still_refused_when_the_flag_is_off() {
    let mut csprng = OsRng;
    let key_a = SigningKey::generate(&mut csprng);
    let key_b = SigningKey::generate(&mut csprng);
    let pub_b = hex::encode(key_b.verifying_key().as_bytes());

    let (state_b, pool_b, server_b_id) = build_state("b", key_b, "http://placeholder", false);
    let (url_b, _) = serve(app((*state_b).clone())).await;
    add_federated_channel(&pool_b, server_b_id);

    // allow_private_peer_addresses defaults to false — the shipped behaviour.
    let (state_a, pool_a, server_a_id) = build_state("a", key_a, "http://placeholder", false);
    assert!(
        !state_a.federation_config.allow_private_peer_addresses,
        "the relaxation must be opt-in"
    );
    add_federated_channel(&pool_a, server_a_id);
    add_peer(&pool_a, server_a_id, &url_b, &pub_b, "FULL_TRANSFER");

    annex_server::services::federation_service::relay_message(
        state_a.clone(),
        CHANNEL_ID.to_string(),
        a_message("msg-refused", "should not leave"),
    )
    .await;

    assert!(
        outbox_row(&pool_a, "msg-refused").is_none(),
        "a loopback peer must still be filtered at enqueue when the flag is off"
    );
    assert_eq!(count_messages(&pool_b, "msg-refused"), 0);
}

/// The relaxation is scoped to the private-address rule. A non-http(s) peer
/// URL is refused whether or not the flag is set.
#[tokio::test]
async fn the_flag_does_not_admit_a_non_http_peer_url() {
    let mut csprng = OsRng;
    let key_a = SigningKey::generate(&mut csprng);
    let (state_a, pool_a, server_a_id) = build_state("a", key_a, "http://127.0.0.1:1/", true);
    add_federated_channel(&pool_a, server_a_id);
    add_peer(
        &pool_a,
        server_a_id,
        "file:///etc/passwd",
        "aa",
        "FULL_TRANSFER",
    );

    annex_server::services::federation_service::relay_message(
        state_a.clone(),
        CHANNEL_ID.to_string(),
        a_message("msg-file-scheme", "nope"),
    )
    .await;

    assert!(
        outbox_row(&pool_a, "msg-file-scheme").is_none(),
        "file:// is not a federation peer under any configuration"
    );
}
