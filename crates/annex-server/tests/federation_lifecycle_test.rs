//! Full federation lifecycle integration test.
//!
//! This test validates Phase 8 completion criterion:
//! "Two servers federate -> user on A sends message -> user on B receives it
//!  -> A changes policy -> trust downgrades -> restricted behavior enforced"
//!
//! We simulate the perspective of Server B (local) receiving interactions from
//! Server A (remote). The test covers:
//!
//! 1. Bilateral VRP handshake establishing federation
//! 2. Cross-server identity attestation (remote user attested locally)
//! 3. Federated channel creation and remote user joining
//! 4. Cross-server message relay with cryptographic verification
//! 5. Policy change triggering re-evaluation
//! 6. Federation severed on CONFLICT — message relay rejected

use annex_db::{create_pool, DbRuntimeSettings};
use annex_federation::FederatedMessageEnvelope;
use annex_identity::MerkleTree;
use annex_server::{
    api_ws::ConnectionManager, app, middleware::RateLimiter,
    policy::recalculate_federation_agreements, AppState,
};
use annex_types::ServerPolicy;
use annex_vrp::{
    VrpAlignmentStatus, VrpAnchorSnapshot, VrpCapabilitySharingContract, VrpFederationHandshake,
    VrpValidationReport,
};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tempfile::NamedTempFile;
use tower::ServiceExt;

/// Builds a fresh AppState backed by a file-based SQLite database.
///
/// We use a file-based DB (via `NamedTempFile`) because `recalculate_federation_agreements`
/// runs in a `spawn_blocking` task that needs its own connection from the pool.
/// In-memory databases with `r2d2` can sometimes have issues with concurrent access
/// across threads when each connection gets its own in-memory database.
fn build_state(db_path: &str, initial_policy: ServerPolicy) -> (Arc<AppState>, annex_db::DbPool) {
    let pool =
        create_pool(db_path, DbRuntimeSettings::default()).expect("failed to create db pool");
    let conn = pool.get().expect("failed to get connection");
    annex_db::run_migrations(&conn).expect("failed to run migrations");

    let policy_json = serde_json::to_string(&initial_policy).expect("failed to serialize policy");
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'server-b', 'Server B', ?1)",
        rusqlite::params![policy_json],
    )
    .expect("failed to insert server");

    drop(conn);

    let tree = MerkleTree::new(20).expect("failed to create merkle tree");
    let (presence_tx, _) = tokio::sync::broadcast::channel(100);

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(annex_identity::zk::generate_dummy_vkey()),
        server_id: 1,
        signing_key: Arc::new(SigningKey::generate(&mut OsRng)),
        public_url: "http://server-b.local".to_string(),
        policy: Arc::new(RwLock::new(initial_policy)),
        rate_limiter: RateLimiter::new(),
        connection_manager: ConnectionManager::new(),
        presence_tx,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::LiveKitConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("dummy", "dummy")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
    };

    (Arc::new(state), pool)
}

/// Helper: send a request via `oneshot` with ConnectInfo injected.
async fn send_request(router: axum::Router, request: Request<Body>) -> axum::response::Response {
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = request;
    request.extensions_mut().insert(ConnectInfo(addr));
    router.oneshot(request).await.expect("request failed")
}

/// Helper: build a signed `FederatedMessageEnvelope`.
fn build_signed_envelope(
    signing_key: &SigningKey,
    message_id: &str,
    channel_id: &str,
    content: &str,
    sender_pseudonym: &str,
    originating_server: &str,
    attestation_ref: &str,
    created_at: &str,
) -> FederatedMessageEnvelope {
    let signature_input = format!(
        "{}{}{}{}{}{}{}",
        message_id,
        channel_id,
        content,
        sender_pseudonym,
        originating_server,
        attestation_ref,
        created_at
    );
    let signature = signing_key.sign(signature_input.as_bytes());

    FederatedMessageEnvelope {
        message_id: message_id.to_string(),
        channel_id: channel_id.to_string(),
        content: content.to_string(),
        sender_pseudonym: sender_pseudonym.to_string(),
        originating_server: originating_server.to_string(),
        attestation_ref: attestation_ref.to_string(),
        signature: hex::encode(signature.to_bytes()),
        created_at: created_at.to_string(),
    }
}

