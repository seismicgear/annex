//! Integration tests for the admin operations surface that closes two
//! deferred items from the hardening pass:
//!
//!   * ADR-0009 — `GET /api/admin/storage` + `POST /api/admin/storage/clear`
//!     (previously, clearing a degraded storage gate required a process
//!     restart).
//!   * ADR-0008 — `GET /api/admin/federation/outbox` +
//!     `POST /api/admin/federation/outbox/{id}/retry`
//!     (previously, operators had to query SQLite directly to inspect or
//!     un-stick failed deliveries).

mod common;

use annex_db::{create_pool, DbPool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::storage_health::StorageHealth;
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::Arc;
use tower::ServiceExt;

/// Builds the app plus handles to the pool and the storage gate so
/// tests can trip / inspect the gate out-of-band.
fn setup() -> (axum::Router, DbPool, Arc<StorageHealth>) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        let policy_json = serde_json::to_string(&ServerPolicy::default()).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active)
             VALUES (1, 'mod_user', 'HUMAN', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active)
             VALUES (1, 'plain_user', 'HUMAN', 0, 1)",
            [],
        )
        .unwrap();
    }

    let tree = MerkleTree::new(20).unwrap();
    let state = common::build_app_state(pool.clone(), tree, ServerPolicy::default());
    let health = state.storage_health.clone();
    (annex_server::app(state), pool, health)
}

