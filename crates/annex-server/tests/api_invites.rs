//! Tests for the invite redemption endpoint.
//!
//! Crucially: `/api/invites/redeem` validates an invite code WITHOUT
//! consuming a seat. The seat-bump happens later in
//! `IdentityService::register_identity` after the identity is committed.
//! This pins both the validation behaviour and the no-bump semantic.
//!
//! Why this matters: the previous implementation incremented `use_count`
//! on every redeem call. That had two real bugs:
//!
//! 1. A real registration burned 2 seats (one in redeem + one in
//!    register).
//! 2. An unauthenticated attacker could exhaust a `max_uses`-limited
//!    invite by hammering this endpoint, without ever registering.
//!
//! Both are observable by the tests below.

mod common;

use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

fn build_state() -> (axum::Router, annex_db::DbPool) {
    let (router, pool, _policy) = build_state_parts();
    (router, pool)
}

fn build_state_parts() -> (axum::Router, annex_db::DbPool, Arc<RwLock<ServerPolicy>>) {
    let policy_handle = Arc::new(RwLock::new(ServerPolicy::default()));
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        // Seed a server row so the redeem path can fetch slug/label.
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('default', 'Default Server', '{}')",
            [],
        )
        .unwrap();
    }
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
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: Arc::new(RwLock::new("http://localhost:3000".to_string())),
        policy: policy_handle.clone(),
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
        storage_health: std::sync::Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
        shutdown: Default::default(),
        metrics: Default::default(),
    };
    (app(state), pool, policy_handle)
}

fn redeem_request(code: &str) -> Request<Body> {
    let body = serde_json::json!({ "code": code }).to_string();
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/invites/redeem")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    req
}

#[tokio::test]
async fn redeem_does_not_consume_seat_on_success() {
    let (app, pool) = build_state();
    {
        let conn = pool.get().unwrap();
        // 1-use invite — if redeem ever bumps, the second redeem will fail.
        conn.execute(
            "INSERT INTO invite_codes (server_id, code, created_by, max_uses, use_count) \
             VALUES (1, 'CODE-X', 'tester', 1, 0)",
            [],
        )
        .unwrap();
    }

    // Three redeems back-to-back — must all succeed because validation must
    // not bump use_count.
    for i in 0..3 {
        let resp = app.clone().oneshot(redeem_request("CODE-X")).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "redeem #{i} must validate without bumping (max_uses=1, use_count must stay 0)"
        );
    }

    let final_use_count: i64 = pool
        .get()
        .unwrap()
        .query_row(
            "SELECT use_count FROM invite_codes WHERE code = 'CODE-X'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        final_use_count, 0,
        "use_count must stay at 0 after multiple redeems (seat is consumed in register, not redeem)"
    );
}

#[tokio::test]
async fn redeem_rejects_exhausted_invite() {
    let (app, pool) = build_state();
    {
        let conn = pool.get().unwrap();
        // Seed a fully-used invite so the use_count guard fires on the
        // validation path even though redeem itself never bumps.
        conn.execute(
            "INSERT INTO invite_codes (server_id, code, created_by, max_uses, use_count) \
             VALUES (1, 'EXHAUSTED', 'tester', 1, 1)",
            [],
        )
        .unwrap();
    }

    let resp = app
        .clone()
        .oneshot(redeem_request("EXHAUSTED"))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "redeem must reject an invite whose use_count >= max_uses"
    );
}

