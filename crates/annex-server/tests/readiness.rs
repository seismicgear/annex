//! `/readyz` must be able to say no.
//!
//! `/health` returned `{"status":"ok"}` against a dead database, because it
//! returned a literal. The whole value of a readiness probe is the case where
//! it reports a problem, so that is what these test — a probe that has only
//! ever been observed saying yes is a probe nobody has tested.

mod common;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use std::net::SocketAddr;
use tower::ServiceExt;

/// A probe GET carrying a peer address.
///
/// The probe routes sit behind the same per-IP middleware as the rest of the
/// public surface, and that middleware answers **500** when it can find no
/// key at all. In production the address always arrives — `main.rs` serves
/// with `into_make_service_with_connect_info` — but `oneshot` supplies no
/// extensions, so a probe test without this reads 500 for a reason that has
/// nothing to do with readiness. Supplying it makes these tests exercise the
/// same request shape an orchestrator sends.
fn probe(uri: &str) -> Request<Body> {
    let mut req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:40000".parse::<SocketAddr>().unwrap(),
    ));
    req
}

/// Read a JSON body out of a response.
async fn json_body(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .expect("body should read");
    serde_json::from_slice(&bytes).expect("body should be JSON")
}

#[tokio::test]
async fn readyz_reports_ready_on_a_healthy_app() {
    let (app, _pool, _storage) = common::setup_test_app_with_storage_health().await;

    let res = app
        .oneshot(probe("/readyz"))
        .await
        .expect("request should complete");

    assert_eq!(res.status(), StatusCode::OK);
    let body = json_body(res).await;
    assert_eq!(body["status"], "ready");

    // Every check must be named in the body. A probe that reports a single
    // boolean tells an operator that something is wrong and not what.
    let names: Vec<&str> = body["checks"]
        .as_array()
        .expect("checks should be an array")
        .iter()
        .map(|c| c["name"].as_str().expect("name should be a string"))
        .collect();
    for expected in ["database", "storage", "merkle"] {
        assert!(
            names.contains(&expected),
            "readiness body should name the {expected} check, got {names:?}"
        );
    }
}

#[tokio::test]
async fn readyz_reports_degraded_when_the_storage_gate_is_blocking_writes() {
    let (app, _pool, storage) = common::setup_test_app_with_storage_health().await;

    // The condition an operator wants paged on: the server answers reads and
    // refuses every mutation with 507. Alive, and not able to do its job.
    storage.mark_degraded("test: simulated full disk");

    let res = app
        .oneshot(probe("/readyz"))
        .await
        .expect("request should complete");

    assert_eq!(
        res.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "a server refusing writes is not ready to serve traffic"
    );
    let body = json_body(res).await;
    assert_eq!(body["status"], "degraded");

    let storage = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "storage")
        .expect("the storage check should be reported");
    assert_eq!(storage["ok"], false);
}

/// The readiness body is public. It may say WHICH check failed and must not
/// say what the underlying error was: a `rusqlite` message carries the
/// database file path, and this endpoint has no authentication by design.
#[tokio::test]
async fn readyz_does_not_leak_internals() {
    let (app, _pool, storage) = common::setup_test_app_with_storage_health().await;
    storage.mark_degraded("/srv/annex/data/annex.db is full");

    let res = app
        .oneshot(probe("/readyz"))
        .await
        .expect("request should complete");
    let body = json_body(res).await;
    let text = body.to_string();

    assert!(
        !text.contains("/srv/annex"),
        "readiness must not echo the storage reason verbatim: {text}"
    );
}

/// `/health` is polled by `scripts/e2e-server.sh`, `client/e2e/startup.spec.ts`
/// and the puppeteer harness. Splitting liveness from readiness must not have
/// changed what it says.
#[tokio::test]
async fn health_keeps_its_shape_and_stays_cheap() {
    let (app, _pool, storage) = common::setup_test_app_with_storage_health().await;
    storage.mark_degraded("test: simulated full disk");

    let res = app
        .oneshot(probe("/health"))
        .await
        .expect("request should complete");

    assert_eq!(
        res.status(),
        StatusCode::OK,
        "liveness must not fail because the disk is full — restarting the process does not add disk"
    );
    let body = json_body(res).await;
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string());
    assert!(body["voice_enabled"].is_boolean());
}

#[tokio::test]
async fn livez_is_an_alias_for_health() {
    let (app, _pool, _storage) = common::setup_test_app_with_storage_health().await;

    let res = app
        .oneshot(probe("/livez"))
        .await
        .expect("request should complete");

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(json_body(res).await["status"], "ok");
}

/// A busy public surface must not be able to starve the liveness probe.
///
/// Both go through the same per-IP middleware, so before the probe routes had
/// their own counter, 60 anonymous requests in a window spent the budget and
/// the next `/livez` came back 429 — which an orchestrator reads as a dead
/// container and restarts. The failure mode is specifically "under load", so
/// it would never show up in a quiet test or a quiet staging environment.
#[tokio::test]
async fn public_traffic_cannot_starve_the_liveness_probe() {
    let (app, _pool, _storage) = common::setup_test_app_with_storage_health().await;

    // `default_limit` is 60 per window. Spend well past it on a public route
    // from the same address the probe will use.
    let mut saw_429 = false;
    for _ in 0..80 {
        let status = app
            .clone()
            .oneshot(probe("/api/registry/topics"))
            .await
            .expect("request should complete")
            .status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            saw_429 = true;
        }
    }
    assert!(
        saw_429,
        "the public bucket should have been exhausted — if it was not, this test \
         is no longer exercising the condition it exists for"
    );

    let res = app
        .oneshot(probe("/livez"))
        .await
        .expect("request should complete");
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "liveness must answer while the public bucket is exhausted"
    );
}
