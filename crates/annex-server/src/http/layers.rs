//! Global tower/axum layers applied to the fully-merged router.
//!
//! The order below is load-bearing — outer layers run first on the way in
//! and last on the way out. From outermost to innermost:
//!
//! 1. `Extension(Arc<AppState>)` — handlers can extract shared state.
//! 2. CORS — applied before our middleware so preflight (`OPTIONS`) responses
//!    are answered with CORS headers without auth/rate-limit interference.
//! 3. Rate-limit middleware.
//! 4. Security-headers middleware.
//! 5. Body-size limit (`MAX_REQUEST_BODY_BYTES`).
//!
//! Rate-limit and security-headers run after CORS so they only see real,
//! same-origin / approved cross-origin requests.

use std::sync::Arc;

use axum::{extract::DefaultBodyLimit, Extension, Router};
use tower_http::cors::CorsLayer;

use crate::middleware;
use crate::state::AppState;

/// Maximum request body size (2 MiB). Protects against OOM from oversized payloads.
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Wraps the router with the global layer chain (body limit, security
/// headers, rate limit, CORS, shared state extension). Order is preserved
/// exactly from the previous inline implementation.
pub(crate) fn apply_global_layers(
    router: Router,
    shared_state: Arc<AppState>,
    cors_layer: CorsLayer,
) -> Router {
    router
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(axum::middleware::from_fn(
            middleware::security_headers_middleware,
        ))
        .layer(axum::middleware::from_fn(middleware::rate_limit_middleware))
        .layer(cors_layer)
        .layer(Extension(shared_state))
}
