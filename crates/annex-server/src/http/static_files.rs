//! Static-file mounts attached to the application router.
//!
//! Two mounts are managed here:
//!
//! 1. `/uploads/*` — served from `AppState::upload_dir` when that directory
//!    exists. Skipped (with an informational log) if the directory has not
//!    been created yet; the first upload will create it on demand.
//! 2. The client SPA — served from `ANNEX_CLIENT_DIR` (default `client/dist`)
//!    when an `index.html` is present, with `index.html` as the SPA fallback
//!    so client-side routes resolve. Skipped if the directory is missing.
//!
//! Both functions are pure router transformers: pass a `Router` in, get a
//! `Router` back. Behaviour, log messages, and skip conditions are unchanged
//! from the previous inline implementation in `lib.rs`.

use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

/// Attaches the `/uploads/*` static mount, if the directory exists.
pub(crate) fn attach_uploads(router: Router, upload_dir: &str) -> Router {
    if std::path::Path::new(upload_dir).exists() {
        tracing::info!(path = %upload_dir, "serving uploaded files at /uploads");
        router.nest_service("/uploads", ServeDir::new(upload_dir))
    } else {
        tracing::info!(path = %upload_dir, "uploads directory not found yet (will be created on first upload)");
        router
    }
}

/// Attaches the client SPA mount as the router's fallback service, if the
/// configured directory contains an `index.html`. The directory is resolved
/// from `ANNEX_CLIENT_DIR` (default `client/dist`) and canonicalised when
/// possible so that subsequent serving is independent of the working
/// directory.
pub(crate) fn attach_client_dist(router: Router) -> Router {
    let client_dir =
        std::env::var("ANNEX_CLIENT_DIR").unwrap_or_else(|_| "client/dist".to_string());
    let client_dir = match std::fs::canonicalize(&client_dir) {
        Ok(abs) => {
            let s = abs.to_string_lossy().to_string();
            tracing::info!(original = %client_dir, resolved = %s, "canonicalized client directory path");
            s
        }
        Err(_) => {
            if !std::path::Path::new(&client_dir).is_absolute() {
                tracing::warn!(
                    path = %client_dir,
                    "ANNEX_CLIENT_DIR is relative and could not be canonicalized — \
                     static file serving depends on working directory"
                );
            }
            client_dir
        }
    };
    if std::path::Path::new(&client_dir)
        .join("index.html")
        .exists()
    {
        tracing::info!(path = %client_dir, "serving client static files");
        let index = format!("{client_dir}/index.html");
        router.fallback_service(ServeDir::new(&client_dir).fallback(ServeFile::new(index)))
    } else {
        tracing::info!(path = %client_dir, "client directory not found, skipping static file serving");
        router
    }
}
