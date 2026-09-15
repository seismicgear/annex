//! Global tower/axum layers applied to the fully-merged router.
//!
//! The order below is load-bearing — outer layers run first on the way in
//! and last on the way out. From outermost to innermost:
//!
//! 1. `Extension(Arc<AppState>)` — handlers can extract shared state.
//! 2. Request tracing — assigns the request id, so everything beneath it logs
//!    under that id. A panic logged without one cannot be tied to the request
//!    that caused it.
//! 3. CORS — applied before our middleware so preflight (`OPTIONS`) responses
//!    are answered with CORS headers without auth/rate-limit interference.
//! 4. Security-headers middleware.
//! 5. Panic catching — converts a handler panic into a 500 instead of a
//!    dropped connection. INSIDE security-headers on purpose: it synthesises
//!    a response, and a synthesised response is only decorated by the layers
//!    OUTSIDE it. Wired outside, its 500 shipped with none of the security
//!    headers every other response carries.
//! 6. Request timeout — bounds how long any one handler can hold a connection.
//! 7. Body-size limit (`MAX_REQUEST_BODY_BYTES`).
//!
//! Rate limiting is NOT in the global chain — it lives per-route group so
//! it can run AFTER per-route auth and key by pseudonym for authenticated
//! requests. See `crate::routes::app` for the per-route composition. Doing
//! it globally would force IP-only keying for everyone, because the global
//! layer is upstream of any per-route auth middleware.
//!
//! Security-headers runs after CORS so it only sees real, same-origin /
//! approved cross-origin requests.

use std::sync::Arc;
use std::time::Duration;

use axum::{extract::DefaultBodyLimit, http::StatusCode, response::Response, Extension, Router};
use tower_http::{catch_panic::CatchPanicLayer, cors::CorsLayer, timeout::TimeoutLayer};

use crate::http::observability;
use crate::middleware;
use crate::state::AppState;

/// Maximum request body size (2 MiB). Protects against OOM from oversized payloads.
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Ceiling on how long a single HTTP request may take.
///
/// Generous, because it is a backstop rather than a policy: the slowest
/// legitimate requests here are ZK verification and a federation round trip,
/// both of which are seconds, not minutes. What it actually bounds is a
/// handler that never returns — a lock never released, a peer that accepts a
/// connection and then says nothing — which otherwise holds its connection
/// (and its pool handle) until the process restarts.
///
/// NOT applied to `/ws`: a WebSocket upgrade is meant to last for hours. The
/// timeout layer sits in the HTTP chain, and `/ws` is mounted on its own
/// router outside it.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Handler panics, counted where the renderer can reach them.
///
/// `CatchPanicLayer::custom` takes a plain `fn`, with no captured state and no
/// request extensions, so there is no route from the renderer to the
/// `AppState` that holds every other counter. A process-global is the honest
/// answer for a process-global event; `GET /metrics` folds it in.
pub(crate) static PANIC_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Render a panic that escaped a handler as a 500.
///
/// Without this layer a panic unwinds into hyper, which drops the connection
/// with no response at all. The client sees a transport error rather than an
/// HTTP status — indistinguishable from a network fault — and the only record
/// is a bare panic line with nothing tying it to a request.
///
/// The body is deliberately opaque. The panic message can name file paths,
/// SQL fragments, or values from the request, and the caller is the last
/// person who should receive those; the detail goes to the log, under the
/// request id that the tracing layer above has already put in scope.
fn render_panic(
    err: Box<dyn std::any::Any + Send + 'static>,
) -> Response<http_body_util::Full<axum::body::Bytes>> {
    let detail = if let Some(s) = err.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = err.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "non-string panic payload".to_string()
    };
    tracing::error!(panic = %detail, "handler panicked; returning 500");
    PANIC_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(http_body_util::Full::from(
            r#"{"error":"internal server error"}"#,
        ))
        .expect("a static response builds")
}

/// Wraps the router with the global layer chain (body limit, security
/// headers, CORS, shared state extension). Rate limiting is intentionally
/// applied per-route group (see `crate::routes::app`) so it can sit
/// downstream of authentication and key by pseudonym for protected
/// routes.
pub(crate) fn apply_global_layers(
    router: Router,
    shared_state: Arc<AppState>,
    cors_layer: CorsLayer,
) -> Router {
    apply_resilience_layers(router.layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES)))
        .layer(axum::middleware::from_fn(
            middleware::security_headers_middleware,
        ))
        .layer(cors_layer)
        .layer(axum::middleware::from_fn(
            observability::request_trace_middleware,
        ))
        .layer(Extension(shared_state))
}

