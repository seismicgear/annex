mod common;

use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use annex_vrp::{
    VrpAlignmentStatus, VrpAnchorSnapshot, VrpCapabilitySharingContract, VrpFederationHandshake,
    VrpTransferScope, VrpValidationReport,
};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt; // for oneshot

async fn setup_app() -> (axum::Router, annex_db::DbPool) {
    setup_app_with_policy(ServerPolicy::default()).await
}

async fn setup_app_with_policy(policy: ServerPolicy) -> (axum::Router, annex_db::DbPool) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Insert a server row for FK constraints
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'test-server', 'Test Server', '{}')",
        [],
    )
    .unwrap();

    drop(conn);

    let tree = MerkleTree::new(20).unwrap();

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: common::load_vkey_or_dummy(),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
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
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: std::sync::Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
    };

    (app(state), pool)
}

#[tokio::test]
async fn test_agent_handshake_aligned() {
    let (app, pool) = setup_app().await;

    // 1. Create Handshake Payload (Aligned)
    // ServerPolicy default has empty principles/prohibitions.
    // We match that for Aligned status.
    let anchor = VrpAnchorSnapshot::new(&[], &[]).unwrap();

    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec!["TEXT".to_string(), "VRP".to_string()],
        redacted_topics: vec![],
    };

    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };

    let payload = serde_json::json!({
        "pseudonymId": "agent-123",
        "handshake": handshake
    });

    // 2. Send Request
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/vrp/agent-handshake")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();

    // 3. Verify Response
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let report: VrpValidationReport = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(report.alignment_status, VrpAlignmentStatus::Aligned);
    // Default transfer config allows reflection summaries for agents (hardcoded in handler for now)
    assert_eq!(
        report.transfer_scope,
        VrpTransferScope::ReflectionSummariesOnly
    );

    // 4. Verify DB State
    let conn = pool.get().unwrap();

    // Check agent_registrations
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_registrations WHERE pseudonym_id = 'agent-123')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(exists, "agent registration should be created");

    // Check handshake log
    let log_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM vrp_handshake_log WHERE peer_pseudonym = 'agent-123'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(log_count, 1, "handshake should be logged");
}

#[tokio::test]
async fn test_agent_handshake_conflict() {
    // The server declares its own values, and the agent's are unrelated to
    // them. That is what a conflict is.
    //
    // This test used to run against `ServerPolicy::default()`, whose
    // `principles` list is empty, and assert that an agent declaring any
    // principle at all was rejected. That passed, and it was the bug: with
    // no local principles the semantic branch in `compare_peer_anchor_scored`
    // was unreachable and everything fell through to Conflict, so the only
    // agent a stock server could admit was one with no anchor whatsoever.
    // `agent_min_alignment_score` was never consulted. The test's own
    // comment — "simple comparison: if hashes differ -> Conflict" — was
    // describing the mechanism rather than any intended policy.
    //
    // A server that has stated no values has nothing for an agent to
    // conflict with; that case is now covered by
    // `test_agent_handshake_admitted_when_server_declares_no_principles`.
    let policy = ServerPolicy {
        principles: vec!["user privacy is paramount".to_string()],
        ..Default::default()
    };
    let (app, pool) = setup_app_with_policy(policy).await;

    let anchor =
        VrpAnchorSnapshot::new(&["maximise engagement at any cost".to_string()], &[]).unwrap();

    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec![],
        redacted_topics: vec![],
    };

    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };

    let payload = serde_json::json!({
        "pseudonymId": "agent-conflict",
        "handshake": handshake
    });

    // 2. Send Request
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/vrp/agent-handshake")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();

    // 3. Verify Response
    assert_eq!(response.status(), StatusCode::OK); // 200 OK with Conflict status

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let report: VrpValidationReport = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(report.alignment_status, VrpAlignmentStatus::Conflict);
    assert_eq!(report.transfer_scope, VrpTransferScope::NoTransfer);

    // 4. Verify DB State
    let conn = pool.get().unwrap();

    // Check agent_registrations - should NOT exist
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_registrations WHERE pseudonym_id = 'agent-conflict')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        !exists,
        "agent registration should NOT be created on conflict"
    );

    // Check handshake log - should exist
    let log_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM vrp_handshake_log WHERE peer_pseudonym = 'agent-conflict'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(log_count, 1, "handshake should be logged even on conflict");
}

/// Re-handshake from an unauthenticated caller against an already-registered
/// AI agent must be rejected. Without this gate, anyone who can read the
/// agent's pseudonym (it's in `/api/public/agents`, the events stream, and
/// channel listings) could submit a fresh anchor/contract and silently
/// rewrite the agent's `agent_registrations` row, including capability
/// contracts and alignment status.
#[tokio::test]
async fn rehandshake_without_token_is_rejected_for_registered_agent() {
    let (app, pool) = setup_app().await;

    // Seed a platform_identities row marking this pseudonym as a registered
    // AI agent. With this row in place, the handshake handler must require
    // a valid session-token Authorization header.
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
             VALUES (1, ?1, ?2, 1)",
            rusqlite::params![
                "agent-already-registered",
                annex_types::RoleCode::AiAgent.label()
            ],
        )
        .unwrap();
    }

    let anchor = VrpAnchorSnapshot::new(&[], &[]).unwrap();
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec!["TEXT".to_string(), "VRP".to_string()],
        redacted_topics: vec![],
    };
    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };
    let payload = serde_json::json!({
        "pseudonymId": "agent-already-registered",
        "handshake": handshake
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/vrp/agent-handshake")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "registered agent must require a session token for re-handshake"
    );
}

