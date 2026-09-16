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
///
/// Issued by verify-membership after ZK proof verification. The client
/// auto-refreshes it via `POST /api/session/refresh`, which accepts an
/// expired-but-validly-signed token — see `startTokenRefresh` in
/// `client/src/api/core.ts`.
///
/// This said `POST /api/ws/token`, four lines below a comment stating that
/// the client never calls that endpoint. Both cannot be true, and it was the
/// wrong one: `/api/ws/token` mints a 60-second WS token and would not
/// refresh a REST session even if the client did call it.
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
pub fn generate_session_token(
    pseudonym: &str,
    secret: &[u8; 32],
    ttl_secs: u64,
    token_epoch: i64,
) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + ttl_secs;

    // `pseudonym|epoch|expires` — the v2 payload. The epoch is inside the MAC,
    // so a holder cannot edit it, and it is checked against the identity's
    // current `token_epoch` at verify time. Bumping that column invalidates
    // every token for one identity and nothing else.
    let payload = format!("{pseudonym}|{token_epoch}|{expires}");

    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key length is valid");
    mac.update(payload.as_bytes());
    let signature = mac.finalize().into_bytes();

    use base64::Engine;
    let token_bytes = format!("{}|{}", payload, hex::encode(signature));
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes.as_bytes())
}

/// What a verified token carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedToken {
    pub pseudonym: String,
    /// The `token_epoch` this token was minted against.
    ///
    /// `0` for a pre-migration token, which is also the column's default — so
    /// an upgrade does not sign anyone out. The caller is responsible for
    /// comparing this against the identity's current epoch; parsing alone
    /// proves the server issued the token, not that it is still valid.
    pub epoch: i64,
    /// Unix seconds at which the token expired (or will).
    pub expires: u64,
}