/// Panic catching and a request timeout — the two layers that SYNTHESISE a
/// response rather than passing one through.
///
/// Split out from [`apply_global_layers`] because neither depends on
/// `AppState`, which makes them directly testable: fabricating an `AppState`
/// for a unit test means filling thirty fields that have nothing to do with
/// whether a panic becomes a 500.
///
/// **These sit INSIDE the security-headers middleware, and that is the whole
/// point of the split.** A layer that synthesises a response produces one the
/// layers outside it still decorate, and one the layers inside it never touch.
/// With catch-panic on the outside — where it was first wired — a panic
/// unwound straight past the security-headers middleware, so the 500 it
/// rendered shipped with no `nosniff`, no `X-Frame-Options` and no
/// `Referrer-Policy`, while every other response on the server carried all
/// three. `a_handler_panic_is_rendered_as_500_and_not_a_dropped_connection`
/// caught it: the status and the request id were already right, and the
/// headers were simply absent.
///
/// The timeout is innermost so a timed-out request is still decorated, still
/// traced, and still counted.
pub(crate) fn apply_resilience_layers(router: Router) -> Router {
    router
        .layer(TimeoutLayer::with_status_code(
            StatusCode::SERVICE_UNAVAILABLE,
            REQUEST_TIMEOUT,
        ))
        .layer(CatchPanicLayer::custom(render_panic))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, routing::get, Router};
    use tower::ServiceExt;

    /// The REAL chain, minus only the `Extension(AppState)` layer.
    ///
    /// These live here rather than in `tests/` because `Router::layer` wraps
    /// only the routes already present — a route grafted onto a finished
    /// `app(state)` sits OUTSIDE the chain entirely. An integration test that
    /// added a panicking route that way exercises no layer at all, and reports
    /// the missing request id as a middleware failure rather than as its own
    /// construction error. (The first version of this test did exactly that.)
    fn chained(router: Router) -> Router {
        apply_resilience_layers(router)
            .layer(axum::middleware::from_fn(
                middleware::security_headers_middleware,
            ))
            .layer(axum::middleware::from_fn(
                observability::request_trace_middleware,
            ))
    }

    #[tokio::test]
    async fn a_handler_panic_is_rendered_as_500_and_not_a_dropped_connection() {
        let app = chained(Router::new().route(
            "/boom",
            get(|| async {
                panic!("deliberate panic: secret-looking detail");
                #[allow(unreachable_code)]
                ""
            }),
        ));

        let response = app
            .oneshot(Request::builder().uri("/boom").body(Body::empty()).unwrap())
            .await
            .expect("the panic must not take the connection with it");

        assert_eq!(
            response.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "a panic must become a status the caller can read; a dropped \
             connection is indistinguishable from a network fault"
        );
        assert!(
            response.headers().get("x-request-id").is_some(),
            "the panic response needs an id, or the 500 a user reports cannot \
             be matched to the backtrace in the log"
        );
        assert_eq!(
            response
                .headers()
                .get("x-content-type-options")
                .map(|v| v.to_str().unwrap()),
            Some("nosniff"),
            "the panic path skipped the security-headers middleware — the panic \
             layer is wired outside it"
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("secret-looking detail"),
            "the panic payload reached the caller: {text}"
        );
    }

    #[tokio::test]
    async fn the_panic_counter_advances() {
        let before = PANIC_COUNT.load(std::sync::atomic::Ordering::Relaxed);
        let app = chained(Router::new().route(
            "/boom",
            get(|| async {
                panic!("counted");
                #[allow(unreachable_code)]
                ""
            }),
        ));
        let _ = app
            .oneshot(Request::builder().uri("/boom").body(Body::empty()).unwrap())
            .await
            .unwrap();
        // `>=`, not `== before + 1`. `PANIC_COUNT` is process-global and the
        // test harness runs these threaded, so the sibling panic test can land
        // between the two reads. Asserting an exact delta here made this fail
        // intermittently against a counter that was working perfectly — the
        // classic shared-global test bug, and worth not re-introducing for the
        // sake of a tighter-looking assertion.
        let after = PANIC_COUNT.load(std::sync::atomic::Ordering::Relaxed);
        assert!(
            after >= before + 1,
            "a caught panic must be counted apart from ordinary 500s — it is a \
             defect, not load, and should page someone on its own \
             (before={before}, after={after})"
        );
    }

    #[tokio::test]
    async fn an_ordinary_response_carries_a_generated_request_id() {
        let app = chained(Router::new().route("/ok", get(|| async { "fine" })));
        let response = app
            .oneshot(Request::builder().uri("/ok").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let id = response
            .headers()
            .get("x-request-id")
            .expect("an id")
            .to_str()
            .unwrap();
        assert_eq!(id.len(), 36, "expected a generated UUID, got {id:?}");
    }

    #[tokio::test]
    async fn a_client_supplied_request_id_is_echoed_and_a_hostile_one_is_not() {
        for (supplied, echoed_verbatim) in [
            ("upstream-proxy-42", true),
            ("a.b_c-1", true),
            ("has space", false),
            ("semi;colon", false),
            ("quote\"inject", false),
        ] {
            let app = chained(Router::new().route("/ok", get(|| async { "fine" })));
            let Ok(request) = Request::builder()
                .uri("/ok")
                .header("x-request-id", supplied)
                .body(Body::empty())
            else {
                // hyper refuses some of these at the header-value level, which
                // is itself a rejection.
                continue;
            };
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "a bad id must not fail the request — the request is fine, its label is not"
            );
            let got = response
                .headers()
                .get("x-request-id")
                .expect("still gets an id")
                .to_str()
                .unwrap();
            if echoed_verbatim {
                assert_eq!(got, supplied, "a proxy's id must survive for correlation");
            } else {
                assert_ne!(
                    got, supplied,
                    "{supplied:?} was echoed into the response and the logs verbatim"
                );
                assert_eq!(got.len(), 36, "should be a freshly generated UUID");
            }
        }
    }

    /// An id long enough to bloat every log line for that request is replaced.
    #[tokio::test]
    async fn an_overlong_request_id_is_replaced() {
        let long = "x".repeat(MAX_CLIENT_REQUEST_ID_FOR_TEST + 1);
        let app = chained(Router::new().route("/ok", get(|| async { "fine" })));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ok")
                    .header("x-request-id", &long)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let got = response
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap();
        assert_ne!(got, long);
        assert_eq!(got.len(), 36);
    }

    const MAX_CLIENT_REQUEST_ID_FOR_TEST: usize = 64;
}
