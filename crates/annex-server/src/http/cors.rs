//! CORS layer construction.
//!
//! The CORS policy is computed from `AppState::cors_origins`:
//!
//! * empty list  → restrictive (same-origin only, no `Access-Control-Allow-Origin`).
//! * `["*"]`     → permissive (any origin).
//! * explicit list → only those origins.
//!
//! Debug builds additionally accept any `http(s)://localhost[:port]`,
//! `http(s)://127.0.0.1[:port]`, or `http(s)://[::1][:port]` origin via
//! [`is_dev_localhost_origin`]. This exists so `cargo tauri dev` (Vite on
//! :5173) and `cargo run -p annex-server` can be hit from the local dev
//! server without hand-configuring `ANNEX_CORS_ORIGINS`. The relaxation is
//! compiled out of release builds — `cfg!(debug_assertions)` is `false`
//! under `cargo build --release`, so production binaries stay strict.

use tower_http::cors::{AllowOrigin, Any, CorsLayer};

/// Builds the global CORS layer from the configured `cors_origins` list.
pub(crate) fn build_cors_layer(origins: &[String]) -> CorsLayer {
    let is_permissive = origins.iter().any(|o| o == "*");
    let base = CorsLayer::new()
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
            axum::http::Method::PATCH,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderName::from_static("x-annex-pseudonym"),
            axum::http::HeaderName::from_static("x-annex-zk-proof"),
        ]);

    if is_permissive {
        tracing::info!("CORS: permissive mode (allow all origins)");
        base.allow_origin(Any)
    } else {
        let parsed: Vec<axum::http::HeaderValue> = origins
            .iter()
            .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
            .collect();

        if cfg!(debug_assertions) {
            tracing::info!(
                origins = ?origins,
                "CORS: configured origins + any localhost (debug build)"
            );
            let allowed = parsed;
            base.allow_origin(AllowOrigin::predicate(move |origin, _req_parts| {
                allowed.iter().any(|a| a == origin) || is_dev_localhost_origin(origin)
            }))
        } else if origins.is_empty() {
            tracing::info!("CORS: restrictive mode (same-origin only)");
            // No Access-Control-Allow-Origin header → browsers block cross-origin requests
            base.allow_origin(AllowOrigin::list(
                std::iter::empty::<axum::http::HeaderValue>(),
            ))
        } else {
            tracing::info!(origins = ?origins, "CORS: restricted to configured origins");
            base.allow_origin(AllowOrigin::list(parsed))
        }
    }
}

/// Returns `true` when the `Origin` header points at any local dev server:
/// `http(s)://localhost[:port]`, `http(s)://127.0.0.1[:port]`, or
/// `http(s)://[::1][:port]`.
///
/// Consulted only from debug builds (see [`build_cors_layer`]). Release
/// binaries never trust this — the caller is gated on
/// `cfg!(debug_assertions)`.
pub(crate) fn is_dev_localhost_origin(origin: &axum::http::HeaderValue) -> bool {
    let Ok(raw) = origin.to_str() else {
        return false;
    };
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    // Origin headers are `scheme://host[:port]` — no path, query, or fragment.
    // `url::Url::parse` normalizes `http://localhost` to path `/`, so accept
    // that too.
    if !parsed.path().is_empty() && parsed.path() != "/" {
        return false;
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return false;
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return false;
    }
    matches!(
        parsed.host_str(),
        Some("localhost") | Some("127.0.0.1") | Some("[::1]")
    )
}

#[cfg(test)]
mod tests {
    use super::is_dev_localhost_origin;
    use axum::http::HeaderValue;

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).unwrap()
    }

    #[test]
    fn accepts_localhost_with_any_port() {
        assert!(is_dev_localhost_origin(&hv("http://localhost:5173")));
        assert!(is_dev_localhost_origin(&hv("http://localhost:3000")));
        assert!(is_dev_localhost_origin(&hv("http://localhost:1")));
        assert!(is_dev_localhost_origin(&hv("http://localhost")));
        assert!(is_dev_localhost_origin(&hv("https://localhost:5173")));
    }

    #[test]
    fn accepts_loopback_ipv4_and_ipv6() {
        assert!(is_dev_localhost_origin(&hv("http://127.0.0.1:5173")));
        assert!(is_dev_localhost_origin(&hv("http://127.0.0.1")));
        assert!(is_dev_localhost_origin(&hv("http://[::1]:5173")));
        assert!(is_dev_localhost_origin(&hv("http://[::1]")));
    }

    #[test]
    fn rejects_non_loopback_hosts() {
        assert!(!is_dev_localhost_origin(&hv("https://example.com")));
        assert!(!is_dev_localhost_origin(&hv("http://evil.localhost")));
        assert!(!is_dev_localhost_origin(&hv("http://localhost.evil.com")));
        assert!(!is_dev_localhost_origin(&hv("http://127.0.0.2")));
        assert!(!is_dev_localhost_origin(&hv("http://10.0.0.1")));
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(!is_dev_localhost_origin(&hv("tauri://localhost")));
        assert!(!is_dev_localhost_origin(&hv("ws://localhost:5173")));
        assert!(!is_dev_localhost_origin(&hv("file://localhost")));
    }

    #[test]
    fn rejects_origins_with_path_or_query() {
        // A well-formed Origin header has no path/query; reject malformed inputs
        // so an attacker can't smuggle `http://localhost/@evil.com` past the check.
        assert!(!is_dev_localhost_origin(&hv("http://localhost:5173/steal")));
        assert!(!is_dev_localhost_origin(&hv("http://localhost:5173/?x=1")));
        assert!(!is_dev_localhost_origin(&hv(
            "http://user:pw@localhost:5173"
        )));
    }

    #[test]
    fn rejects_garbage() {
        assert!(!is_dev_localhost_origin(&hv("")));
        assert!(!is_dev_localhost_origin(&hv("not-a-url")));
        assert!(!is_dev_localhost_origin(&hv("http://")));
    }
}
