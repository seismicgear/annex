//! Global tower/axum layers applied to the fully-merged router.
//!
//! The order below is load-bearing — outer layers run first on the way in
//! and last on the way out. From outermost to innermost:
//!
//! 1. `Extension(Arc<AppState>)` — handlers can extract shared state.
//! 2. CORS — applied before our middleware so preflight (`OPTIONS`) responses
//!    are answered with CORS headers without auth/rate-limit interference.
//! 3. Security-headers middleware.
//! 4. Body-size limit (`MAX_REQUEST_BODY_BYTES`).
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

use axum::{extract::DefaultBodyLimit, Extension, Router};
use tower_http::cors::CorsLayer;

use crate::middleware;
use crate::state::AppState;

/// Maximum request body size (2 MiB). Protects against OOM from oversized payloads.
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 2 * 1024 * 1024;

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
    router
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(axum::middleware::from_fn(
            middleware::security_headers_middleware,
        ))
        .layer(cors_layer)
        .layer(Extension(shared_state))
}
