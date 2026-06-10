//! Integration tests for federated edit propagation:
//! `POST /api/federation/edits`.
//!
//! Mirrors the redaction tombstone suite (`api_federation_redaction.rs`)
//! with the edit-specific rules: per-event `edit:<edit_id>` ledger keys,
//! editor-must-be-sender authority, out-of-order edits never regressing
//! newer content, and tombstones never being resurrected by an edit.

use annex_db::{create_pool, DbPool, DbRuntimeSettings};
use annex_federation::FederatedEditEnvelope;
use annex_identity::MerkleTree;
use annex_server::services::federation_service::edit_signing_input;
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

fn iso_at(offset_seconds: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(offset_seconds))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn signed_edit(
    key: &SigningKey,
    edit_id: &str,
    edited_by: &str,
    content: &str,
    created_at: &str,
) -> FederatedEditEnvelope {
    let mut envelope = FederatedEditEnvelope {
        envelope_kind: "edit".to_string(),
        envelope_version: "v1".to_string(),
        edit_id: edit_id.to_string(),
        message_id: MESSAGE_ID.to_string(),
        channel_id: CHANNEL_ID.to_string(),
        content: content.to_string(),
        originating_server: REMOTE_ORIGIN.to_string(),
        edited_by: edited_by.to_string(),
        attestation_ref: "annex:server:v1:unknown".to_string(),
        signature: String::new(),
        created_at: created_at.to_string(),
    };
    let sig = key.sign(edit_signing_input(&envelope).as_bytes());
    envelope.signature = hex::encode(sig.to_bytes());
    envelope
}

async fn post_edit(
    app: axum::Router,
    envelope: &FederatedEditEnvelope,
) -> axum::response::Response {
    let mut request = Request::builder()
        .uri("/api/federation/edits")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(envelope).unwrap()))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    app.oneshot(request).await.unwrap()
}

async fn applied_flag(response: axum::response::Response) -> bool {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["applied"].as_bool().unwrap()
}

