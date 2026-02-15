use annex_identity::{get_platform_identity, PlatformIdentity};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

use crate::AppState;

/// Wrapper for `PlatformIdentity` to be stored in request extensions.
#[derive(Clone, Debug)]
pub struct IdentityContext(pub PlatformIdentity);

/// Middleware to authenticate requests via `X-Annex-Pseudonym` or `Authorization: Bearer`.
///
/// # Security Note
///
/// In this phase (Phase 2), authentication relies on the pseudonym acting as a bearer token.
/// There is currently no cryptographic signature verification for individual requests.
/// This is a known limitation of the current roadmap state. Future phases (Client/Hardening)
/// will likely introduce signed requests or session tokens.
///
/// For now, the "Bearer" token IS the pseudonym.
pub async fn auth_middleware(mut req: Request<Body>, next: Next) -> Result<Response, StatusCode> {
    // 1. Extract pseudonym from header
    let pseudonym = if let Some(val) = req.headers().get("X-Annex-Pseudonym") {
        val.to_str()
            .map_err(|_| StatusCode::UNAUTHORIZED)?
            .to_string()
    } else if let Some(val) = req.headers().get("Authorization") {
        let val_str = val.to_str().map_err(|_| StatusCode::UNAUTHORIZED)?;
        if let Some(token) = val_str.strip_prefix("Bearer ") {
            token.to_string()
        } else {
            return Err(StatusCode::UNAUTHORIZED);
        }
    } else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    // 2. Get AppState
    let state = req
        .extensions()
        .get::<Arc<AppState>>()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?
        .clone();

    let server_id = state.server_id;

    // 3. Verify Identity (blocking DB operation)
    let identity = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        // Get Identity
        // We treat any error (including "not found") as Unauthorized for security
        get_platform_identity(&conn, server_id, &pseudonym).map_err(|_| StatusCode::UNAUTHORIZED)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    // 4. Check if active
    if !identity.active {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // 5. Insert into extensions
    req.extensions_mut().insert(IdentityContext(identity));

    Ok(next.run(req).await)
}
