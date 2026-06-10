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
//! server without hand-configuring `ANNEX_CORS_ORIGINS`.
//!
//! The relaxation is governed by [`dev_localhost_enabled`], resolved once
//! at layer-build time:
//!
//! * `ANNEX_BUILD_PROFILE=production|release` → **always off**, even in a
//!   debug binary. A debug build accidentally deployed under the
//!   production env contract stays strict (previously the relaxation was
//!   gated only on `cfg!(debug_assertions)`, so such a deployment would
//!   silently accept any localhost origin).
//! * `ANNEX_CORS_ALLOW_DEV_LOCALHOST=true|false` → explicit override for
//!   non-production profiles (e.g. a release-built binary used for local
//!   development, or a debug binary that wants strictness).
//! * neither set → `cfg!(debug_assertions)`, the original behaviour:
//!   on in debug builds, compiled out of `--release` builds.

use tower_http::cors::{AllowOrigin, Any, CorsLayer};

/// Resolves whether the dev-localhost CORS relaxation is active.
///
/// Pure function over the two relevant env values so the precedence
/// table is unit-testable without process-global env mutation. The
/// precedence is: production profile (hard off) → explicit override →
/// `cfg!(debug_assertions)`. See the module docs for the rationale.
pub(crate) fn dev_localhost_enabled(
    build_profile: Option<&str>,
    override_flag: Option<&str>,
) -> bool {
    let profile = build_profile.map(|p| p.trim().to_ascii_lowercase());
    if matches!(profile.as_deref(), Some("production") | Some("release")) {
        if matches!(parse_bool_flag(override_flag), Some(true)) {
            tracing::warn!(
                "ANNEX_CORS_ALLOW_DEV_LOCALHOST=true is ignored under \
                 ANNEX_BUILD_PROFILE=production/release — the production \
                 profile always disables the dev-localhost CORS relaxation"
            );
        }
        return false;
    }
    if let Some(explicit) = parse_bool_flag(override_flag) {
        return explicit;
    }
    if override_flag.is_some() {
        tracing::warn!(
            value = ?override_flag,
            "unrecognized ANNEX_CORS_ALLOW_DEV_LOCALHOST value (expected \
             true/false); falling back to the build-profile default"
        );
    }
    cfg!(debug_assertions)
}

/// Parses a permissive boolean env value. `None` for unset or
/// unrecognized input.
fn parse_bool_flag(raw: Option<&str>) -> Option<bool> {
    match raw.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("1") | Some("true") | Some("yes") => Some(true),
        Some("0") | Some("false") | Some("no") => Some(false),
        _ => None,
    }
}

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

        // Resolved once here; the predicate closure captures the bool,
        // so there is no per-request env read.
        let allow_dev_localhost = dev_localhost_enabled(
            std::env::var("ANNEX_BUILD_PROFILE").ok().as_deref(),
            std::env::var("ANNEX_CORS_ALLOW_DEV_LOCALHOST")
                .ok()
                .as_deref(),
        );

        if allow_dev_localhost {
            tracing::info!(
                origins = ?origins,
                "CORS: configured origins + any localhost (dev relaxation active)"
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
/// Consulted only when [`dev_localhost_enabled`] resolved true at
/// layer-build time (see [`build_cors_layer`]). Production-profile
/// deployments never trust this, regardless of build type.
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
    use super::{dev_localhost_enabled, is_dev_localhost_origin};
    use axum::http::HeaderValue;

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).unwrap()
    }

    // ── dev_localhost_enabled precedence ────────────────────────────

    #[test]
    fn production_profile_always_disables_dev_localhost() {
        assert!(!dev_localhost_enabled(Some("production"), None));
        assert!(!dev_localhost_enabled(Some("release"), None));
        assert!(!dev_localhost_enabled(Some("PRODUCTION"), None));
        assert!(!dev_localhost_enabled(Some(" production "), None));
        // The hard gate beats an explicit opt-in.
        assert!(!dev_localhost_enabled(Some("production"), Some("true")));
        assert!(!dev_localhost_enabled(Some("release"), Some("1")));
    }

    #[test]
    fn explicit_override_wins_outside_production() {
        assert!(dev_localhost_enabled(None, Some("true")));
        assert!(dev_localhost_enabled(None, Some("1")));
        assert!(dev_localhost_enabled(Some("dev"), Some("yes")));
        assert!(!dev_localhost_enabled(None, Some("false")));
        assert!(!dev_localhost_enabled(None, Some("0")));
        assert!(!dev_localhost_enabled(Some("dev"), Some("no")));
    }

    #[test]
    fn unset_falls_back_to_build_type() {
        // The unset default is the compile-time build type: on for
        // debug builds, off for --release. The test asserts against
        // cfg! directly so it is correct under either profile.
        assert_eq!(dev_localhost_enabled(None, None), cfg!(debug_assertions));
        assert_eq!(
            dev_localhost_enabled(Some("dev"), None),
            cfg!(debug_assertions)
        );
        // Unrecognized override values fall back rather than guessing.
        assert_eq!(
            dev_localhost_enabled(None, Some("banana")),
            cfg!(debug_assertions)
        );
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