#[tokio::test]
async fn redeem_rejects_unknown_code() {
    let (app, _pool) = build_state();
    let resp = app
        .clone()
        .oneshot(redeem_request("DOES-NOT-EXIST"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn redeem_rejects_past_expires_at() {
    let (app, pool) = build_state();
    {
        let conn = pool.get().unwrap();
        // Past expiration in the canonical write format.
        conn.execute(
            "INSERT INTO invite_codes \
             (server_id, code, created_by, max_uses, use_count, expires_at) \
             VALUES (1, 'PAST', 'tester', NULL, 0, '2020-01-01 00:00:00')",
            [],
        )
        .unwrap();
    }
    let resp = app.clone().oneshot(redeem_request("PAST")).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "redeem must reject an invite whose expires_at is in the past"
    );
}

#[tokio::test]
async fn redeem_rejects_malformed_expires_at() {
    // Pre-fix, the redeem handler used `if let Ok(exp_dt) = parse_from_str(..)`
    // and silently *fell through* on parse failure — so any non-canonical
    // value (operator-issued ISO 8601, manual repair, format drift in a
    // future migration) would silently extend the invite's life forever.
    // Defence in depth: malformed expires_at is rejected as expired.
    let (app, pool) = build_state();
    {
        let conn = pool.get().unwrap();
        // Five distinct shapes that all USED to bypass expiration
        // because they don't match `%Y-%m-%d %H:%M:%S`. All should now
        // be rejected.
        for (code, exp) in [
            ("ISO8601", "2030-01-01T00:00:00Z"),
            ("DATE_ONLY", "2030-01-01"),
            ("EMPTY", ""),
            ("GARBAGE", "tomorrow"),
            ("FRACTIONAL", "2030-01-01 00:00:00.123"),
        ] {
            conn.execute(
                "INSERT INTO invite_codes \
                 (server_id, code, created_by, max_uses, use_count, expires_at) \
                 VALUES (1, ?1, 'tester', NULL, 0, ?2)",
                rusqlite::params![code, exp],
            )
            .unwrap();
        }
    }
    for code in ["ISO8601", "DATE_ONLY", "EMPTY", "GARBAGE", "FRACTIONAL"] {
        let resp = app.clone().oneshot(redeem_request(code)).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "redeem must reject invite '{code}' with non-canonical expires_at"
        );
    }
}

#[tokio::test]
async fn redeem_accepts_canonical_future_expires_at() {
    let (app, pool) = build_state();
    {
        let conn = pool.get().unwrap();
        // Canonical-format future expiration must redeem cleanly.
        conn.execute(
            "INSERT INTO invite_codes \
             (server_id, code, created_by, max_uses, use_count, expires_at) \
             VALUES (1, 'FUTURE', 'tester', NULL, 0, '2099-12-31 23:59:59')",
            [],
        )
        .unwrap();
    }
    let resp = app.clone().oneshot(redeem_request("FUTURE")).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "redeem must accept an invite whose expires_at is well in the future"
    );
}

// ── Admission must be one decision, not a check and a hope ────────────────
//
// `register_identity` used to validate the invite, create the identity, and
// only THEN claim a seat. When the claim affected zero rows — because a
// concurrent request had taken the last one in between — it logged a warning
// and accepted the registration anyway. The code called that a documented
// compensation path. It is an over-issue: a one-use invite admits two
// identities, and the only record is a log line.
//
// The seat is now claimed BEFORE anything is created, and released again on
// any path that creates nothing.

fn seed_invite(pool: &annex_db::DbPool, code: &str, max_uses: i64) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO invite_codes (server_id, code, created_by, max_uses, use_count) \
         VALUES (1, ?1, 'founder', ?2, 0)",
        rusqlite::params![code, max_uses],
    )
    .unwrap();
}

/// A server whose access mode is invite-only.
///
/// `read_access_mode` reads `AppState.policy` — the in-memory copy — not
/// `servers.policy_json`. Updating the row and expecting the handler to notice
/// was the first draft here, and it produced tests in which every registration
/// succeeded because the gate was never armed: the invite code was accepted,
/// ignored, and no seat was ever claimed. Exactly the "a check keyed on a
/// different identifier than the query it guards" shape, in a test.
fn build_invite_only_state() -> (axum::Router, annex_db::DbPool) {
    let (router, pool, policy) = build_state_parts();
    policy.write().unwrap().access_mode = "invite_only".to_string();
    (router, pool)
}

fn use_count(pool: &annex_db::DbPool, code: &str) -> i64 {
    let conn = pool.get().unwrap();
    conn.query_row(
        "SELECT use_count FROM invite_codes WHERE code = ?1",
        [code],
        |r| r.get(0),
    )
    .unwrap()
}

