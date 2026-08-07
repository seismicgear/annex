//! HMAC-signed session tokens for the WebSocket and REST surfaces.
//!
//! Token format (preserved verbatim from the previous inline
//! implementation):
//!
//!   `base64url_no_pad("pseudonym|expires_unix_secs|hex(hmac_sha256_signature))`
//!
//! The HMAC key is derived once at startup from the server's Ed25519
//! signing key with a domain-separation prefix
//! (`b"annex-ws-token-v1:"`) so the derived key is independent of any
//! other use of the signing key.
//!
//! Two TTL constants are exposed:
//!
//!   * [`WS_TOKEN_TTL_SECS`] — 60 s. Used by `POST /api/ws/token`. The
//!     WebSocket upgrade exchanges this token for a session, and the
//!     short window limits replay if a token is leaked unused.
//!   * [`SESSION_TOKEN_TTL_SECS`] — 1 h. Used by
//!     `verify-membership` after ZK proof verification; the client
//!     auto-refreshes via `POST /api/session/refresh`.
//!
//! [`verify_token_allow_expired`] permits tokens up to 7 days past their
//! `expires` timestamp so a returning user whose app sat closed for a few
//! days can be rotated to a fresh token without re-doing the ZK proof.
//! Anything older is rejected.

use axum::http::StatusCode;

/// Duration for which a WebSocket session token is valid (60 seconds).
///
/// These are NOT single-use, whatever an earlier version of this comment
/// claimed. [`verify_ws_token`] checks an HMAC and a clock and keeps no
/// state, so a token can be spent as many times as its lifetime allows; the
/// TTL is the entire replay bound. That is worth stating plainly because a
/// reader who believes "single-use" will reason about a replay window that
/// does not exist.
///
/// Two things follow, both pinned by `tests/api_ws_token.rs`:
///
///   * A replay opens a fully functional second session. Since
///     `ConnectionManager::add_session` keeps one session per pseudonym by
///     design, the newer socket takes the older one's place in the broadcast
///     registry — the victim's socket stays open and stops receiving.
///   * The shipped client makes the window an hour, not a minute. It never
///     calls `POST /api/ws/token`; `client/src/lib/ws.ts` connects with the
///     REST session token (`SESSION_TOKEN_TTL_SECS`) instead, so the
///     short-lived token this constant describes is currently unused.
///
/// Making the upgrade single-use is not a local change: consumption cannot
/// live in [`verify_ws_token`], which [`verify_ws_token_for_auth`] calls on
/// every REST request under `enforce_zk_proofs` — burning the token there
/// would sign the user out after one API call. It needs a consumption store
/// on the upgrade path only, and the client minting a fresh token per
/// connection so reconnects are not locked out.
pub const WS_TOKEN_TTL_SECS: u64 = 60;

/// Duration for which a REST session token is valid (1 hour).
/// Issued by verify-membership after ZK proof verification. The client
/// auto-refreshes before expiry via `POST /api/ws/token`.
pub const SESSION_TOKEN_TTL_SECS: u64 = 3600;

/// Derive a 32-byte HMAC key for WebSocket session tokens from the server's
/// Ed25519 signing key. Uses SHA-256 with a domain-separation prefix so the
/// derived key is independent of any other use of the signing key.
pub fn derive_ws_token_secret(signing_key: &ed25519_dalek::SigningKey) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"annex-ws-token-v1:");
    hasher.update(signing_key.as_bytes());
    let result = hasher.finalize();
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&result);
    secret
}

/// Generates an HMAC-SHA256 signed session token with a configurable TTL.
///
/// Token format: `base64(pseudonym|expires_unix_secs|hmac_signature)`
/// The token binds the pseudonym to a time window, preventing both
/// impersonation (different pseudonym) and replay (after expiry).
pub fn generate_session_token(pseudonym: &str, secret: &[u8; 32], ttl_secs: u64) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + ttl_secs;

    let payload = format!("{pseudonym}|{expires}");

    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key length is valid");
    mac.update(payload.as_bytes());
    let signature = mac.finalize().into_bytes();

    use base64::Engine;
    let token_bytes = format!("{}|{}", payload, hex::encode(signature));
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes.as_bytes())
}

/// Verifies an HMAC-SHA256 signed WebSocket session token.
/// Returns the pseudonym if valid and not expired.
pub(crate) fn verify_ws_token(token: &str, secret: &[u8; 32]) -> Result<String, StatusCode> {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let token_str = String::from_utf8(decoded).map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Parse: pseudonym|expires|signature_hex
    let parts: Vec<&str> = token_str.splitn(3, '|').collect();
    if parts.len() != 3 {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let pseudonym = parts[0];
    let expires_str = parts[1];
    let sig_hex = parts[2];

    // Verify HMAC using constant-time comparison to prevent timing side-channels
    let payload = format!("{pseudonym}|{expires_str}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key length is valid");
    mac.update(payload.as_bytes());
    let provided_sig = hex::decode(sig_hex).map_err(|_| StatusCode::UNAUTHORIZED)?;
    mac.verify_slice(&provided_sig)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Check expiry
    let expires: u64 = expires_str.parse().map_err(|_| StatusCode::UNAUTHORIZED)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if now > expires {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(pseudonym.to_string())
}

/// Public wrapper for session token verification, used by the REST auth middleware
/// when `enforce_zk_proofs` is enabled.
pub fn verify_ws_token_for_auth(token: &str, secret: &[u8; 32]) -> Result<String, StatusCode> {
    verify_ws_token(token, secret)
}

/// Verify HMAC signature of a session token but allow recently-expired tokens.
/// Used by the session refresh endpoint to re-issue tokens for returning users
/// whose session expired while the app was closed.
///
/// Allows tokens expired up to 7 days ago. Tokens older than that are rejected
/// to limit the replay window if a token is ever leaked.
pub fn verify_token_allow_expired(token: &str, secret: &[u8; 32]) -> Result<String, StatusCode> {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let token_str = String::from_utf8(decoded).map_err(|_| StatusCode::UNAUTHORIZED)?;

    let parts: Vec<&str> = token_str.splitn(3, '|').collect();
    if parts.len() != 3 {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let pseudonym = parts[0];
    let expires_str = parts[1];
    let sig_hex = parts[2];

    // Verify HMAC — proves the token was issued by this server
    let payload = format!("{pseudonym}|{expires_str}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key length is valid");
    mac.update(payload.as_bytes());
    let provided_sig = hex::decode(sig_hex).map_err(|_| StatusCode::UNAUTHORIZED)?;
    mac.verify_slice(&provided_sig)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Allow recently-expired tokens (up to 7 days) for session refresh.
    // This limits the replay window if a token is ever leaked while still
    // accommodating users who haven't opened the app in a few days.
    const MAX_EXPIRED_AGE_SECS: u64 = 7 * 24 * 60 * 60; // 7 days
    let expires: u64 = expires_str.parse().map_err(|_| StatusCode::UNAUTHORIZED)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now > expires + MAX_EXPIRED_AGE_SECS {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(pseudonym.to_string())
}
