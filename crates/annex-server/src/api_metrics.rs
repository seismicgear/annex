//! `GET /metrics` — Prometheus text exposition.
//!
//! There was no metrics surface at all. An operator could see that the process
//! was alive (`/livez`) and that its dependencies answered (`/readyz`), and
//! nothing about whether it was healthy: not how many sockets were open, not
//! how full the connection pool was, not whether the federation outbox was
//! draining or growing. Those are the numbers that say "this will fall over in
//! twenty minutes", and they were unavailable by any means short of attaching
//! a debugger.
//!
//! ## Hand-rolled rather than a metrics crate
//!
//! `metrics` + `metrics-exporter-prometheus` is the conventional answer and it
//! is a large dependency tree for what this needs. Everything below is either
//! a value `AppState` already holds or a counter incremented in one place in
//! `http::observability`. The exposition format is a documented, stable text
//! format — four lines per metric — and writing it directly keeps the
//! dependency budget of a sovereign, self-hosted binary where it belongs.
//!
//! The cost is honest: no histograms. Latency quantiles need reservoir
//! sampling or bucketing, and a bad hand-rolled histogram is worse than none,
//! so latency is logged per request (with a request id) rather than
//! summarised here. If quantiles become the question, take the dependency
//! then.
//!
//! ## Why it is not public
//!
//! These numbers describe the deployment: member counts, storage consumption,
//! federation peer activity. That is reconnaissance for anyone deciding
//! whether a server is worth attacking, and on a small server the member count
//! is close to personally identifying. So `/metrics` sits inside the
//! authenticated, moderator-gated group by default. An operator whose Prometheus
//! cannot present a token — the usual case, scraping over a private network —
//! sets `ANNEX_METRICS_PUBLIC=1`, which is a deliberate statement that the
//! endpoint is already behind a network boundary.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};

use crate::middleware::IdentityContext;

/// Process-wide request counters.
///
/// Relaxed ordering throughout: these are counters read by a scrape, not
/// synchronisation. A scrape landing between two increments may see a total
/// that is one behind a class sum, which matters to nobody; making them
/// `SeqCst` would put a fence on every request to fix a discrepancy no
/// operator can observe.
#[derive(Debug, Default)]
pub struct RequestMetrics {
    pub total: AtomicU64,
    pub success: AtomicU64,
    pub client_error: AtomicU64,
    pub server_error: AtomicU64,
    pub panics: AtomicU64,
}