/// Re-handshake from a session token bound to a different pseudonym must be
/// rejected. This pins the second leg of the binding: a valid token alone
/// is not enough — it has to be a token issued for the same pseudonym whose
/// handshake we're rewriting.
#[tokio::test]
async fn rehandshake_with_mismatched_token_is_rejected() {
    let (app, pool) = setup_app().await;

    // Same setup as above — an existing AI agent.
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
             VALUES (1, ?1, ?2, 1)",
            rusqlite::params!["agent-victim", annex_types::RoleCode::AiAgent.label()],
        )
        .unwrap();
    }

    // Issue a session token for a *different* pseudonym (the attacker).
    // Use the `[0u8; 32]` `ws_token_secret` configured in `setup_app`.
    let attacker_token = annex_server::api_ws::generate_session_token(
        "attacker-pseudonym",
        &[0u8; 32],
        annex_server::api_ws::SESSION_TOKEN_TTL_SECS,
    );

    let anchor = VrpAnchorSnapshot::new(&[], &[]).unwrap();
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec!["TEXT".to_string(), "VRP".to_string()],
        redacted_topics: vec![],
    };
    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };
    let payload = serde_json::json!({
        "pseudonymId": "agent-victim",
        "handshake": handshake
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/vrp/agent-handshake")
        .method("POST")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {attacker_token}"))
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "mismatched session token must not authorise a re-handshake"
    );
}

/// Pre-registration handshake (no platform_identities row yet) must still
/// succeed without an Authorization header. This is the path real agents
/// hit on first contact, before the identity registration + verify-membership
/// flow has run.
#[tokio::test]
async fn pre_registration_handshake_remains_unauthenticated() {
    let (app, _pool) = setup_app().await;

    let anchor = VrpAnchorSnapshot::new(&[], &[]).unwrap();
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec!["TEXT".to_string(), "VRP".to_string()],
        redacted_topics: vec![],
    };
    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };
    let payload = serde_json::json!({
        "pseudonymId": "agent-fresh",
        "handshake": handshake
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/vrp/agent-handshake")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "pre-registration handshake must still work without auth"
    );
}

/// A re-handshake authenticated with a valid session token bound to the
/// same pseudonym must succeed. This is the legitimate re-handshake flow
/// the new gate is designed to allow — the agent still owns the
/// capability-contract / anchor it controls.
#[tokio::test]
async fn rehandshake_with_matching_token_is_allowed() {
    let (app, pool) = setup_app().await;

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
             VALUES (1, ?1, ?2, 1)",
            rusqlite::params!["agent-owner", annex_types::RoleCode::AiAgent.label()],
        )
        .unwrap();
    }

    let owner_token = annex_server::api_ws::generate_session_token(
        "agent-owner",
        &[0u8; 32],
        annex_server::api_ws::SESSION_TOKEN_TTL_SECS,
    );

    let anchor = VrpAnchorSnapshot::new(&[], &[]).unwrap();
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec!["TEXT".to_string(), "VRP".to_string()],
        redacted_topics: vec![],
    };
    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };
    let payload = serde_json::json!({
        "pseudonymId": "agent-owner",
        "handshake": handshake
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/vrp/agent-handshake")
        .method("POST")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {owner_token}"))
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "matching session token must authorise a re-handshake"
    );
}

/// A server running the shipped defaults must be able to register an agent
/// that declares an ethical anchor.
///
/// This is the case the old `test_agent_handshake_conflict` asserted the
/// opposite of. `ServerPolicy::default()` has `principles: []`, and every
/// agent that arrived with principles of its own was classified Conflict, no
/// `agent_registrations` row was written, and the agent's WebSocket was
/// disconnected — after which every `POST /api/channels/{id}/join` from it
/// 403'd with "agent not registered". Agent participation did not work out
/// of the box, and nothing pointed at the empty `principles` list as the
/// reason.
#[tokio::test]
async fn test_agent_handshake_admitted_when_server_declares_no_principles() {
    let (app, pool) = setup_app().await;

    let anchor = VrpAnchorSnapshot::new(&["users deserve privacy".to_string()], &[]).unwrap();

    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: VrpCapabilitySharingContract {
            required_capabilities: vec![],
            offered_capabilities: vec![],
            redacted_topics: vec![],
        },
    };

    let payload = serde_json::json!({
        "pseudonymId": "agent-with-values",
        "handshake": handshake,
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/vrp/agent-handshake")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let report: VrpValidationReport = serde_json::from_slice(&body_bytes).unwrap();
    assert_ne!(
        report.alignment_status,
        VrpAlignmentStatus::Conflict,
        "a default server rejected an agent for having principles",
    );

    // The registration row is what every later join checks, so its absence
    // is the part that actually breaks the feature.
    let conn = pool.get().unwrap();
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_registrations WHERE pseudonym_id = 'agent-with-values')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(exists, "no agent_registrations row was written");
}
