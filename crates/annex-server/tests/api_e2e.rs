//! Integration tests for the content-blind E2E channel-key directory
//! (`api_e2e.rs`). The decisive test (`key_wrap_roundtrip_is_content_blind`)
//! seals a channel key with the real Rust sealed box, stores it through the
//! server, then opens it as the recipient — proving the server only ever holds
//! ciphertext it cannot read.

mod common;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use base64::Engine;
use serde_json::Value;
use std::net::SocketAddr;
use tower::ServiceExt;

fn seed_identity(pool: &annex_db::DbPool, pseudonym: &str, can_moderate: bool) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active)
         VALUES (1, ?1, 'HUMAN', ?2, 1)",
        rusqlite::params![pseudonym, can_moderate as i64],
    )
    .unwrap();
}

fn seed_channel(pool: &annex_db::DbPool, channel_id: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO channels (server_id, channel_id, name, channel_type)
         VALUES (1, ?1, ?1, 'TEXT')",
        rusqlite::params![channel_id],
    )
    .unwrap();
}

fn seed_member(pool: &annex_db::DbPool, channel_id: &str, pseudonym: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO channel_members (server_id, channel_id, pseudonym_id) VALUES (1, ?1, ?2)",
        rusqlite::params![channel_id, pseudonym],
    )
    .unwrap();
}

fn request(method: &str, uri: &str, pseudonym: &str, body: Option<Value>) -> Request<Body> {
    let body = match body {
        Some(v) => Body::from(v.to_string()),
        None => Body::empty(),
    };
    let mut req = Request::builder()
        .uri(uri)
        .method(method)
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", pseudonym)
        .body(body)
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    req
}

