//! Smoke tests that start a real TCP server and make HTTP requests.
//!
//! These validate the full request path (TCP -> Axum -> handler -> response)
//! rather than using in-process `tower::ServiceExt::oneshot()`.

mod common;

use annex_server::app;
use std::net::SocketAddr;
use tokio::net::TcpListener;

/// Starts a real Axum server on an OS-assigned port.
/// Returns the base URL (e.g. "http://127.0.0.1:12345").
async fn start_server() -> String {
    let (_, pool) = common::setup_test_app().await;

    let tree = {
        let conn = pool.get().unwrap();
        annex_identity::MerkleTree::restore(&conn, 20).unwrap()
    };
    let state = common::build_app_state(pool, tree, annex_types::ServerPolicy::default());

    let router = app(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    format!("http://127.0.0.1:{}", addr.port())
}

#[tokio::test]
async fn smoke_health_endpoint() {
    let base = start_server().await;
    let resp = reqwest::get(format!("{}/health", base)).await.unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn smoke_registry_current_root() {
    let base = start_server().await;
    let resp = reqwest::get(format!("{}/api/registry/current-root", base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    // API uses camelCase field names
    assert!(body["rootHex"].is_string());
}

#[tokio::test]
async fn smoke_register_identity() {
    let base = start_server().await;
    let client = reqwest::Client::new();

    // Register a new identity with a small valid commitment (API uses camelCase)
    let commitment = "0000000000000000000000000000000000000000000000000000000000000042";
    let resp = client
        .post(format!("{}/api/registry/register", base))
        .json(&serde_json::json!({
            "commitmentHex": commitment,
            "roleCode": 1,
            "nodeId": 1
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["identityId"].is_number());
    assert!(body["leafIndex"].is_number());
    assert!(body["rootHex"].is_string());
}

#[tokio::test]
async fn smoke_protected_endpoint_requires_auth() {
    let base = start_server().await;
    let client = reqwest::Client::new();

    // Attempting to create a channel without a valid pseudonym should fail
    let resp = client
        .post(format!("{}/api/channels", base))
        .json(&serde_json::json!({
            "name": "test-channel",
            "channel_type": "Text"
        }))
        .send()
        .await
        .unwrap();

    // Should be 400 or 401 (missing pseudonym header)
    assert!(
        resp.status().is_client_error(),
        "expected client error, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn smoke_unknown_route_returns_401_or_404() {
    let base = start_server().await;
    let resp = reqwest::get(format!("{}/api/does-not-exist", base))
        .await
        .unwrap();
    // The server may return 401 (auth middleware) or 404 depending on route matching
    let status = resp.status().as_u16();
    assert!(
        status == 401 || status == 404,
        "expected 401 or 404, got {}",
        status
    );
}
