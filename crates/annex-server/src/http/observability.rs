//! Per-request observability: a request id, a span, a latency, an outcome.
//!
//! The server had none of this. `tower-http` was compiled with its `trace`
//! feature enabled and `TraceLayer` was never installed, so the only thing an
//! operator saw was whatever a handler chose to log on its own. A 500 with no
//! `tracing::error!` beside it — which is most of them, because `ApiError`
//! renders itself and returns — left no record that the request had happened
//! at all. "A user says saving failed at about 3pm" was not an answerable
//! question.
//!
//! Three pieces, and the middle one is why the other two are useful:
//!
//! * **A request id** on every request, echoed in the `x-request-id` response
//!   header. Generated here when the client did not supply one, so a user can
//!   read it out of their browser's network tab and an operator can find the
//!   exact request.
//! * **A span carrying that id**, wrapped around the request, so every
//!   `tracing` event a handler emits — including ones written long before
//!   this existed — is automatically attributed to it. This is what turns
//!   scattered log lines into a request.
//! * **Latency and status** recorded on the way out, at a level chosen by the
//!   outcome: 5xx is an error, 4xx a warning, everything else debug. A log at
//!   a fixed level is either noise or silence depending on traffic.

use std::time::Instant;

use axum::{
    extract::Request,
    http::{header::HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

use tracing::Instrument;
use uuid::Uuid;

use crate::state::AppState;

/// The header carried in and out.
///
/// Lower-case because HTTP/2 requires it; a mixed-case name is rejected
/// outright by some proxies rather than normalised.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Longest client-supplied request id we will echo.
///
/// The id is accepted from the caller so a reverse proxy or an upstream
/// service can correlate its logs with ours — but an unbounded, unvalidated
/// string from a stranger ends up in every log line for that request, which is
/// a log-injection and a log-volume problem at once. Anything longer, or
/// carrying anything outside the ASCII set below, is REPLACED rather than
/// rejected: the request is fine, its label is not.
const MAX_CLIENT_REQUEST_ID: usize = 64;

fn sanitized_client_id(req: &Request) -> Option<String> {
    let raw = req.headers().get(REQUEST_ID_HEADER)?.to_str().ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_CLIENT_REQUEST_ID {
        return None;
    }
    // No control characters, no newlines, nothing that could forge a second
    // log line. Conservative on purpose: this string is echoed and logged.
    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        return None;
    }
    Some(trimmed.to_string())
}

/// Assign a request id, span the request with it, and record how it ended.
pub async fn request_trace_middleware(mut req: Request, next: Next) -> Response {
    let request_id = sanitized_client_id(&req).unwrap_or_else(|| Uuid::new_v4().to_string());
    let header_name = HeaderName::from_static(REQUEST_ID_HEADER);

    // Put it back on the request so anything reading headers downstream sees
    // the same id this middleware will log and echo.
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        req.headers_mut().insert(header_name.clone(), value);
    }

    let method = req.method().clone();
    // The PATH only, never the query string. Query strings here carry
    // WebSocket tokens (`/ws?token=…`) and invite codes, and a log file is
    // exactly where those must not end up.
    let path = req.uri().path().to_string();

    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        method = %method,
        path = %path,
    );

    // `.instrument(span)`, NOT `let _g = span.enter()`.
    //
    // A span guard held across an `.await` stays entered while the task is
    // suspended, so every other task polled on that thread in the meantime is
    // attributed to THIS request. On a busy server that does not merely add
    // noise — it puts one user's request id on another user's log lines, which
    // is worse than having no request id at all.
    // Counters live on AppState, which the Extension layer beneath this one
    // installs. Pulled out before the request is consumed. `None` only when
    // this middleware is composed without that layer, which no production
    // path does — the metrics are skipped rather than panicking, because a
    // missing counter must never cost a request.
    let metrics = req
        .extensions()
        .get::<Arc<AppState>>()
        .map(|s| s.metrics.clone());

    let started = Instant::now();
    let mut response = async move {
        let response = next.run(req).await;
        let latency_ms = started.elapsed().as_millis();
        let status = response.status();
        if let Some(metrics) = &metrics {
            metrics.record(status);
        }

        // The level is chosen by what happened rather than fixed. At a fixed
        // level this is either noise on every health probe or silence on
        // every failure.
        if status.is_server_error() {
            tracing::error!(status = status.as_u16(), latency_ms, "request failed");
        } else if status.is_client_error() {
            tracing::warn!(status = status.as_u16(), latency_ms, "request refused");
        } else {
            tracing::debug!(status = status.as_u16(), latency_ms, "request completed");
        }
        response
    }
    .instrument(span)
    .await;

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(header_name, value);
    }
    response
}