fn message_content(pool: &DbPool) -> (String, Option<String>) {
    let conn = pool.get().unwrap();
    conn.query_row(
        "SELECT content, edited_at FROM messages WHERE message_id = ?1",
        rusqlite::params![MESSAGE_ID],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

fn audit_rows(pool: &DbPool) -> i64 {
    let conn = pool.get().unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM message_edits WHERE message_id = ?1",
        rusqlite::params![MESSAGE_ID],
        |row| row.get(0),
    )
    .unwrap()
}

#[tokio::test]
async fn valid_edit_applies_and_replay_is_idempotent() {
    let h = build_harness(true);
    let envelope = signed_edit(
        &h.remote_key,
        "edit-1",
        SENDER,
        "edited content",
        &iso_at(0),
    );

    let response = post_edit(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(applied_flag(response).await);

    let (content, edited_at) = message_content(&h.pool);
    assert_eq!(content, "edited content");
    assert!(edited_at.is_some());
    assert_eq!(audit_rows(&h.pool), 1, "prior content must be audited");

    // Identical replay: 200, applied=false, no duplicate audit row.
    let response = post_edit(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!applied_flag(response).await);
    assert_eq!(audit_rows(&h.pool), 1);
}

#[tokio::test]
async fn tampered_signature_is_rejected() {
    let h = build_harness(true);
    let mut envelope = signed_edit(&h.remote_key, "edit-1", SENDER, "edited", &iso_at(0));
    envelope.content = "attacker content".to_string();

    let response = post_edit(h.app.clone(), &envelope).await;
    // InvalidSignature maps to 401, same as the message-envelope path.
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let (content, _) = message_content(&h.pool);
    assert_eq!(content, "original federated content");
}

#[tokio::test]
async fn edit_without_original_receipt_is_rejected() {
    let h = build_harness(false);
    let envelope = signed_edit(&h.remote_key, "edit-1", SENDER, "edited", &iso_at(0));

    let response = post_edit(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let (content, _) = message_content(&h.pool);
    assert_eq!(content, "original federated content");
}

#[tokio::test]
async fn non_sender_editor_is_rejected() {
    let h = build_harness(true);
    let envelope = signed_edit(
        &h.remote_key,
        "edit-1",
        "someone-else",
        "edited",
        &iso_at(0),
    );

    let response = post_edit(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let (content, _) = message_content(&h.pool);
    assert_eq!(content, "original federated content");
}

#[tokio::test]
async fn stale_created_at_is_rejected() {
    let h = build_harness(true);
    let envelope = signed_edit(&h.remote_key, "edit-1", SENDER, "edited", &iso_at(-7200));

    let response = post_edit(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn edit_never_resurrects_a_deleted_message() {
    let h = build_harness(true);
    {
        let conn = h.pool.get().unwrap();
        conn.execute(
            "UPDATE messages SET content = '', deleted_at = datetime('now') WHERE message_id = ?1",
            rusqlite::params![MESSAGE_ID],
        )
        .unwrap();
    }

    let envelope = signed_edit(&h.remote_key, "edit-1", SENDER, "resurrected!", &iso_at(0));
    let response = post_edit(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!applied_flag(response).await, "tombstone must win");

    let (content, _) = message_content(&h.pool);
    assert_eq!(content, "", "deleted content must stay blank");
}

#[tokio::test]
async fn out_of_order_older_edit_does_not_regress_newer_content() {
    let h = build_harness(true);

    // The newer edit arrives first…
    let newer = signed_edit(
        &h.remote_key,
        "edit-newer",
        SENDER,
        "newer content",
        &iso_at(-5),
    );
    let response = post_edit(h.app.clone(), &newer).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(applied_flag(response).await);

    // …then an older edit is delivered late (distinct edit event).
    let older = signed_edit(
        &h.remote_key,
        "edit-older",
        SENDER,
        "older content",
        &iso_at(-60),
    );
    let response = post_edit(h.app.clone(), &older).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        !applied_flag(response).await,
        "an out-of-order older edit must not be applied"
    );

    let (content, _) = message_content(&h.pool);
    assert_eq!(content, "newer content");
    assert_eq!(
        audit_rows(&h.pool),
        1,
        "skipped edit must not add audit rows"
    );
}

#[tokio::test]
async fn oversized_content_is_rejected() {
    let h = build_harness(true);
    let big = "x".repeat(65_537);
    let envelope = signed_edit(&h.remote_key, "edit-1", SENDER, &big, &iso_at(0));

    let response = post_edit(h.app.clone(), &envelope).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn relay_edit_enqueues_per_event_outbox_rows() {
    use annex_server::services::federation_service::relay_edit;

    let h = build_harness(true);

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

    // Two edits to the same message must produce two distinct outbox
    // rows (per-event keys), not collide on UNIQUE(peer, message_id).
    relay_edit(
        state.clone(),
        CHANNEL_ID.to_string(),
        MESSAGE_ID.to_string(),
        SENDER.to_string(),
        "first edit".to_string(),
    )
    .await;
    relay_edit(
        state,
        CHANNEL_ID.to_string(),
        MESSAGE_ID.to_string(),
        SENDER.to_string(),
        "second edit".to_string(),
    )
    .await;

    let conn = h.pool.get().unwrap();
    let mut stmt = conn
        .prepare("SELECT message_id, envelope_json FROM federation_outbox WHERE status = 'pending' ORDER BY id")
        .unwrap();
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(rows.len(), 2, "each edit event gets its own outbox row");
    assert!(rows[0].0.starts_with("edit:"));
    assert!(rows[1].0.starts_with("edit:"));
    assert_ne!(rows[0].0, rows[1].0, "edit events must have distinct keys");

    let parsed: FederatedEditEnvelope = serde_json::from_str(&rows[1].1).unwrap();
    assert_eq!(parsed.envelope_kind, "edit");
    assert_eq!(parsed.message_id, MESSAGE_ID);
    assert_eq!(parsed.content, "second edit");
    assert!(!parsed.signature.is_empty());
}
