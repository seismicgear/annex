//! Authorised access to chat attachments.
//!
//! `/uploads` was a bare `tower_http::services::ServeDir`, mounted outside the
//! authenticated route group. Anyone holding a URL could fetch the file: no
//! session, no channel membership, no expiry. Random UUID filenames make a URL
//! hard to guess; they do not make it an authorization check.
//!
//! Three consequences, and the third is the one that decides this:
//!
//! * A member removed from a channel keeps every attachment URL they ever saw,
//!   forever. The messages become unreachable to them; the files do not.
//! * A URL that leaks — a screenshot, a pasted link, a browser profile someone
//!   else uses — grants the file to whoever reads it.
//! * A private conversation's FILES did not have the same access boundary as
//!   its messages, while the UI presents them as one thing.
//!
//! ## Why signed URLs rather than a header or a cookie
//!
//! An `Authorization` header is not available: the browser fetches these
//! through `<img src>` and `<video src>`, which cannot carry one. That is the
//! constraint that shapes everything else.
//!
//! A session cookie would work on the web and fail in the desktop app, which
//! loads from `tauri://localhost` and talks to the server cross-origin — so the
//! cookie would need `SameSite=None`, which needs HTTPS, and a self-hosted
//! server on a LAN often has neither. One mechanism that behaves identically in
//! both places is worth more than two that each work somewhere.
//!
//! So: a short-lived grant, minted by an authenticated call, appended as a
//! query parameter by the client's single URL-resolution chokepoint. The grant
//! names the caller; authorization is still a live membership check at fetch
//! time, so leaving a channel takes effect on the next request rather than
//! whenever a token happens to expire.

use std::sync::Arc;

