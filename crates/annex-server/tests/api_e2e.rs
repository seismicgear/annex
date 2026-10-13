//! HTTP checks for encryption-policy metadata and refusal of unsigned mode changes.
//! Genuine signed genesis/policy and sealed-key round trips are covered by the
//! channel_manifest/e2e_service tests and the live encryption session lane.

mod common;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
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
         VALUES (1, ?1, ?1, json_quote('Text'))",
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

#[tokio::test]
async fn unsigned_e2e_changes_are_refused_even_for_a_global_moderator() {
    let (app, pool) = common::setup_test_app().await;
    seed_channel(&pool, "chan");
    seed_identity(&pool, "mod", true);
    seed_identity(&pool, "user", false);
    seed_member(&pool, "chan", "user");
    seed_member(&pool, "chan", "mod");

    // Exercise ordinary plaintext and an existing encrypted channel. Global
    // administration must neither bypass signed enablement nor downgrade a
    // channel whose users have not signed that policy change.
    for was_enabled in [false, true] {
        pool.get()
            .unwrap()
            .execute(
                "UPDATE channels SET e2e_enabled=?1 WHERE server_id=1 AND channel_id='chan'",
                [was_enabled],
            )
            .unwrap();
        for pseudonym in ["user", "mod"] {
            let response = app
                .clone()
                .oneshot(request(
                    "PUT",
                    "/api/channels/chan/e2e",
                    pseudonym,
                    Some(serde_json::json!({"enabled": !was_enabled})),
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "unsigned change by {pseudonym} must not bypass the manifest"
            );
            let read = app
                .clone()
                .oneshot(request("GET", "/api/channels/chan/e2e", "user", None))
                .await
                .unwrap();
            assert_eq!(read.status(), StatusCode::OK);
            assert_eq!(json_body(read).await["e2e_enabled"], was_enabled);
            let stored: bool = pool
                .get()
                .unwrap()
                .query_row(
                    "SELECT e2e_enabled FROM channels WHERE server_id=1 AND channel_id='chan'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                stored, was_enabled,
                "refusal must preserve the channel policy"
            );
        }
    }
}

/// `GET /api/channels/{id}/e2e` was the one handler in this module with no
/// membership check.
///
/// Its five siblings all call `is_member` first, and `get_channel_handler`
/// states the rule the server means to keep: "Moderators can view any
/// channel; regular users must be members, preventing metadata leakage on
/// private channels." Whether a channel is end-to-end encrypted is exactly
/// that kind of metadata, and `list_channels` — which any authenticated user
/// can call — deliberately does not return it. This route did, for any
/// channel, to anyone with a pseudonym.
#[tokio::test]
async fn reading_the_e2e_flag_requires_membership_or_moderation() {
    let (app, pool) = common::setup_test_app().await;
    seed_channel(&pool, "private-chan");
    seed_identity(&pool, "member", false);
    seed_identity(&pool, "outsider", false);
    seed_identity(&pool, "mod", true);
    seed_member(&pool, "private-chan", "member");

    let resp = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/channels/private-chan/e2e",
            "outsider",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-member learned whether a channel they cannot see is encrypted",
    );

    // A member still gets the answer — the client calls this on every channel
    // open, after joining, so this is the path that must keep working.
    let resp = app
        .clone()
        .oneshot(request(
            "GET",
            "/api/channels/private-chan/e2e",
            "member",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["e2e_enabled"], false);

    // Moderators keep the same override `get_channel_handler` grants them,
    // so the two routes cannot disagree about who may see a channel.
    let resp = app
        .oneshot(request(
            "GET",
            "/api/channels/private-chan/e2e",
            "mod",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
