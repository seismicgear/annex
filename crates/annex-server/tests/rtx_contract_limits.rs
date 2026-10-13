//! Actual handler/SQLite contract checks using controlled authentication fixtures.
//! These are component tests; live registry and federation proof run separately.

mod common;

use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

async fn setup_app() -> (axum::Router, annex_db::DbPool) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Insert server row
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'test-server', 'Test Server', '{}')",
        [],
    )
    .unwrap();

    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let policy = ServerPolicy {
        allow_full_knowledge: true,
        ..ServerPolicy::default()
    };

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: common::load_vkey_or_dummy(),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: std::sync::Arc::new(std::sync::RwLock::new(
            "http://localhost:3000".to_string(),
        )),
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
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
        voice_token_secret: std::sync::Arc::new([0u8; 32]),
        federation_transport: Default::default(),
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: std::sync::Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
        shutdown: Default::default(),
        metrics: Default::default(),
    };

    (app(state), pool)
}

async fn register_limited(
    app: &axum::Router,
    pool: &annex_db::DbPool,
    contract: annex_vrp::VrpCapabilitySharingContract,
) -> Value {
    let public = hex::encode(
        ed25519_dalek::SigningKey::from_bytes(&[42; 32])
            .verifying_key()
            .to_bytes(),
    );
    let handshake = annex_vrp::VrpFederationHandshake {
        anchor_snapshot: annex_vrp::VrpAnchorSnapshot::new(&[], &[]).unwrap(),
        capability_contract: contract,
        scorer: None,
    };
    let mut req=Request::builder().uri("/api/vrp/agent-handshake").method("POST").header("content-type","application/json")
        .body(Body::from(serde_json::json!({"pseudonymId":"limited-agent","handshake":handshake,"signingPubkey":public}).to_string())).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 13000))));
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    pool.get().unwrap().execute("INSERT INTO platform_identities(server_id,pseudonym_id,participant_type,active) VALUES(1,'limited-agent','AI_AGENT',1)",[]).unwrap();
    body
}

fn bundle(reference: &str) -> annex_rtx::ReflectionSummaryBundle {
    annex_rtx::ReflectionSummaryBundle {
        bundle_id: uuid::Uuid::new_v4().to_string(),
        source_pseudonym: "limited-agent".into(),
        source_server: "http://localhost:3000".into(),
        domain_tags: vec!["rust".into()],
        summary: "An authenticated reflection.".into(),
        reasoning_chain: Some("A short private rationale.".into()),
        caveats: vec![],
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
        signature: String::new(),
        vrp_handshake_ref: reference.into(),
    }
}
async fn publish(
    app: &axum::Router,
    mut bundle: annex_rtx::ReflectionSummaryBundle,
) -> (StatusCode, Value) {
    bundle.signature = annex_server::services::rtx_service::sign_bundle_author(&bundle, &[42; 32]);
    let mut req = Request::builder()
        .uri("/api/rtx/publish")
        .method("POST")
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", "limited-agent")
        .body(Body::from(serde_json::to_string(&bundle).unwrap()))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 13000))));
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let body = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    (status, body)
}

#[tokio::test]
async fn actual_signed_publish_obeys_domains_size_and_absolute_retention() {
    let (app, pool) = setup_app().await;
    let mut contract =
        annex_vrp::VrpCapabilitySharingContract::from_policy(&ServerPolicy::default());
    contract.knowledge_domains_allowed = Some(vec!["rust".into()]);
    contract.max_exchange_size = Some(1500);
    contract.retention_policy = Some(annex_vrp::VrpRetentionPolicy::MaxAge { seconds: 60 });
    let report = register_limited(&app, &pool, contract).await;
    let reference = report["handshakeRef"].as_str().unwrap();
    let accepted = bundle(reference);
    let id = accepted.bundle_id.clone();
    let created = accepted.created_at as i64;
    assert_eq!(publish(&app, accepted).await.0, StatusCode::OK);
    let stored: (i64, String) = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT expires_at_ms,contract_json FROM rtx_bundles WHERE bundle_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(stored.0, created + 60_000);
    let contract: annex_vrp::NegotiatedRtxContract = serde_json::from_str(&stored.1).unwrap();
    assert_eq!(contract.max_exchange_size, 1500);
    assert_eq!(contract.retention_seconds, Some(60));
    for kind in ["mixed-domain", "untagged", "oversized", "expired", "future"] {
        let mut rejected = bundle(reference);
        match kind {
            "mixed-domain" => rejected.domain_tags.push("private".into()),
            "untagged" => rejected.domain_tags.clear(),
            "oversized" => rejected.reasoning_chain = Some("x".repeat(1500)),
            "expired" => rejected.created_at -= 60_001,
            "future" => rejected.created_at += 100_000,
            _ => unreachable!(),
        }
        let id = rejected.bundle_id.clone();
        assert_eq!(
            publish(&app, rejected).await.0,
            StatusCode::FORBIDDEN,
            "{kind}"
        );
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM rtx_bundles WHERE bundle_id=?1",
                    [id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn exact_canonical_byte_ceiling_includes_signature_before_scope_stripping() {
    let (app, pool) = setup_app().await;
    let mut contract =
        annex_vrp::VrpCapabilitySharingContract::from_policy(&ServerPolicy::default());
    // Fix the byte ceiling independently of an implementation-side byte count:
    // 700 ASCII reasoning bytes alone exceed this limit even if scope removes them.
    contract.max_exchange_size = Some(700);
    let report = register_limited(&app, &pool, contract).await;
    pool.get()
        .unwrap()
        .execute(
            "UPDATE agent_registrations SET transfer_scope='REFLECTION_SUMMARIES_ONLY'",
            [],
        )
        .unwrap();
    let mut b = bundle(report["handshakeRef"].as_str().unwrap());
    b.reasoning_chain = Some("x".repeat(700));
    assert_eq!(publish(&app, b).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        pool.get()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM rtx_bundles", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