use axum::{
    extract::{Extension, Path as AxumPath, Query},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::api::ApiError;
use crate::middleware::IdentityContext;
use crate::AppState;

/// How long a grant is good for.
///
/// Short, because it is a bearer credential that travels in a URL and lands in
/// server logs and browser history. Long enough that scrolling a channel does
/// not re-mint constantly. The grant is only half the check — membership is
/// re-read on every fetch — so its lifetime bounds the leak of an *identity
/// assertion*, not of the files.
pub const UPLOAD_GRANT_TTL_SECS: u64 = 900;

#[derive(Debug, Serialize)]
pub struct UploadGrantResponse {
    pub token: String,
    #[serde(rename = "expiresInSecs")]
    pub expires_in_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct GrantQuery {
    #[serde(default)]
    pub t: Option<String>,
}

fn sign(payload: &str, secret: &[u8; 32]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key length is valid");
    mac.update(payload.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// `pseudonym|epoch|expires|sig`, base64url so it survives a query string.
pub fn generate_upload_grant(
    pseudonym: &str,
    secret: &[u8; 32],
    ttl_secs: u64,
    token_epoch: i64,
) -> String {
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + ttl_secs;
    let payload = format!("{pseudonym}|{token_epoch}|{expires}");
    let sig = sign(&payload, secret);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{payload}|{sig}"))
}

pub struct VerifiedGrant {
    pub pseudonym: String,
    pub epoch: i64,
}

/// Verify a grant's signature and expiry. Says nothing about membership.
pub fn verify_upload_grant(token: &str, secret: &[u8; 32]) -> Option<VerifiedGrant> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .ok()?;
    let decoded = String::from_utf8(raw).ok()?;
    let (payload, sig) = decoded.rsplit_once('|')?;

    // Constant-time compare: a byte-at-a-time comparison of a MAC is a forgery
    // oracle given enough attempts, and this endpoint is reachable by anyone.
    use subtle::ConstantTimeEq;
    let expected = sign(payload, secret);
    if !bool::from(expected.as_bytes().ct_eq(sig.as_bytes())) {
        return None;
    }

    let mut fields = payload.split('|');
    let pseudonym = fields.next()?.to_string();
    let epoch: i64 = fields.next()?.parse().ok()?;
    let expires: u64 = fields.next()?.parse().ok()?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if expires <= now {
        return None;
    }
    Some(VerifiedGrant { pseudonym, epoch })
}

/// `POST /api/uploads/grant` — mint a short-lived attachment grant.
pub async fn issue_upload_grant(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Json<UploadGrantResponse>, ApiError> {
    Ok(Json(UploadGrantResponse {
        token: generate_upload_grant(
            &identity.pseudonym_id,
            &state.ws_token_secret,
            UPLOAD_GRANT_TTL_SECS,
            identity.token_epoch,
        ),
        expires_in_secs: UPLOAD_GRANT_TTL_SECS,
    }))
}

/// `GET /uploads/chat/{category}/{filename}` — authorised attachment fetch.
pub async fn serve_chat_upload(
    Extension(state): Extension<Arc<AppState>>,
    AxumPath((category, filename)): AxumPath<(String, String)>,
    Query(q): Query<GrantQuery>,
) -> Response {
    let Some(token) = q.t.as_deref() else {
        return refuse(
            StatusCode::UNAUTHORIZED,
            "this attachment requires a grant; request one from /api/uploads/grant",
        );
    };
    let Some(grant) = verify_upload_grant(token, &state.ws_token_secret) else {
        return refuse(StatusCode::UNAUTHORIZED, "grant is invalid or expired");
    };

    // Reject traversal before the value is ever joined to a path. `..`, a
    // separator, or a NUL in either segment means a caller is trying to leave
    // the upload directory, and there is no legitimate request that looks like
    // this.
    if !is_safe_segment(&category) || !is_safe_segment(&filename) {
        return refuse(StatusCode::BAD_REQUEST, "invalid attachment path");
    }

    // The upload id is the filename stem: chat uploads are written as
    // `{upload_id}.{ext}`.
    let upload_id = filename.split('.').next().unwrap_or("").to_string();
    if upload_id.is_empty() {
        return refuse(StatusCode::BAD_REQUEST, "invalid attachment path");
    }

    let state_for_db = state.clone();
    let lookup = tokio::task::spawn_blocking(move || -> Result<Option<(String, i64)>, String> {
        let conn = state_for_db.pool.get().map_err(|e| e.to_string())?;
        use rusqlite::OptionalExtension;
        // channel_id for the file, and the CURRENT epoch of the grant holder.
        let channel: Option<String> = conn
            .query_row(
                "SELECT channel_id FROM uploads WHERE server_id = ?1 AND upload_id = ?2",
                rusqlite::params![state_for_db.server_id, &upload_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        let epoch: Option<i64> = conn
            .query_row(
                "SELECT token_epoch FROM platform_identities \
                 WHERE server_id = ?1 AND pseudonym_id = ?2 AND active = 1",
                rusqlite::params![state_for_db.server_id, &grant.pseudonym],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let (Some(channel), Some(epoch)) = (channel, epoch) else {
            return Ok(None);
        };
        let member =
            annex_channels::is_member(&conn, state_for_db.server_id, &channel, &grant.pseudonym)
                .map_err(|e| e.to_string())?;
        Ok(member.then_some((channel, epoch)))
    })
    .await;

    let authorized = match lookup {
        Ok(Ok(Some((_channel, current_epoch)))) => {
            // A revoked grant is refused here too. Without this, revoking a
            // member's sessions would stop their API calls and leave their
            // in-flight attachment grants working until they expired.
            if current_epoch != grant.epoch {
                return refuse(StatusCode::UNAUTHORIZED, "grant has been revoked");
            }
            true
        }
        Ok(Ok(None)) => false,
        Ok(Err(e)) => {
            tracing::error!(error = %e, "attachment authorization query failed");
            return refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
        Err(e) => {
            tracing::error!(error = %e, "attachment authorization task failed");
            return refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };

    if !authorized {
        // 404, not 403. A 403 confirms the file exists, which turns this
        // endpoint into an oracle for whether a given upload id is real.
        return refuse(StatusCode::NOT_FOUND, "not found");
    }

    let path = std::path::Path::new(&state.upload_dir)
        .join("chat")
        .join(&category)
        .join(&filename);
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let ct = content_type_for(&filename);
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, ct),
                    // Never cached by a shared proxy: the URL carries a
                    // credential and the response is per-member.
                    (header::CACHE_CONTROL, "private, max-age=300"),
                ],
                bytes,
            )
                .into_response()
        }
        Err(_) => refuse(StatusCode::NOT_FOUND, "not found"),
    }
}

fn refuse(status: StatusCode, message: &str) -> Response {
    (status, message.to_string()).into_response()
}

/// One path segment, no traversal, no separators.
fn is_safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

fn content_type_for(filename: &str) -> &'static str {
    match filename.rsplit('.').next().unwrap_or("") {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "txt" => "text/plain; charset=utf-8",
        // Anything unrecognised is handed back as opaque bytes rather than
        // guessed at, so a file cannot talk a browser into executing it.
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: [u8; 32] = [0x5a; 32];

    #[test]
    fn a_grant_round_trips() {
        let t = generate_upload_grant("alice", &SECRET, 900, 3);
        let v = verify_upload_grant(&t, &SECRET).expect("should verify");
        assert_eq!(v.pseudonym, "alice");
        assert_eq!(v.epoch, 3);
    }

    #[test]
    fn a_grant_signed_with_another_key_is_refused() {
        let t = generate_upload_grant("alice", &SECRET, 900, 0);
        assert!(verify_upload_grant(&t, &[0xa5; 32]).is_none());
    }

    #[test]
    fn an_expired_grant_is_refused() {
        // TTL 0 expires at the same second it is minted, and the check is
        // `expires <= now`.
        let t = generate_upload_grant("alice", &SECRET, 0, 0);
        assert!(verify_upload_grant(&t, &SECRET).is_none());
    }

    #[test]
    fn a_tampered_grant_is_refused() {
        let t = generate_upload_grant("alice", &SECRET, 900, 0);
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&t)
            .unwrap();
        let decoded = String::from_utf8(raw).unwrap();
        // Swap the pseudonym, keep the signature.
        let forged = decoded.replacen("alice", "mallo", 1);
        let reencoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(forged);
        assert!(verify_upload_grant(&reencoded, &SECRET).is_none());
    }

    #[test]
    fn path_segments_that_try_to_escape_are_rejected() {
        for bad in [
            "..",
            ".",
            "",
            "a/b",
            "a\\b",
            "../../etc/passwd",
            "%2e%2e",
            "a b",
        ] {
            assert!(!is_safe_segment(bad), "{bad:?} should be rejected");
        }
        for good in ["images", "abc-123.png", "a_b.webm"] {
            assert!(is_safe_segment(good), "{good:?} should be accepted");
        }
    }

    #[test]
    fn unknown_extensions_are_served_as_opaque_bytes() {
        assert_eq!(content_type_for("x.png"), "image/png");
        assert_eq!(content_type_for("x.svg"), "application/octet-stream");
        assert_eq!(content_type_for("x.html"), "application/octet-stream");
        assert_eq!(content_type_for("noext"), "application/octet-stream");
    }
}