#[tokio::test]
async fn test_federation_full_lifecycle() {
    // =========================================================================
    // SETUP: Server B (local) with federation-enabled policy
    // =========================================================================
    let temp_file = NamedTempFile::new().expect("failed to create temp file");
    let db_path = temp_file
        .path()
        .to_str()
        .expect("invalid db path")
        .to_string();

    let initial_policy = ServerPolicy {
        federation_enabled: true,
        ..Default::default()
    };

    let (state, pool) = build_state(&db_path, initial_policy);

    // Server A (remote) keypair
    let remote_signing_key = SigningKey::generate(&mut OsRng);
    let remote_public_key = remote_signing_key.verifying_key();
    let remote_public_key_hex = hex::encode(remote_public_key.as_bytes());
    let remote_origin = "http://server-a.local";

    // Register Server A as a known instance
    {
        let conn = pool.get().expect("failed to get connection");
        conn.execute(
            "INSERT INTO instances (id, base_url, public_key, label, status) \
             VALUES (10, ?1, ?2, 'Server A', 'ACTIVE')",
            rusqlite::params![remote_origin, remote_public_key_hex],
        )
        .expect("failed to insert remote instance");
    }

    // =========================================================================
    // STEP 1: Federation handshake (Server A -> Server B)
    // =========================================================================
    let anchor = VrpAnchorSnapshot::new(&[], &[]); // Matches empty default policy
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec!["federation".to_string()],
        redacted_topics: vec![],
    };
    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor.clone(),
        capability_contract: contract.clone(),
    };

    let handshake_payload = json!({
        "base_url": remote_origin,
        "anchor_snapshot": handshake.anchor_snapshot,
        "capability_contract": handshake.capability_contract
    });

    let request = Request::builder()
        .uri("/api/federation/handshake")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(handshake_payload.to_string()))
        .expect("failed to build request");

    let response = send_request(app((*state).clone()), request).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "Federation handshake should succeed"
    );

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read body");
    let report: VrpValidationReport =
        serde_json::from_slice(&body_bytes).expect("failed to parse VrpValidationReport");

    assert_eq!(
        report.alignment_status,
        VrpAlignmentStatus::Aligned,
        "Initial handshake should produce Aligned status"
    );

    // Verify agreement persisted in DB
    {
        let conn = pool.get().expect("failed to get connection");
        let (status, active): (String, bool) = conn
            .query_row(
                "SELECT alignment_status, active FROM federation_agreements \
                 WHERE remote_instance_id = 10",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("failed to query agreement");
        assert_eq!(status, "ALIGNED");
        assert!(active, "Agreement should be active");
    }

    // =========================================================================
    // STEP 2: Cross-server identity attestation (simulate)
    //
    // In production, Server A sends an AttestationRequest to Server B,
    // Server B verifies the ZK proof against Server A's Merkle root.
    // Here we simulate the result: a federated_identities row.
    // =========================================================================
    let commitment_hex = "0000000000000000000000000000000000000000000000000000000000000042";
    let topic = "annex:server:v1";
    let local_pseudonym_id = "federated-user-alice";

    {
        let conn = pool.get().expect("failed to get connection");
        conn.execute(
            "INSERT INTO federated_identities \
             (server_id, remote_instance_id, commitment_hex, pseudonym_id, vrp_topic, attested_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            rusqlite::params![
                state.server_id,
                10,
                commitment_hex,
                local_pseudonym_id,
                topic
            ],
        )
        .expect("failed to insert federated identity");

        // Also create platform identity (needed for channel membership)
        conn.execute(
            "INSERT INTO platform_identities \
             (server_id, pseudonym_id, participant_type, active) \
             VALUES (?1, ?2, 'HUMAN', 1)",
            rusqlite::params![state.server_id, local_pseudonym_id],
        )
        .expect("failed to insert platform identity");
    }

    // =========================================================================
    // STEP 3: Create federated channel and add remote user
    // =========================================================================
    let channel_id = "fed-lifecycle-channel";

    {
        let conn = pool.get().expect("failed to get connection");
        conn.execute(
            r#"INSERT INTO channels
                (server_id, channel_id, name, channel_type, federation_scope, created_at)
               VALUES (?1, ?2, 'Lifecycle Chat', '"Text"', '"Federated"', datetime('now'))"#,
            rusqlite::params![state.server_id, channel_id],
        )
        .expect("failed to insert channel");

        conn.execute(
            "INSERT INTO channel_members \
             (server_id, channel_id, pseudonym_id, role, joined_at) \
             VALUES (?1, ?2, ?3, 'MEMBER', datetime('now'))",
            rusqlite::params![state.server_id, channel_id, local_pseudonym_id],
        )
        .expect("failed to add member to channel");
    }

    // =========================================================================
    // STEP 4: Cross-server message relay — Server A sends message to Server B
    // =========================================================================
    let attestation_ref = format!("{}:{}", topic, commitment_hex);

    let envelope = build_signed_envelope(
        &remote_signing_key,
        "msg-lifecycle-001",
        channel_id,
        "Hello from Server A!",
        "alice-on-server-a",
        remote_origin,
        &attestation_ref,
        "2026-02-17T12:00:00Z",
    );

    let request = Request::builder()
        .uri("/api/federation/messages")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&envelope).expect("failed to serialize envelope"),
        ))
        .expect("failed to build request");

    let response = send_request(app((*state).clone()), request).await;

    if response.status() != StatusCode::OK {
        let status = response.status();
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("failed to read body");
        let body_str = String::from_utf8_lossy(&body_bytes);
        panic!(
            "Federated message relay should succeed, got {}: {}",
            status, body_str
        );
    }

    // Verify message persisted with LOCAL pseudonym (not the remote one)
    {
        let conn = pool.get().expect("failed to get connection");
        let (sender, content): (String, String) = conn
            .query_row(
                "SELECT sender_pseudonym, content FROM messages WHERE message_id = ?1",
                rusqlite::params!["msg-lifecycle-001"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("failed to query message");

        assert_eq!(
            sender, local_pseudonym_id,
            "Message should be stored with local pseudonym, not remote"
        );
        assert_eq!(content, "Hello from Server A!");
    }

    // =========================================================================
    // STEP 5: Policy change — Server B adds principles that cause CONFLICT
    //
    // The remote handshake had empty anchor (no principles, no prohibited actions).
    // Adding principles to Server B's policy will cause a mismatch on re-evaluation
    // because the remote's anchor snapshot won't match the new policy root.
    // =========================================================================
    {
        let mut policy = state.policy.write().expect("failed to acquire policy lock");
        policy.principles.push("Privacy is paramount".to_string());
        policy
            .principles
            .push("No surveillance capitalism".to_string());
    }

    // Trigger re-evaluation
    recalculate_federation_agreements(state.clone())
        .await
        .expect("recalculation should not error");

    // =========================================================================
    // STEP 6: Verify federation severed — agreement downgraded to CONFLICT
    // =========================================================================
    {
        let conn = pool.get().expect("failed to get connection");
        let (status, active): (String, bool) = conn
            .query_row(
                "SELECT alignment_status, active FROM federation_agreements \
                 WHERE remote_instance_id = 10",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("failed to query agreement after re-evaluation");

        assert_eq!(
            status, "CONFLICT",
            "Agreement should be downgraded to CONFLICT after policy change"
        );
        assert!(
            !active,
            "Agreement should be deactivated when alignment drops to CONFLICT"
        );
    }

    // =========================================================================
    // STEP 7: Verify restricted behavior — message relay rejected
    //
    // With the federation agreement deactivated, Server B should reject
    // incoming messages from Server A.
    // =========================================================================
    let rejected_envelope = build_signed_envelope(
        &remote_signing_key,
        "msg-lifecycle-002",
        channel_id,
        "This should be rejected!",
        "alice-on-server-a",
        remote_origin,
        &attestation_ref,
        "2026-02-17T13:00:00Z",
    );

    let request = Request::builder()
        .uri("/api/federation/messages")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_string(&rejected_envelope).expect("failed to serialize envelope"),
        ))
        .expect("failed to build request");

    let response = send_request(app((*state).clone()), request).await;

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "Message relay should be rejected after federation is severed"
    );

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("failed to read body");
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(
        body_str.contains("No active federation agreement"),
        "Rejection should mention missing active agreement, got: {}",
        body_str
    );

    // Verify the rejected message was NOT persisted
    {
        let conn = pool.get().expect("failed to get connection");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE message_id = ?1",
                rusqlite::params!["msg-lifecycle-002"],
                |row| row.get(0),
            )
            .expect("failed to count messages");

        assert_eq!(
            count, 0,
            "Rejected message should not be persisted in the database"
        );
    }

    // =========================================================================
    // STEP 8: Verify federated channel join is also rejected
    //
    // After federation is severed, new remote users should not be able to
    // join federated channels.
    // =========================================================================
    let join_message = format!("{}new-remote-user", channel_id);
    let join_signature = remote_signing_key.sign(join_message.as_bytes());
    let join_signature_hex = hex::encode(join_signature.to_bytes());

    // First, insert another federated identity to simulate a second remote user
    {
        let conn = pool.get().expect("failed to get connection");
        conn.execute(
            "INSERT INTO federated_identities \
             (server_id, remote_instance_id, commitment_hex, pseudonym_id, vrp_topic) \
             VALUES (?1, ?2, 'aabbccdd', 'new-remote-user', 'annex:server:v1')",
            rusqlite::params![state.server_id, 10],
        )
        .expect("failed to insert second federated identity");

        conn.execute(
            "INSERT INTO platform_identities \
             (server_id, pseudonym_id, participant_type, active) \
             VALUES (?1, 'new-remote-user', 'HUMAN', 1)",
            rusqlite::params![state.server_id],
        )
        .expect("failed to insert platform identity");
    }

    let join_payload = json!({
        "originating_server": remote_origin,
        "pseudonym_id": "new-remote-user",
        "signature": join_signature_hex
    });

    let request = Request::builder()
        .uri(format!("/api/federation/channels/{}/join", channel_id))
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(join_payload.to_string()))
        .expect("failed to build request");

    let response = send_request(app((*state).clone()), request).await;

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "Channel join should be rejected after federation is severed"
    );
}