impl RequestMetrics {
    pub fn record(&self, status: StatusCode) {
        self.total.fetch_add(1, Ordering::Relaxed);
        if status.is_server_error() {
            &self.server_error
        } else if status.is_client_error() {
            &self.client_error
        } else {
            &self.success
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_panic(&self) {
        self.panics.fetch_add(1, Ordering::Relaxed);
    }
}

/// Whether `/metrics` may be served without authentication.
pub fn metrics_is_public() -> bool {
    matches!(
        std::env::var("ANNEX_METRICS_PUBLIC").as_deref(),
        Ok("1") | Ok("true")
    )
}

fn gauge(out: &mut String, name: &str, help: &str, value: impl std::fmt::Display) {
    metric(out, name, help, "gauge", value);
}

fn counter(out: &mut String, name: &str, help: &str, value: impl std::fmt::Display) {
    metric(out, name, help, "counter", value);
}

fn metric(out: &mut String, name: &str, help: &str, kind: &str, value: impl std::fmt::Display) {
    use std::fmt::Write;
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
    let _ = writeln!(out, "{name} {value}");
}

/// Moderator-gated scrape. Mounted in the authenticated group.
pub async fn metrics(
    Extension(state): Extension<Arc<crate::AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Response {
    if !identity.can_moderate && !metrics_is_public() {
        return (
            StatusCode::FORBIDDEN,
            "metrics require moderator capability; set ANNEX_METRICS_PUBLIC=1 to scrape \
             anonymously from inside a trusted network\n",
        )
            .into_response();
    }
    render(&state)
}

/// Unauthenticated scrape, mounted only when `ANNEX_METRICS_PUBLIC` is set.
///
/// A separate handler rather than an `Option<IdentityContext>` on the one
/// above: the public route is not behind `auth_middleware`, so the extension
/// is not merely absent, it is never installed, and an extractor that treats
/// "no identity" as "not a moderator" would 403 every scrape on the very
/// route the operator opted into.
pub async fn metrics_public(Extension(state): Extension<Arc<crate::AppState>>) -> Response {
    if !metrics_is_public() {
        return StatusCode::NOT_FOUND.into_response();
    }
    render(&state)
}

fn render(state: &Arc<crate::AppState>) -> Response {
    let mut out = String::with_capacity(2048);

    // Build identity. One series with a constant value of 1 and the detail in
    // labels is the conventional shape, and it lets a dashboard show which
    // version is running without a second source of truth.
    out.push_str("# HELP annex_build_info Build metadata; always 1.\n");
    out.push_str("# TYPE annex_build_info gauge\n");
    out.push_str(&format!(
        "annex_build_info{{version=\"{}\",profile=\"{}\"}} 1\n",
        env!("CARGO_PKG_VERSION"),
        crate::build_profile::current().as_str(),
    ));

    let m = &state.metrics;
    counter(
        &mut out,
        "annex_http_requests_total",
        "HTTP requests that reached the router.",
        m.total.load(Ordering::Relaxed),
    );
    counter(
        &mut out,
        "annex_http_responses_success_total",
        "Responses with a 1xx/2xx/3xx status.",
        m.success.load(Ordering::Relaxed),
    );
    counter(
        &mut out,
        "annex_http_responses_client_error_total",
        "Responses with a 4xx status.",
        m.client_error.load(Ordering::Relaxed),
    );
    counter(
        &mut out,
        "annex_http_responses_server_error_total",
        "Responses with a 5xx status.",
        m.server_error.load(Ordering::Relaxed),
    );
    // Separate from the 5xx count on purpose. A panic that the catch-panic
    // layer rendered as a 500 is a defect, not load; it should page someone
    // even when the 5xx rate looks unremarkable.
    counter(
        &mut out,
        "annex_http_handler_panics_total",
        "Handler panics caught and rendered as 500.",
        crate::http::layers::PANIC_COUNT.load(Ordering::Relaxed),
    );

    // WebSockets. `open` counts admitted SOCKETS and `sessions` counts
    // registered identities; they differ while a displaced socket is closing,
    // and a persistent gap between them is the signature of sockets not being
    // released.
    gauge(
        &mut out,
        "annex_ws_connections_open",
        "WebSocket connections admitted and not yet released.",
        state.connection_manager.open_connections(),
    );
    gauge(
        &mut out,
        "annex_ws_connections_max",
        "Ceiling on concurrent WebSocket connections.",
        crate::api_ws::MAX_WS_CONNECTIONS,
    );

    // Connection pool. `idle` approaching zero while `size` is at its maximum
    // is the shape of pool exhaustion, which presents to users as unexplained
    // slowness rather than as an error.
    let pool = state.pool.state();
    gauge(
        &mut out,
        "annex_db_pool_connections",
        "Connections currently managed by the pool.",
        pool.connections,
    );
    gauge(
        &mut out,
        "annex_db_pool_idle_connections",
        "Idle connections available for checkout.",
        pool.idle_connections,
    );

    // Storage budget. The probe worker writes this; exposing the gate here
    // means an operator sees the wall coming rather than discovering it when
    // writes start being refused.
    gauge(
        &mut out,
        "annex_storage_max_bytes",
        "Configured database size budget, 0 when unlimited.",
        state.storage_config.max_db_bytes,
    );
    // `writes_blocked` is the operator-facing question; `state` distinguishes
    // healthy from warn from degraded, and the two together say whether a
    // server is near its budget or already past it.
    gauge(
        &mut out,
        "annex_storage_writes_blocked",
        "1 when the storage gate is refusing writes.",
        u8::from(state.storage_health.writes_blocked()),
    );
    out.push_str("# HELP annex_storage_state Storage gate state; always 1.\n");
    out.push_str("# TYPE annex_storage_state gauge\n");
    out.push_str(&format!(
        "annex_storage_state{{state=\"{}\"}} 1\n",
        state.storage_health.state().as_str(),
    ));

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        out,
    )
        .into_response()
}