/// Builds an authenticated request. The test harness runs with
/// `enforce_zk_proofs = false`, so the dev `X-Annex-Pseudonym` header
/// authenticates directly.
fn req(method: &str, uri: &str, user: &str, body: Body) -> Request<Body> {
    let mut r = Request::builder()
        .uri(uri)
        .method(method)
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", user)
        .body(body)
        .unwrap();
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    r
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Inserts a peer instance and one outbox row, returning the row id.
fn seed_outbox_row(
    pool: &DbPool,
    peer_instance_id: i64,
    message_id: &str,
    status: &str,
    attempts: u32,
    last_error: Option<&str>,
) -> i64 {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT OR IGNORE INTO instances (id, base_url, public_key, label, status)
         VALUES (?1, 'https://peer.example.com', 'pubkey', 'Test Peer', 'ACTIVE')",
        rusqlite::params![peer_instance_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO federation_outbox (peer_instance_id, message_id, envelope_json, status, attempts, last_error, next_retry_at)
         VALUES (?1, ?2, '{\"k\":\"v\"}', ?3, ?4, ?5, datetime('now', '+1 hour'))",
        rusqlite::params![peer_instance_id, message_id, status, attempts, last_error],
    )
    .unwrap();
    conn.last_insert_rowid()
}

// ── Storage gate ──

#[tokio::test]
async fn storage_health_requires_moderator() {
    let (app, _pool, _health) = setup();
    let response = app
        .oneshot(req(
            "GET",
            "/api/admin/storage",
            "plain_user",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn storage_health_reports_healthy_then_degraded() {
    let (app, _pool, health) = setup();

    let response = app
        .clone()
        .oneshot(req("GET", "/api/admin/storage", "mod_user", Body::empty()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["state"], "healthy");
    assert_eq!(json["writes_blocked"], false);
    assert_eq!(json["reason"], "");

    health.mark_degraded("disk full (test)");

    let response = app
        .oneshot(req("GET", "/api/admin/storage", "mod_user", Body::empty()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["state"], "degraded");
    assert_eq!(json["writes_blocked"], true);
    assert_eq!(json["reason"], "disk full (test)");
}

/// The core recovery flow: a degraded gate 507s ordinary writes but the
/// clear endpoint stays reachable, and after clearing, writes flow again.
#[tokio::test]
async fn degraded_gate_blocks_writes_but_clear_endpoint_recovers() {
    let (app, _pool, health) = setup();
    health.mark_degraded("disk full (test)");

    // An ordinary mutating admin request is rejected with 507.
    let response = app
        .clone()
        .oneshot(req(
            "PATCH",
            "/api/admin/server",
            "mod_user",
            Body::from(r#"{"label":"New Label"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::INSUFFICIENT_STORAGE,
        "mutating requests must 507 while the gate is degraded"
    );

    // The clear endpoint is exempt from the gate and recovers it.
    let response = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/admin/storage/clear",
            "mod_user",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the clear endpoint must remain reachable while the gate is degraded"
    );
    let json = body_json(response).await;
    assert_eq!(json["previous_state"], "degraded");
    assert_eq!(json["state"], "healthy");
    assert!(!health.writes_blocked());

    // Writes flow again after the clear.
    let response = app
        .oneshot(req(
            "PATCH",
            "/api/admin/server",
            "mod_user",
            Body::from(r#"{"label":"New Label"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn clear_storage_gate_requires_moderator() {
    let (app, _pool, health) = setup();
    health.mark_degraded("disk full (test)");

    let response = app
        .oneshot(req(
            "POST",
            "/api/admin/storage/clear",
            "plain_user",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        health.writes_blocked(),
        "a non-moderator must not be able to clear the gate"
    );
}

#[tokio::test]
async fn clear_storage_gate_emits_moderation_event() {
    let (app, pool, health) = setup();
    health.mark_degraded("disk full (test)");

    let response = app
        .oneshot(req(
            "POST",
            "/api/admin/storage/clear",
            "mod_user",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let conn = pool.get().unwrap();
    let payload: String = conn
        .query_row(
            "SELECT payload_json FROM public_event_log
             WHERE event_type = 'MODERATION_ACTION'
             ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("a MODERATION_ACTION event must be recorded for the clear");
    assert!(payload.contains("storage_gate_clear"));
    assert!(payload.contains("disk full (test)"));
}

// ── Federation outbox ──

#[tokio::test]
async fn list_outbox_requires_moderator() {
    let (app, _pool, _health) = setup();
    let response = app
        .oneshot(req(
            "GET",
            "/api/admin/federation/outbox",
            "plain_user",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_outbox_returns_rows_and_counts() {
    let (app, pool, _health) = setup();
    seed_outbox_row(&pool, 10, "msg-pending", "pending", 2, None);
    seed_outbox_row(
        &pool,
        10,
        "msg-failed",
        "failed",
        12,
        Some("HTTP 503: unavailable"),
    );
    seed_outbox_row(&pool, 10, "msg-delivered", "delivered", 1, None);

    let response = app
        .oneshot(req(
            "GET",
            "/api/admin/federation/outbox",
            "mod_user",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;

    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    // Most recent first.
    assert_eq!(entries[0]["message_id"], "msg-delivered");
    assert_eq!(entries[2]["message_id"], "msg-pending");
    assert_eq!(entries[0]["peer_base_url"], "https://peer.example.com");
    assert_eq!(entries[0]["peer_label"], "Test Peer");
    assert_eq!(entries[1]["last_error"], "HTTP 503: unavailable");
    assert!(entries[0]["envelope_bytes"].as_i64().unwrap() > 0);

    assert_eq!(json["counts"]["pending"], 1);
    assert_eq!(json["counts"]["failed"], 1);
    assert_eq!(json["counts"]["delivered"], 1);
}

#[tokio::test]
async fn list_outbox_filters_by_status() {
    let (app, pool, _health) = setup();
    seed_outbox_row(&pool, 10, "msg-pending", "pending", 0, None);
    seed_outbox_row(&pool, 10, "msg-failed", "failed", 12, Some("boom"));

    let response = app
        .oneshot(req(
            "GET",
            "/api/admin/federation/outbox?status=failed",
            "mod_user",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let entries = json["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["message_id"], "msg-failed");
    // Counts remain global so the operator still sees total queue depth.
    assert_eq!(json["counts"]["pending"], 1);
}

#[tokio::test]
async fn list_outbox_rejects_invalid_status() {
    let (app, _pool, _health) = setup();
    let response = app
        .oneshot(req(
            "GET",
            "/api/admin/federation/outbox?status=bogus",
            "mod_user",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn retry_resets_failed_row_to_pending() {
    let (app, pool, _health) = setup();
    let id = seed_outbox_row(
        &pool,
        10,
        "msg-1",
        "failed",
        12,
        Some("HTTP 503: unavailable"),
    );

    let response = app
        .oneshot(req(
            "POST",
            &format!("/api/admin/federation/outbox/{id}/retry"),
            "mod_user",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["message_id"], "msg-1");
    assert_eq!(json["new_status"], "pending");

    let conn = pool.get().unwrap();
    let (status, attempts, last_error, due_now): (String, u32, Option<String>, bool) = conn
        .query_row(
            "SELECT status, attempts, last_error, next_retry_at <= datetime('now')
             FROM federation_outbox WHERE id = ?1",
            rusqlite::params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(status, "pending");
    assert_eq!(attempts, 0, "retry must grant a fresh backoff budget");
    assert_eq!(last_error, None);
    assert!(due_now, "a retried row must be due on the next worker tick");
}

#[tokio::test]
async fn retry_conflicts_on_delivered_and_pending_rows() {
    let (app, pool, _health) = setup();
    let delivered = seed_outbox_row(&pool, 10, "msg-d", "delivered", 1, None);
    let pending = seed_outbox_row(&pool, 10, "msg-p", "pending", 3, None);

    for id in [delivered, pending] {
        let response = app
            .clone()
            .oneshot(req(
                "POST",
                &format!("/api/admin/federation/outbox/{id}/retry"),
                "mod_user",
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    // Neither row was mutated.
    let conn = pool.get().unwrap();
    let (status, attempts): (String, u32) = conn
        .query_row(
            "SELECT status, attempts FROM federation_outbox WHERE id = ?1",
            rusqlite::params![pending],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "pending");
    assert_eq!(attempts, 3);
}

#[tokio::test]
async fn retry_returns_404_for_missing_row() {
    let (app, _pool, _health) = setup();
    let response = app
        .oneshot(req(
            "POST",
            "/api/admin/federation/outbox/99999/retry",
            "mod_user",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn retry_requires_moderator() {
    let (app, pool, _health) = setup();
    let id = seed_outbox_row(&pool, 10, "msg-1", "failed", 12, Some("boom"));

    let response = app
        .oneshot(req(
            "POST",
            &format!("/api/admin/federation/outbox/{id}/retry"),
            "plain_user",
            Body::empty(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let conn = pool.get().unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM federation_outbox WHERE id = ?1",
            rusqlite::params![id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "failed");
}
