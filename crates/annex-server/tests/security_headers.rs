//! Document CSP follows the deployment's configured transport, never a caller's
//! Host/Forwarded headers. Actual browser fetch acceptance lives in the live lane.
mod common;

use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::middleware::security_headers_middleware;
use annex_types::ServerPolicy;
use axum::{body::Body, http::Request, response::Html, routing::get, Extension, Router};
use std::sync::{Arc, RwLock};
use tower::ServiceExt;

fn document_app() -> (Router, Arc<RwLock<String>>) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    run_migrations(&pool.get().unwrap()).unwrap();
    let state =
        common::build_app_state(pool, MerkleTree::new(20).unwrap(), ServerPolicy::default());
    let public_url = state.public_url.clone();
    let app = Router::new()
        .route(
            "/",
            get(|| async { Html("<!doctype html><title>Annex</title>") }),
        )
        .route(
            "/json",
            get(|| async { axum::Json(serde_json::json!({"ok":true})) }),
        )
        .layer(axum::middleware::from_fn(security_headers_middleware))
        .layer(Extension(Arc::new(state)));
    (app, public_url)
}

#[tokio::test]
async fn documents_allow_http_peers_only_for_an_explicit_http_deployment() {
    let (app, public_url) = document_app();
    for (configured, allow_http) in [
        ("http://127.0.0.1:3000", true),
        ("http://[::1]:3000", true),
        ("http://192.168.1.20:3000", true),
        ("https://annex.example", false),
        ("", false),
        ("not a URL", false),
        ("file:///private", false),
    ] {
        *public_url.write().unwrap() = configured.to_owned();
        let response = app
            .clone()
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let policy = response.headers()["content-security-policy"]
            .to_str()
            .unwrap();
        let connect = policy
            .split(';')
            .find(|part| part.trim().starts_with("connect-src"))
            .unwrap();
        assert_eq!(
            connect.split_whitespace().any(|source| source == "http:"),
            allow_http,
            "{configured}: {policy}"
        );
        assert!(connect.contains("https:") && connect.contains("wss:"));
        assert!(policy.contains("script-src 'self' 'wasm-unsafe-eval'"));
        assert!(policy.contains("frame-ancestors 'none'"));
        assert!(!policy.contains("script-src 'self' 'unsafe-eval'"));
    }
}

#[tokio::test]
async fn caller_headers_cannot_downgrade_https_or_unconfigured_documents() {
    let (app, public_url) = document_app();
    for configured in ["https://annex.example", ""] {
        *public_url.write().unwrap() = configured.to_owned();
        let response = app
            .clone()
            .oneshot(
                Request::get("/")
                    .header("host", "localhost:3000")
                    .header("x-forwarded-proto", "http")
                    .header("x-forwarded-host", "peer.example")
                    .header("forwarded", "proto=http;host=peer.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(!response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains(" http:"));
    }
}

#[tokio::test]
async fn api_responses_keep_universal_headers_without_document_permissions() {
    let (app, public_url) = document_app();
    *public_url.write().unwrap() = "http://127.0.0.1:3000".to_owned();
    let response = app
        .oneshot(Request::get("/json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert!(!response.headers().contains_key("content-security-policy"));
    assert!(!response.headers().contains_key("permissions-policy"));
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(response.headers()["x-frame-options"], "DENY");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
}
