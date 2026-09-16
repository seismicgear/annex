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
        shutdown: Default::default(),
        metrics: Default::default(),
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

// ── Outage and recovery ───────────────────────────────────────────────────
//
// A live handshake proves two servers can talk. It does not prove what happens
// when one of them stops answering, which is the ordinary condition of a
// federated network: the peer is someone else's self-hosted box, and it will be
// down for upgrades, reboots and bad afternoons.
//
// The configuration distinguishes live delivery from catch-up (a five-minute
// freshness window, three delivery attempts), so the questions worth asking are
// whether queued work survives the outage, whether it is delivered on return,
// and whether the return delivers it TWICE.

/// A peer that refuses every connection: nothing is listening on the port.
///
/// Binding and immediately dropping the listener gives a port that is
/// realistically dead — connection refused — rather than one that hangs, which
/// is a different failure with a different timeout.
async fn a_dead_peer_url() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}")
}

#[tokio::test]
async fn a_message_queued_while_a_peer_is_down_is_delivered_when_it_returns() {
    let mut csprng = OsRng;
    let key_a = SigningKey::generate(&mut csprng);
    let key_b = SigningKey::generate(&mut csprng);
    let pub_a = hex::encode(key_a.verifying_key().as_bytes());
    let pub_b = hex::encode(key_b.verifying_key().as_bytes());

    // B's future address, claimed now and not served until later — the same
    // shape as a peer that is configured and currently rebooting.
    let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_b = listener_b.local_addr().unwrap();
    let url_b = format!("http://{addr_b}");
    drop(listener_b);

    let (state_a, pool_a, server_a_id) = build_state("a", key_a, "http://placeholder", true);
    let (url_a, _) = serve(app((*state_a).clone())).await;
    *state_a.public_url.write().unwrap() = url_a.clone();
    add_federated_channel(&pool_a, server_a_id);
    add_peer(&pool_a, server_a_id, &url_b, &pub_b, "FULL_TRANSFER");
    add_sender_nullifier(&pool_a);

    let msg = a_message("msg-during-outage", "sent while B was down");
    annex_server::services::federation_service::relay_message(
        state_a.clone(),
        CHANNEL_ID.to_string(),
        msg,
    )
    .await;

    // Drain against a peer that is not listening.
    annex_server::background::drain_outbox_batch(state_a.clone(), 32)
        .await
        .expect("a dead peer must not error the whole drain");

    let (status, attempts, err) =
        outbox_row(&pool_a, "msg-during-outage").expect("the row must still exist");
    assert_ne!(
        status, "delivered",
        "a message cannot be delivered to a peer that is not listening"
    );
    assert!(
        attempts >= 1,
        "the attempt should have been counted, so the retry budget is real"
    );
    assert!(
        err.is_some(),
        "the failure should be recorded on the row, not only in a log"
    );
    assert_eq!(
        status, "pending",
        "the row must stay pending and retryable rather than being dropped or          failed on the first refusal; status was {status:?}"
    );

    // --- B comes back, with the SAME identity and the same address ---
    let (state_b, pool_b, server_b_id) = build_state("b", key_b, "http://placeholder", true);
    let listener_b = tokio::net::TcpListener::bind(addr_b).await.expect(
        "B should be able to reclaim its address; if this fails the test is racing          something else on the port",
    );
    let app_b = app((*state_b).clone());
    tokio::spawn(async move {
        let _ = axum::serve(
            listener_b,
            app_b.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    *state_b.public_url.write().unwrap() = url_b.clone();
    add_federated_channel(&pool_b, server_b_id);
    let a_on_b = add_peer(&pool_b, server_b_id, &url_a, &pub_a, "FULL_TRANSFER");
    add_federated_identity(&pool_b, server_b_id, a_on_b);

    // Let the backoff come due.
    //
    // `drain_outbox_batch` selects `WHERE status = 'pending' AND next_retry_at
    // <= datetime('now')`, and a failed attempt pushes `next_retry_at` forward
    // exponentially. Draining immediately after recovery therefore returns
    // nothing — which is correct behaviour, not a delivery failure, and the
    // first version of this test read it as one. Sleeping for the real backoff
    // would make the test slow and flaky; moving the clock on the row is the
    // same event without the wait.
    {
        let conn = pool_a.get().unwrap();
        conn.execute(
            "UPDATE federation_outbox SET next_retry_at = datetime('now', '-1 minute')
              WHERE message_id = 'msg-during-outage'",
            [],
        )
        .unwrap();
    }

    annex_server::background::drain_outbox_batch(state_a.clone(), 32)
        .await
        .expect("drain after recovery should not error");

    assert_eq!(
        count_messages(&pool_b, "msg-during-outage"),
        1,
        "the message queued during the outage should arrive once B is back;          outbox row is {:?}",
        outbox_row(&pool_a, "msg-during-outage")
    );

    let (status, _, err) = outbox_row(&pool_a, "msg-during-outage").unwrap();
    assert_eq!(status, "delivered", "last_error={err:?}");
}

/// Recovery must not duplicate. A retried delivery — the outbox's whole
/// purpose — reaches an endpoint that may already have stored the message, and
/// `federation_receipts` carries `UNIQUE (remote_instance_id, message_id)` for
/// exactly this. Asserted at the receiver rather than by inspecting the sender:
/// what matters is how many copies a member would see.
#[tokio::test]
async fn re_delivering_the_same_message_does_not_duplicate_it() {
    let mut csprng = OsRng;
    let key_a = SigningKey::generate(&mut csprng);
    let key_b = SigningKey::generate(&mut csprng);
    let pub_a = hex::encode(key_a.verifying_key().as_bytes());
    let pub_b = hex::encode(key_b.verifying_key().as_bytes());

    let (state_b, pool_b, server_b_id) = build_state("b", key_b, "http://placeholder", true);
    let (url_b, _) = serve(app((*state_b).clone())).await;
    add_federated_channel(&pool_b, server_b_id);

    let (state_a, pool_a, server_a_id) = build_state("a", key_a, "http://placeholder", true);
    let (url_a, _) = serve(app((*state_a).clone())).await;
    *state_a.public_url.write().unwrap() = url_a.clone();
    *state_b.public_url.write().unwrap() = url_b.clone();

    add_federated_channel(&pool_a, server_a_id);
    add_peer(&pool_a, server_a_id, &url_b, &pub_b, "FULL_TRANSFER");
    let a_on_b = add_peer(&pool_b, server_b_id, &url_a, &pub_a, "FULL_TRANSFER");
    add_federated_identity(&pool_b, server_b_id, a_on_b);
    add_sender_nullifier(&pool_a);

    let msg = a_message("msg-delivered-twice", "exactly once, please");
    annex_server::services::federation_service::relay_message(
        state_a.clone(),
        CHANNEL_ID.to_string(),
        msg.clone(),
    )
    .await;
    annex_server::background::drain_outbox_batch(state_a.clone(), 32)
        .await
        .unwrap();
    assert_eq!(count_messages(&pool_b, "msg-delivered-twice"), 1);

    // Re-enqueue the SAME message id and deliver again — a duplicate arriving
    // from a retry that the sender believed had failed.
    {
        let conn = pool_a.get().unwrap();
        conn.execute(
            "UPDATE federation_outbox SET status = 'pending', attempts = 0, last_error = NULL,
                    next_retry_at = datetime('now', '-1 minute')
              WHERE message_id = 'msg-delivered-twice'",
            [],
        )
        .unwrap();
    }
    annex_server::background::drain_outbox_batch(state_a.clone(), 32)
        .await
        .unwrap();

    assert_eq!(
        count_messages(&pool_b, "msg-delivered-twice"),
        1,
        "a re-delivered message was stored twice — a member would see the \
         message duplicated every time a retry raced a slow acknowledgement"
    );
}

/// One unreachable peer must not stop delivery to a healthy one. Without this,
/// a single self-hosted box that is down takes the whole federation with it.
#[tokio::test]
async fn an_unreachable_peer_does_not_block_a_healthy_one() {
    let mut csprng = OsRng;
    let key_a = SigningKey::generate(&mut csprng);
    let key_b = SigningKey::generate(&mut csprng);
    let key_dead = SigningKey::generate(&mut csprng);
    let pub_a = hex::encode(key_a.verifying_key().as_bytes());
    let pub_b = hex::encode(key_b.verifying_key().as_bytes());
    let pub_dead = hex::encode(key_dead.verifying_key().as_bytes());

    let (state_b, pool_b, server_b_id) = build_state("b", key_b, "http://placeholder", true);
    let (url_b, _) = serve(app((*state_b).clone())).await;
    add_federated_channel(&pool_b, server_b_id);

    let (state_a, pool_a, server_a_id) = build_state("a", key_a, "http://placeholder", true);
    let (url_a, _) = serve(app((*state_a).clone())).await;
    *state_a.public_url.write().unwrap() = url_a.clone();
    *state_b.public_url.write().unwrap() = url_b.clone();

    add_federated_channel(&pool_a, server_a_id);
    // The dead peer is added FIRST, so it is drained first and a naive
    // implementation would stop there.
    add_peer(
        &pool_a,
        server_a_id,
        &a_dead_peer_url().await,
        &pub_dead,
        "FULL_TRANSFER",
    );
    add_peer(&pool_a, server_a_id, &url_b, &pub_b, "FULL_TRANSFER");
    let a_on_b = add_peer(&pool_b, server_b_id, &url_a, &pub_a, "FULL_TRANSFER");
    add_federated_identity(&pool_b, server_b_id, a_on_b);
    add_sender_nullifier(&pool_a);

    let msg = a_message("msg-past-a-dead-peer", "B should still get this");
    annex_server::services::federation_service::relay_message(
        state_a.clone(),
        CHANNEL_ID.to_string(),
        msg,
    )
    .await;

    annex_server::background::drain_outbox_batch(state_a.clone(), 32)
        .await
        .expect("one dead peer must not error the drain");

    assert_eq!(
        count_messages(&pool_b, "msg-past-a-dead-peer"),
        1,
        "the healthy peer did not receive the message; one unreachable peer \
         should not take the federation with it"
    );
}