/// Verify the MAC and parse a token, WITHOUT checking expiry or epoch.
///
/// Accepts both shapes:
///   v2  `pseudonym|epoch|expires|sig`
///   v1  `pseudonym|expires|sig`      (pre-`044_identity_token_epoch`)
///
/// v1 is tried only after v2 fails to verify, and yields `epoch: 0`. Keeping it
/// for one release is what stops the migration signing every user out mid-
/// upgrade; it can be deleted once no unexpired v1 token can exist, which is
/// `SESSION_TOKEN_TTL_SECS` plus [`MAX_EXPIRED_AGE_SECS`] after deployment.
///
/// Pseudonyms are hex, so they never contain `|` — which is what makes the two
/// shapes unambiguous rather than merely usually-distinguishable.
fn parse_and_verify_mac(token: &str, secret: &[u8; 32]) -> Result<VerifiedToken, StatusCode> {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let token_str = String::from_utf8(decoded).map_err(|_| StatusCode::UNAUTHORIZED)?;

    let mac_ok = |payload: &str, sig_hex: &str| -> bool {
        let Ok(provided) = hex::decode(sig_hex) else {
            return false;
        };
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key length is valid");
        mac.update(payload.as_bytes());
        // Constant-time comparison — `verify_slice`, never `==` on the digest.
        mac.verify_slice(&provided).is_ok()
    };

    // v2
    let v2: Vec<&str> = token_str.splitn(4, '|').collect();
    if v2.len() == 4 {
        let (pseudonym, epoch_str, expires_str, sig_hex) = (v2[0], v2[1], v2[2], v2[3]);
        if mac_ok(&format!("{pseudonym}|{epoch_str}|{expires_str}"), sig_hex) {
            return Ok(VerifiedToken {
                pseudonym: pseudonym.to_string(),
                epoch: epoch_str.parse().map_err(|_| StatusCode::UNAUTHORIZED)?,
                expires: expires_str.parse().map_err(|_| StatusCode::UNAUTHORIZED)?,
            });
        }
    }

    // v1
    let v1: Vec<&str> = token_str.splitn(3, '|').collect();
    if v1.len() == 3 {
        let (pseudonym, expires_str, sig_hex) = (v1[0], v1[1], v1[2]);
        if mac_ok(&format!("{pseudonym}|{expires_str}"), sig_hex) {
            return Ok(VerifiedToken {
                pseudonym: pseudonym.to_string(),
                epoch: 0,
                expires: expires_str.parse().map_err(|_| StatusCode::UNAUTHORIZED)?,
            });
        }
    }

    Err(StatusCode::UNAUTHORIZED)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Verifies an HMAC-SHA256 signed WebSocket session token.
///
/// Returns the pseudonym and the epoch the token was minted against. The epoch
/// is NOT checked here — this function has no database — so every caller must
/// compare it against the identity's current `token_epoch`. `auth_middleware`
/// and the WS upgrade both do; see `revocation::token_epoch_for`.
pub(crate) fn verify_ws_token(token: &str, secret: &[u8; 32]) -> Result<VerifiedToken, StatusCode> {
    let verified = parse_and_verify_mac(token, secret)?;
    if now_secs() > verified.expires {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(verified)
}

/// Public wrapper for session token verification, used by the REST auth middleware
/// when `enforce_zk_proofs` is enabled.
///
/// Returns the epoch alongside the pseudonym. The caller MUST check it — see
/// [`verify_ws_token`]. Returning only the pseudonym, as this used to, made it
/// impossible for a call site to honour revocation even if it wanted to.
pub fn verify_ws_token_for_auth(
    token: &str,
    secret: &[u8; 32],
) -> Result<VerifiedToken, StatusCode> {
    verify_ws_token(token, secret)
}

/// How long past expiry a token may still be refreshed.
///
/// Was seven days. That, combined with `POST /api/session/refresh` being a
/// public endpoint (correctly — its whole job is to accept an expired token),
/// meant a stolen token could be refreshed forever by touching it once a week.
/// Shortening it does not fix that on its own; the epoch check does. This
/// narrows the window in which a token stolen from a device that is now offline
/// is still worth anything.
pub const MAX_EXPIRED_AGE_SECS: u64 = 72 * 60 * 60;

/// Verify a session token's MAC but allow a recently-expired one.
///
/// Used by the refresh endpoint to re-issue for a returning user whose session
/// expired while the app was closed, without re-doing the ZK proof. Expiry is
/// relaxed; the epoch is not — the caller still checks it, so a revoked
/// identity cannot refresh its way back in.
pub fn verify_token_allow_expired(
    token: &str,
    secret: &[u8; 32],
) -> Result<VerifiedToken, StatusCode> {
    let verified = parse_and_verify_mac(token, secret)?;
    if now_secs() > verified.expires + MAX_EXPIRED_AGE_SECS {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(verified)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: [u8; 32] = [7u8; 32];
    const PSEUDONYM: &str = "a1b2c3d4e5f6";

    /// The v1 shape, as `generate_session_token` produced it before migration
    /// 044. Reconstructed here rather than kept as a fixture string so the MAC
    /// is right for whatever secret the test uses.
    fn legacy_token(pseudonym: &str, expires: u64, secret: &[u8; 32]) -> String {
        use base64::Engine;
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let payload = format!("{pseudonym}|{expires}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(payload.as_bytes());
        let sig = hex::encode(mac.finalize().into_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{payload}|{sig}"))
    }

    #[test]
    fn round_trips_the_epoch() {
        let token = generate_session_token(PSEUDONYM, &SECRET, 3600, 42);
        let v = verify_ws_token(&token, &SECRET).expect("fresh token should verify");
        assert_eq!(v.pseudonym, PSEUDONYM);
        assert_eq!(v.epoch, 42, "the epoch must survive the round trip");
    }

    /// The epoch is inside the MAC. Without this, revocation is decoration:
    /// a holder would simply edit the number.
    #[test]
    fn the_epoch_cannot_be_edited_by_the_holder() {
        use base64::Engine;
        let token = generate_session_token(PSEUDONYM, &SECRET, 3600, 1);
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token.as_bytes())
            .unwrap();
        let text = String::from_utf8(decoded).unwrap();
        let parts: Vec<&str> = text.splitn(4, '|').collect();
        let forged = format!("{}|{}|{}|{}", parts[0], 999, parts[2], parts[3]);
        let reencoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(forged.as_bytes());
        assert!(
            verify_ws_token(&reencoded, &SECRET).is_err(),
            "a token with a rewritten epoch must not verify"
        );
    }

    /// Migration 044 must not sign everyone out. A token minted by the previous
    /// release still verifies, and reads as epoch 0 — which is the column's
    /// default, so it matches every identity that has never been revoked.
    #[test]
    fn a_pre_migration_token_still_verifies_as_epoch_zero() {
        let expires = now_secs() + 3600;
        let token = legacy_token(PSEUDONYM, expires, &SECRET);
        let v = verify_ws_token(&token, &SECRET).expect("a v1 token must still verify");
        assert_eq!(v.pseudonym, PSEUDONYM);
        assert_eq!(v.epoch, 0);
    }

    #[test]
    fn a_token_signed_with_another_secret_is_refused() {
        let token = generate_session_token(PSEUDONYM, &SECRET, 3600, 0);
        assert!(verify_ws_token(&token, &[9u8; 32]).is_err());
    }

    #[test]
    fn an_expired_token_is_refused_but_refreshable_inside_the_window() {
        // Mint one that expired an hour ago by asking for a negative-ish TTL
        // via the legacy helper, which takes an absolute expiry.
        let expired_1h = now_secs() - 3600;
        let token = legacy_token(PSEUDONYM, expired_1h, &SECRET);
        assert!(
            verify_ws_token(&token, &SECRET).is_err(),
            "an expired token must not authenticate"
        );
        assert!(
            verify_token_allow_expired(&token, &SECRET).is_ok(),
            "an hour past expiry is inside the refresh window"
        );
    }

    #[test]
    fn a_token_past_the_refresh_window_is_refused() {
        let long_gone = now_secs() - (MAX_EXPIRED_AGE_SECS + 3600);
        let token = legacy_token(PSEUDONYM, long_gone, &SECRET);
        assert!(verify_token_allow_expired(&token, &SECRET).is_err());
    }

    #[test]
    fn garbage_is_refused_rather_than_panicking() {
        for junk in ["", "not-base64!!", "YWJj", "|||"] {
            assert!(verify_ws_token(junk, &SECRET).is_err(), "{junk:?}");
            assert!(
                verify_token_allow_expired(junk, &SECRET).is_err(),
                "{junk:?}"
            );
        }
    }
}