fn register_request(commitment: &str, code: &str) -> Request<Body> {
    let body = serde_json::json!({
        "commitmentHex": commitment,
        // 1 = HUMAN. `roleCode` is 1..=5; 0 is rejected before the invite is
        // ever consulted, which made the first draft of these tests 422 on a
        // body the handler never parsed.
        "roleCode": 1,
        "nodeId": 1,
        "inviteCode": code,
    })
    .to_string();
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/registry/register")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    req
}

/// A 32-byte commitment as 64 lowercase hex chars, distinct per `n`.
fn commitment(n: u8) -> String {
    let mut bytes = [0u8; 32];
    bytes[31] = n;
    bytes[0] = 1; // keep it inside the field
    hex::encode(bytes)
}

#[tokio::test]
async fn a_one_use_invite_admits_exactly_one_identity() {
    let (app, pool) = build_invite_only_state();
    seed_invite(&pool, "ONE-SEAT", 1);

    let first = app
        .clone()
        .oneshot(register_request(&commitment(1), "ONE-SEAT"))
        .await
        .unwrap();
    assert_eq!(
        first.status(),
        StatusCode::OK,
        "the first seat must be granted"
    );

    let second = app
        .clone()
        .oneshot(register_request(&commitment(2), "ONE-SEAT"))
        .await
        .unwrap();
    let status = second.status();
    let bytes = axum::body::to_bytes(second.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes);

    assert_ne!(
        status,
        StatusCode::OK,
        "a second identity was admitted on a one-use invite. Before the fix this \
         succeeded and logged a warning. Body: {body}"
    );
    assert_eq!(
        use_count(&pool, "ONE-SEAT"),
        1,
        "use_count must not exceed max_uses"
    );
}

/// Retrying a successful registration must cost nothing. The response may be
/// lost in transit and a client will retry; returning the same identity is not
/// idempotent if the retry burns another seat.
#[tokio::test]
async fn retrying_a_successful_registration_spends_no_extra_seat() {
    let (app, pool) = build_invite_only_state();
    seed_invite(&pool, "TWO-SEATS", 2);

    let c = commitment(7);

    let first = app
        .clone()
        .oneshot(register_request(&c, "TWO-SEATS"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(
        use_count(&pool, "TWO-SEATS"),
        1,
        "the first admission spends one"
    );

    // Same commitment: an idempotent replay, creating nothing.
    let retry = app
        .clone()
        .oneshot(register_request(&c, "TWO-SEATS"))
        .await
        .unwrap();
    assert_eq!(
        retry.status(),
        StatusCode::OK,
        "a replay must still succeed — it is how a client recovers a lost response"
    );
    assert_eq!(
        use_count(&pool, "TWO-SEATS"),
        1,
        "the replay created no identity and must not have spent a second seat"
    );

    // And the seat it did not spend is still available to someone else.
    let other = app
        .clone()
        .oneshot(register_request(&commitment(8), "TWO-SEATS"))
        .await
        .unwrap();
    assert_eq!(
        other.status(),
        StatusCode::OK,
        "the second seat should still have been free"
    );
    assert_eq!(use_count(&pool, "TWO-SEATS"), 2);
}

/// An invite that is already exhausted is refused before anything is created.
#[tokio::test]
async fn an_exhausted_invite_is_refused_without_creating_an_identity() {
    let (app, pool) = build_invite_only_state();
    seed_invite(&pool, "SPENT", 1);
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE invite_codes SET use_count = 1 WHERE code = 'SPENT'",
            [],
        )
        .unwrap();
    }

    let response = app
        .clone()
        .oneshot(register_request(&commitment(9), "SPENT"))
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::OK);

    // Scoped: `create_pool(":memory:", ..)` clamps the pool to ONE connection,
    // so holding this one while `use_count` asks for another blocks until
    // r2d2's 30s timeout and then panics — which reads as "the invite row
    // disappeared" rather than as a deadlock in the test.
    let identities: i64 = {
        let conn = pool.get().unwrap();
        conn.query_row("SELECT COUNT(*) FROM vrp_identities", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(
        identities, 0,
        "a refused registration must leave no identity behind"
    );
    assert_eq!(
        use_count(&pool, "SPENT"),
        1,
        "and must not move the counter"
    );
}
