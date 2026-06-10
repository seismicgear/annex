//! Integration tests for the federated redaction tombstone protocol
//! (ADR-0011): `POST /api/federation/redactions`.
//!
//! Covers the verification chain end-to-end over HTTP: signature
//! verification against the originating server's key, origin authority
//! (only the delivering peer may redact), redactor authority (sender or
//! moderation), the freshness gate, receipt-ledger idempotency, and the
//! actual tombstone effect (content blanked, audit fields kept).

use annex_db::{create_pool, DbPool, DbRuntimeSettings};
use annex_federation::FederatedRedactionEnvelope;
use annex_identity::MerkleTree;
use annex_server::services::federation_service::redaction_signing_input;
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

fn load_dummy_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    Arc::new(annex_identity::zk::generate_dummy_vkey())
}

const REMOTE_ORIGIN: &str = "http://remote-server.com";
const CHANNEL_ID: &str = "chan-fed";
const MESSAGE_ID: &str = "msg-remote-123";
const SENDER: &str = "user-local-pseudo";

struct Harness {
    app: axum::Router,
    pool: DbPool,
    remote_key: SigningKey,
}

/// Seeds a local server federated with one remote instance, a federated
/// channel, and one message previously delivered from that instance
/// (message row + original delivery receipt).
fn build_harness(seed_original_receipt: bool) -> Harness {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    let policy = ServerPolicy::default();
    let policy_json = serde_json::to_string(&policy).unwrap();
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local Server', ?1)",
        rusqlite::params![policy_json],
    )
    .unwrap();
    let local_server_id = conn.last_insert_rowid();

    let mut csprng = OsRng;
    let remote_key = SigningKey::generate(&mut csprng);
    let remote_public_key_hex = hex::encode(remote_key.verifying_key().as_bytes());
    conn.execute(
        "INSERT INTO instances (base_url, public_key, label, status) VALUES (?1, ?2, 'Remote', 'ACTIVE')",
        rusqlite::params![REMOTE_ORIGIN, remote_public_key_hex],
    )
    .unwrap();
    let remote_instance_id = conn.last_insert_rowid();

    conn.execute(
        "INSERT INTO federation_agreements (
            local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active
        ) VALUES (?1, ?2, 'ALIGNED', 'REFLECTION_SUMMARIES_ONLY', '{}', 1)",
        rusqlite::params![local_server_id, remote_instance_id],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (?1, ?2, 'HUMAN', 1)",
        rusqlite::params![local_server_id, SENDER],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO channels (
            server_id, channel_id, name, channel_type, federation_scope, created_at
           ) VALUES (?1, ?2, 'Federated Chat', '\"Text\"', '\"Federated\"', datetime('now'))",
        rusqlite::params![local_server_id, CHANNEL_ID],
    )
    .unwrap();

    // The previously delivered federated message.
    conn.execute(
        "INSERT INTO messages (server_id, channel_id, message_id, sender_pseudonym, content)
         VALUES (?1, ?2, ?3, ?4, 'original federated content')",
        rusqlite::params![local_server_id, CHANNEL_ID, MESSAGE_ID, SENDER],
    )
    .unwrap();

    if seed_original_receipt {
        conn.execute(
            "INSERT INTO federation_message_receipts
             (remote_instance_id, message_id, envelope_hash, envelope_created_at, delivery_mode)
             VALUES (?1, ?2, 'orig-hash', datetime('now'), 'live')",
            rusqlite::params![remote_instance_id, MESSAGE_ID],
        )
        .unwrap();
    }
    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_dummy_vkey(),
        membership_vkey_v2: None,
        server_id: local_server_id,
        signing_key: Arc::new(SigningKey::generate(&mut csprng)),
        public_url: Arc::new(RwLock::new("http://localhost:3000".to_string())),
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
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
    };

    Harness {
        app: app(state),
        pool,
        remote_key,
    }
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Builds a redaction envelope signed with `key`.
fn signed_redaction(
    key: &SigningKey,
    redacted_by: &str,
    reason: &str,
    created_at: &str,
) -> FederatedRedactionEnvelope {
    let mut envelope = FederatedRedactionEnvelope {
        envelope_kind: "redaction".to_string(),
        envelope_version: "v1".to_string(),
        message_id: MESSAGE_ID.to_string(),
        channel_id: CHANNEL_ID.to_string(),
        originating_server: REMOTE_ORIGIN.to_string(),
        redacted_by: redacted_by.to_string(),
        redaction_reason: reason.to_string(),
        attestation_ref: "annex:server:v1:unknown".to_string(),
        signature: String::new(),
        created_at: created_at.to_string(),
    };
    let sig = key.sign(redaction_signing_input(&envelope).as_bytes());
    envelope.signature = hex::encode(sig.to_bytes());
    envelope
}

async fn post_redaction(
    app: axum::Router,
    envelope: &FederatedRedactionEnvelope,
) -> axum::response::Response {
    let mut request = Request::builder()
        .uri("/api/federation/redactions")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(envelope).unwrap()))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    app.oneshot(request).await.unwrap()
}