async fn json_body(resp: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

const VALID_PUB: &str = "07a37cbc142093c8b755dc1b10e86cb426374ad16aa853ed0bdfc0b2b86d1c7c";

#[tokio::test]
async fn publish_and_fetch_member_key() {
    let (app, pool) = common::setup_test_app().await;
    seed_identity(&pool, "alice", false);

    let resp = app
        .clone()
        .oneshot(request(
            "PUT",
            "/api/keys/me",
            "alice",
            Some(serde_json::json!({ "x25519_pub_hex": VALID_PUB })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(request("GET", "/api/keys/alice", "alice", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["x25519_pub_hex"], VALID_PUB);
    assert_eq!(body["pseudonym_id"], "alice");
}

#[tokio::test]
async fn rejects_malformed_key() {
    let (app, pool) = common::setup_test_app().await;
    seed_identity(&pool, "alice", false);

    for bad in ["xyz", "07A3", &"zz".repeat(32)] {
        let resp = app
            .clone()
            .oneshot(request(
                "PUT",
                "/api/keys/me",
                "alice",
                Some(serde_json::json!({ "x25519_pub_hex": bad })),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "bad key: {bad}");
    }
}

#[tokio::test]
async fn missing_key_is_404() {
    let (app, pool) = common::setup_test_app().await;
    seed_identity(&pool, "alice", false);
    let resp = app
        .oneshot(request("GET", "/api/keys/nobody", "alice", None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn member_key_directory_lists_only_channel_members() {
    let (app, pool) = common::setup_test_app().await;
    seed_channel(&pool, "chan");
    for p in ["alice", "bob", "carol"] {
        seed_identity(&pool, p, false);
        seed_member(&pool, "chan", p);
        app.clone()
            .oneshot(request(
                "PUT",
                "/api/keys/me",
                p,
                Some(serde_json::json!({ "x25519_pub_hex": VALID_PUB })),
            ))
            .await
            .unwrap();
    }
    // An outsider who is not in the channel.
    seed_identity(&pool, "mallory", false);

    let resp = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/channels/chan/member-keys",
            "alice",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert_eq!(body["member_keys"].as_array().unwrap().len(), 3);

    // Non-member is forbidden.
    let resp = app
        .oneshot(request(
            "GET",
            "/api/channels/chan/member-keys",
            "mallory",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// The decisive end-to-end test: a channel key sealed with the production Rust
/// sealed box, stored through the server, and opened by the recipient — while
/// the server only ever sees ciphertext.
#[tokio::test]
async fn key_wrap_roundtrip_is_content_blind() {
    use annex_federation::seal::{open_x25519, seal_x25519, x25519_public_key};

    let (app, pool) = common::setup_test_app().await;
    seed_channel(&pool, "secret-chan");
    seed_identity(&pool, "alice", false);
    seed_identity(&pool, "bob", false);
    seed_member(&pool, "secret-chan", "alice");
    seed_member(&pool, "secret-chan", "bob");

    // Bob's device key. The secret never leaves "Bob".
    let bob_secret = [0x11u8; 32];
    let bob_pub = x25519_public_key(&bob_secret);
    app.clone()
        .oneshot(request(
            "PUT",
            "/api/keys/me",
            "bob",
            Some(serde_json::json!({ "x25519_pub_hex": hex::encode(bob_pub) })),
        ))
        .await
        .unwrap();

    // Alice fetches Bob's published key from the directory.
    let resp = app
        .clone()
        .oneshot(request("GET", "/api/keys/bob", "alice", None))
        .await
        .unwrap();
    let bob_pub_hex = json_body(resp).await["x25519_pub_hex"]
        .as_str()
        .unwrap()
        .to_string();
    let bob_pub_fetched: [u8; 32] = hex::decode(&bob_pub_hex).unwrap().try_into().unwrap();

    // Alice generates a channel content key and seals it to Bob.
    let cek = b"a-secret-32-byte-channel-key-!!!";
    let wrapped = seal_x25519(cek, &bob_pub_fetched).unwrap();
    let wrapped_b64 = base64::engine::general_purpose::STANDARD.encode(&wrapped);

    let resp = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/channels/secret-chan/key-wraps",
            "alice",
            Some(serde_json::json!({
                "epoch": 1,
                "wraps": [{ "recipient_pseudonym_id": "bob", "wrapped_key_b64": wrapped_b64 }]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The server stores ONLY ciphertext — the raw CEK must not be in the DB.
    {
        let conn = pool.get().unwrap();
        let stored: String = conn
            .query_row(
                "SELECT wrapped_key_b64 FROM channel_key_wraps WHERE recipient_pseudonym_id = 'bob'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let stored_bytes = base64::engine::general_purpose::STANDARD
            .decode(&stored)
            .unwrap();
        assert!(
            !stored_bytes.windows(cek.len()).any(|w| w == cek),
            "server stored the plaintext channel key!"
        );
    }

    // Bob fetches his wrap and opens it with his secret — recovering the CEK.
    let resp = app
        .oneshot(request(
            "GET",
            "/api/channels/secret-chan/key-wraps",
            "bob",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    let wraps = body["wraps"].as_array().unwrap();
    assert_eq!(wraps.len(), 1);
    let blob = base64::engine::general_purpose::STANDARD
        .decode(wraps[0]["wrapped_key_b64"].as_str().unwrap())
        .unwrap();
    let opened = open_x25519(&blob, &bob_secret).unwrap();
    assert_eq!(opened, cek, "Bob failed to recover the channel key");
}

#[tokio::test]
async fn wraps_are_only_visible_to_their_recipient() {
    let (app, pool) = common::setup_test_app().await;
    seed_channel(&pool, "chan");
    for p in ["alice", "bob", "carol"] {
        seed_identity(&pool, p, false);
        seed_member(&pool, "chan", p);
    }
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());

    app.clone()
        .oneshot(request(
            "POST",
            "/api/channels/chan/key-wraps",
            "alice",
            Some(serde_json::json!({
                "wraps": [
                    { "recipient_pseudonym_id": "bob", "wrapped_key_b64": b64("for-bob") },
                    { "recipient_pseudonym_id": "carol", "wrapped_key_b64": b64("for-carol") }
                ]
            })),
        ))
        .await
        .unwrap();

    let resp = app
        .oneshot(request("GET", "/api/channels/chan/key-wraps", "bob", None))
        .await
        .unwrap();
    let body = json_body(resp).await;
    let wraps = body["wraps"].as_array().unwrap();
    assert_eq!(wraps.len(), 1);
    assert_eq!(wraps[0]["sender_pseudonym_id"], "alice");
    // Bob sees his own wrap, never carol's.
    assert_eq!(wraps[0]["wrapped_key_b64"], b64("for-bob"));
}

#[tokio::test]
async fn wraps_for_non_members_are_dropped_and_not_counted() {
    let (app, pool) = common::setup_test_app().await;
    seed_channel(&pool, "chan");
    seed_identity(&pool, "alice", false);
    seed_member(&pool, "chan", "alice");
    seed_identity(&pool, "outsider", false); // exists, but NOT a channel member
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());

    // Alice (a member) wraps to an outsider — it must be silently dropped.
    let resp = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/channels/chan/key-wraps",
            "alice",
            Some(serde_json::json!({
                "wraps": [{ "recipient_pseudonym_id": "outsider", "wrapped_key_b64": b64("x") }]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["inserted"], 0);

    // And the channel must NOT look keyed, so real members can still provision.
    let resp = app
        .oneshot(request(
            "GET",
            "/api/channels/chan/key-status",
            "alice",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["has_key"], false);
}

#[tokio::test]
async fn first_wrap_wins_no_clobber() {
    let (app, pool) = common::setup_test_app().await;
    seed_channel(&pool, "chan");
    for p in ["alice", "bob"] {
        seed_identity(&pool, p, false);
        seed_member(&pool, "chan", p);
    }
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());

    let post = |val: &str| {
        request(
            "POST",
            "/api/channels/chan/key-wraps",
            "bob",
            Some(serde_json::json!({
                "wraps": [{ "recipient_pseudonym_id": "alice", "wrapped_key_b64": b64(val) }]
            })),
        )
    };

    app.clone().oneshot(post("original")).await.unwrap();
    // Second write to the same (channel, recipient, epoch) is ignored.
    app.clone().oneshot(post("attacker-clobber")).await.unwrap();

    let resp = app
        .oneshot(request(
            "GET",
            "/api/channels/chan/key-wraps",
            "alice",
            None,
        ))
        .await
        .unwrap();
    let body = json_body(resp).await;
    let wraps = body["wraps"].as_array().unwrap();
    assert_eq!(wraps.len(), 1);
    assert_eq!(wraps[0]["wrapped_key_b64"], b64("original"));
}

#[tokio::test]
async fn key_status_reflects_provisioning() {
    let (app, pool) = common::setup_test_app().await;
    seed_channel(&pool, "chan");
    for p in ["alice", "bob"] {
        seed_identity(&pool, p, false);
        seed_member(&pool, "chan", p);
    }
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());

    // Before any wrap, the channel has no key.
    let resp = app
        .clone()
        .oneshot(request("GET", "/api/channels/chan/key-status", "bob", None))
        .await
        .unwrap();
    let body = json_body(resp).await;
    assert_eq!(body["has_key"], false);
    assert_eq!(body["max_epoch"], 0);

    // Alice provisions an epoch-2 key.
    app.clone()
        .oneshot(request(
            "POST",
            "/api/channels/chan/key-wraps",
            "alice",
            Some(serde_json::json!({
                "epoch": 2,
                "wraps": [{ "recipient_pseudonym_id": "bob", "wrapped_key_b64": b64("k") }]
            })),
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(request("GET", "/api/channels/chan/key-status", "bob", None))
        .await
        .unwrap();
    let body = json_body(resp).await;
    assert_eq!(body["has_key"], true);
    assert_eq!(body["max_epoch"], 2);

    // Non-member cannot probe key status.
    seed_identity(&pool, "mallory", false);
    let resp = app
        .oneshot(request(
            "GET",
            "/api/channels/chan/key-status",
            "mallory",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn non_member_cannot_post_wraps() {
    let (app, pool) = common::setup_test_app().await;
    seed_channel(&pool, "chan");
    seed_identity(&pool, "alice", false);
    seed_member(&pool, "chan", "alice");
    seed_identity(&pool, "mallory", false); // not a member

    let resp = app
        .oneshot(request(
            "POST",
            "/api/channels/chan/key-wraps",
            "mallory",
            Some(serde_json::json!({
                "wraps": [{
                    "recipient_pseudonym_id": "alice",
                    "wrapped_key_b64": base64::engine::general_purpose::STANDARD.encode("x")
                }]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn e2e_flag_toggle_requires_moderation() {
    let (app, pool) = common::setup_test_app().await;
    seed_channel(&pool, "chan");
    seed_identity(&pool, "mod", true);
    seed_identity(&pool, "user", false);

    // Default is off.
    let resp = app
        .clone()
        .oneshot(request("GET", "/api/channels/chan/e2e", "user", None))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["e2e_enabled"], false);

    // Non-moderator cannot enable.
    let resp = app
        .clone()
        .oneshot(request(
            "PUT",
            "/api/channels/chan/e2e",
            "user",
            Some(serde_json::json!({ "enabled": true })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Moderator enables it.
    let resp = app
        .clone()
        .oneshot(request(
            "PUT",
            "/api/channels/chan/e2e",
            "mod",
            Some(serde_json::json!({ "enabled": true })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(request("GET", "/api/channels/chan/e2e", "user", None))
        .await
        .unwrap();
    assert_eq!(json_body(resp).await["e2e_enabled"], true);
}
