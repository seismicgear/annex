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
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: Arc::new(RwLock::new("http://localhost:3000".to_string())),
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
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: std::sync::Arc::new(annex_server::storage_health::StorageHealth::new()),
    };
    (app(state), pool)
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
