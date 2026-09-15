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
    // ONLY `/uploads/server` is public.
    //
    // This used to mount the whole `upload_dir` as a bare `ServeDir` at
    // `/uploads`, outside the authenticated route group — so every chat
    // attachment was fetchable by anyone holding its URL, with no session, no
    // channel membership and no expiry. A member removed from a channel kept
    // every attachment URL they had ever seen, permanently: the messages became
    // unreachable to them and the files did not.
    //
    // Random UUID filenames make a URL hard to guess. They are not an
    // authorization check, and treating them as one meant a private
    // conversation's FILES had a different access boundary from its messages
    // while the UI presents them as one thing.
    //
    // `/uploads/server` is the server's own icon and banner — deliberately
    // public, because they are shown on the join screen to people who have no
    // identity yet. Chat attachments are served by
    // `api_uploads_access::serve_chat_upload`, which checks a signed grant and
    // re-reads membership on every request.
    // Mounted unconditionally, NOT behind an `exists()` check.
    //
    // The previous version decided at router-construction time whether to
    // mount at all, which on a fresh server means the directory does not exist
    // yet and the mount is skipped — permanently, until someone restarts the
    // process. So the first operator to upload a server icon got a 404 for it
    // and no indication why. `ServeDir` resolves paths per request and answers
    // 404 for a missing directory on its own, so the check bought nothing and
    // cost a startup-ordering dependency.
    let server_dir = std::path::Path::new(upload_dir).join("server");
    tracing::info!(
        path = %server_dir.display(),
        "serving public server branding at /uploads/server"
    );
    router.nest_service("/uploads/server", ServeDir::new(server_dir))
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
