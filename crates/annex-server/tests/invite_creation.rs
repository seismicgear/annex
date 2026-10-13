//! HTTP and transaction boundaries for public invite creation.
mod common;
use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::{create_platform_identity, MerkleTree, PlatformIdentity};
use annex_server::{
    api_invite::{create_invite_handler, CreateInviteRequest},
    middleware::IdentityContext,
    AppState,
};
use axum::{
    body::{to_bytes, Body},
    extract::{ConnectInfo, Extension},
    http::{HeaderMap, Request, StatusCode},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::{net::SocketAddr, sync::Arc};
use tower::ServiceExt;

fn fixture() -> (Arc<AppState>, PlatformIdentity, String) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let identity = {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute("INSERT INTO servers (slug,label,policy_json) VALUES ('invite-test','Invite Test','{}')", []).unwrap();
        create_platform_identity(&conn, 1, "invite-owner", annex_types::RoleCode::Human).unwrap()
    };
    let mut state = common::build_app_state(pool, MerkleTree::new(20).unwrap(), Default::default());
    state.enforce_zk_proofs = true;
    *state.public_url.write().unwrap() = "https://annex.example.test:8443".into();
    let token = annex_server::api_ws::generate_session_token(
        &identity.pseudonym_id,
        &state.ws_token_secret,
        600,
        0,
    );
    (Arc::new(state), identity, token)
}

fn count(state: &AppState) -> i64 {
    state
        .pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM invite_codes", [], |r| r.get(0))
        .unwrap()
}

async fn request(
    state: &AppState,
    token: &str,
    payload: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/invites")
        .header("Authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:41000".parse::<SocketAddr>().unwrap(),
    ));
    let response = annex_server::app(state.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 100_000).await.unwrap();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::json!({"error":String::from_utf8_lossy(&bytes)}));
    (status, body)
}

#[tokio::test]
async fn invalid_public_destinations_never_persist_an_invite() {
    let (state, _, token) = fixture();
    for destination in [
        "",
        "http://annex.example.test",
        "https://user:secret@annex.example.test",
        "https://annex.example.test/path",
        "https://annex.example.test?target=elsewhere",
        "https://annex.example.test#target",
        "https://127.0.0.1",
        "https://0x7f000001",
        "https://10.1.2.3",
        "https://[::ffff:127.0.0.1]",
        "https://[fd00::1]",
        "https://localhost",
        "https://localhost.",
        "https://subdomain.localhost.",
    ] {
        *state.public_url.write().unwrap() = destination.into();
        let (status, body) = request(&state, &token, serde_json::json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{destination}: {body}");
        assert!(
            !body.to_string().contains("user:secret"),
            "credentials must not be reflected"
        );
        assert_eq!(count(&state), 0, "a rejected URL left an orphaned invite");
    }
}

#[tokio::test]
async fn invalid_limits_and_preview_metadata_roll_back_without_panicking() {
    let (state, _, token) = fixture();
    for payload in [
        serde_json::json!({"maxUses":0}),
        serde_json::json!({"maxUses":-1}),
        serde_json::json!({"expiresInHours":0}),
        serde_json::json!({"expiresInHours":-1}),
        serde_json::json!({"expiresInHours":i64::MAX}),
    ] {
        assert_eq!(
            request(&state, &token, payload).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(count(&state), 0);
    }
    state
        .pool
        .get()
        .unwrap()
        .execute("UPDATE servers SET description=?1", ["x".repeat(301)])
        .unwrap();
    assert_eq!(
        request(&state, &token, serde_json::json!({})).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(count(&state), 0);
}

#[tokio::test]
async fn created_url_contains_the_exact_committed_code_and_destination() {
    let (state, identity, token) = fixture();
    let (status, body) = request(
        &state,
        &token,
        serde_json::json!({"maxUses":1,"expiresInHours":2}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let encoded = body["url"]
        .as_str()
        .unwrap()
        .strip_prefix("https://monolithannex.com/invite/")
        .unwrap();
    let decoded: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
    assert_eq!(decoded["server"], "https://annex.example.test:8443");
    assert_eq!(decoded["code"], body["code"]);
    let stored: (String, i64, i64, String) = state
        .pool
        .get()
        .unwrap()
        .query_row(
            "SELECT created_by,max_uses,use_count,expires_at FROM invite_codes WHERE code=?1",
            [body["code"].as_str().unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        stored,
        (
            identity.pseudonym_id,
            1,
            0,
            body["expiresAt"].as_str().unwrap().to_owned()
        )
    );
    assert_eq!(count(&state), 1);
}

#[tokio::test]
async fn context_captured_before_revocation_cannot_create_after_it_commits() {
    for (mutation, expected) in [
        (
            "UPDATE platform_identities SET can_invite=0",
            StatusCode::FORBIDDEN,
        ),
        (
            "UPDATE platform_identities SET active=0",
            StatusCode::UNAUTHORIZED,
        ),
        (
            "UPDATE platform_identities SET token_epoch=token_epoch+1",
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        let (state, captured_identity, _) = fixture();
        state.pool.get().unwrap().execute(mutation, []).unwrap();
        let result = create_invite_handler(
            Extension(state.clone()),
            Extension(IdentityContext(captured_identity)),
            HeaderMap::new(),
            Json(CreateInviteRequest {
                max_uses: None,
                expires_in_hours: None,
            }),
        )
        .await;
        use axum::response::IntoResponse;
        let response = match result {
            Ok(_) => panic!("stale authority created invite"),
            Err(error) => error.into_response(),
        };
        assert_eq!(response.status(), expected);
        assert_eq!(count(&state), 0);
    }
}

#[tokio::test]
async fn unconfigured_origin_can_derive_a_valid_link_without_rewriting_server_configuration() {
    let (state, identity, _) = fixture();
    *state.public_url.write().unwrap() = String::new();
    let mut headers = HeaderMap::new();
    headers.insert("host", "annex.example.test:8443".parse().unwrap());
    let result = create_invite_handler(
        Extension(state.clone()),
        Extension(IdentityContext(identity)),
        headers,
        Json(CreateInviteRequest {
            max_uses: None,
            expires_in_hours: None,
        }),
    )
    .await
    .unwrap();
    assert!(result.url.starts_with("https://monolithannex.com/invite/"));
    assert_eq!(count(&state), 1);
    assert!(state.get_public_url().is_empty());
}