fn message_state(pool: &DbPool) -> (String, Option<String>) {
    let conn = pool.get().unwrap();
    conn.query_row(
        "SELECT content, deleted_at FROM messages WHERE message_id = ?1",
        rusqlite::params![MESSAGE_ID],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

fn redaction_receipt_count(pool: &DbPool) -> i64 {
    let conn = pool.get().unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM federation_message_receipts WHERE message_id = ?1",
        rusqlite::params![format!("redaction:{MESSAGE_ID}")],
        |row| row.get(0),
    )
    .unwrap()
}

#[tokio::test]
async fn valid_redaction_blanks_message_and_replay_is_idempotent() {
    let h = build_harness(true);
    let envelope = signed_redaction(&h.remote_key, SENDER, "deleted", &now_iso());

    let response = post_redaction(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["applied"], true);

    let (content, deleted_at) = message_state(&h.pool);
    assert_eq!(content, "", "content must be blanked");
    assert!(deleted_at.is_some(), "deleted_at must be set");
    assert_eq!(redaction_receipt_count(&h.pool), 1);

    // Replay of the identical envelope: 200, applied=false, no
    // duplicate receipt — outbox retries are safe.
    let response = post_redaction(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["applied"], false);
    assert_eq!(redaction_receipt_count(&h.pool), 1);
}

#[tokio::test]
async fn tampered_signature_is_rejected() {
    let h = build_harness(true);
    let mut envelope = signed_redaction(&h.remote_key, SENDER, "deleted", &now_iso());
    // Mutate a signed field after signing.
    envelope.redacted_by = "attacker".to_string();

    let response = post_redaction(h.app.clone(), &envelope).await;
    // InvalidSignature maps to 401, same as the message-envelope path.
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let (content, deleted_at) = message_state(&h.pool);
    assert_eq!(content, "original federated content");
    assert!(deleted_at.is_none());
}

#[tokio::test]
async fn redaction_without_original_receipt_is_rejected() {
    // No original delivery receipt: the message exists locally but was
    // NOT delivered by this peer (e.g. locally authored). Only the
    // delivering peer may redact.
    let h = build_harness(false);
    let envelope = signed_redaction(&h.remote_key, SENDER, "deleted", &now_iso());

    let response = post_redaction(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let (content, deleted_at) = message_state(&h.pool);
    assert_eq!(content, "original federated content");
    assert!(deleted_at.is_none());
}

#[tokio::test]
async fn non_sender_redactor_is_rejected_for_author_delete() {
    let h = build_harness(true);
    let envelope = signed_redaction(&h.remote_key, "someone-else", "deleted", &now_iso());

    let response = post_redaction(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let (content, _) = message_state(&h.pool);
    assert_eq!(content, "original federated content");
}

#[tokio::test]
async fn moderation_redaction_by_non_sender_is_accepted() {
    // The channel lives on the originating server; its moderators
    // govern it. A moderation redaction is accepted on the origin's
    // signature alone.
    let h = build_harness(true);
    let envelope = signed_redaction(&h.remote_key, "remote-moderator", "moderation", &now_iso());

    let response = post_redaction(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::OK);

    let (content, deleted_at) = message_state(&h.pool);
    assert_eq!(content, "");
    assert!(deleted_at.is_some());
}

#[tokio::test]
async fn stale_created_at_is_rejected() {
    let h = build_harness(true);
    let stale = (chrono::Utc::now() - chrono::Duration::hours(2))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let envelope = signed_redaction(&h.remote_key, SENDER, "deleted", &stale);

    let response = post_redaction(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let (content, _) = message_state(&h.pool);
    assert_eq!(content, "original federated content");
}

#[tokio::test]
async fn invalid_reason_is_rejected() {
    let h = build_harness(true);
    let envelope = signed_redaction(&h.remote_key, SENDER, "oops", &now_iso());

    let response = post_redaction(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let (content, _) = message_state(&h.pool);
    assert_eq!(content, "original federated content");
}

#[tokio::test]
async fn relay_redaction_enqueues_prefixed_outbox_rows() {
    use annex_server::services::federation_service::relay_redaction;

    let h = build_harness(true);

    // Rebuild a state handle over the same pool for the relay call.
    let tree = MerkleTree::new(20).unwrap();
    let state = Arc::new(AppState {
        pool: h.pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_dummy_vkey(),
        membership_vkey_v2: None,
        server_id: 1,
        signing_key: Arc::new(SigningKey::generate(&mut OsRng)),
        public_url: Arc::new(RwLock::new("http://local-origin.example".to_string())),
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
    });

    relay_redaction(
        state,
        CHANNEL_ID.to_string(),
        MESSAGE_ID.to_string(),
        SENDER.to_string(),
        "deleted",
    )
    .await;

    let conn = h.pool.get().unwrap();
    let (outbox_message_id, envelope_json): (String, String) = conn
        .query_row(
            "SELECT message_id, envelope_json FROM federation_outbox WHERE status = 'pending'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("one pending outbox row must exist for the active peer");

    assert_eq!(
        outbox_message_id,
        format!("redaction:{MESSAGE_ID}"),
        "outbox key must be namespaced so it can't collide with the original message row"
    );
    let parsed: FederatedRedactionEnvelope = serde_json::from_str(&envelope_json).unwrap();
    assert_eq!(parsed.envelope_kind, "redaction");
    assert_eq!(parsed.message_id, MESSAGE_ID);
    assert_eq!(parsed.redacted_by, SENDER);
    assert_eq!(parsed.redaction_reason, "deleted");
    assert!(!parsed.signature.is_empty());
}
