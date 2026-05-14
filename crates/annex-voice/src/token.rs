//! Signed, expiring voice-join tokens.
//!
//! A voice-join token is a capability the server hands to an
//! authenticated user after a successful `POST /api/channels/:id/voice/join`.
//! Possessing one means: "this pseudonym was authorised by the server to
//! enter this channel's voice room at this point in time."
//!
//! ## Why this is a real token now
//!
//! The legacy `generate_join_token` produced an unsigned base64-encoded
//! JSON blob (`{room, sub, name, iss: "annex-native-sfu"}`). It had no
//! expiry, no signature, and was never verified. The only thing keeping
//! unauthorised users out of voice rooms was the per-request membership
//! check at the HTTP /voice/join handler and the SECOND membership check
//! at the WS WebRtcOffer handler. The token itself was a vestigial
//! artifact whose name implied stronger security than it provided.
//!
//! This module replaces that with an HMAC-SHA256 signed token bound to
//! `(room, sub, expiry)`, mirroring the format used by
//! `crates/annex-server/src/ws/tokens.rs` for WebSocket session tokens:
//!
//!   `base64url_no_pad("voice|room|sub|expires_unix_secs|hex(hmac_sha256_signature)")`
//!
//! `verify_join_token` checks the signature with constant-time
//! comparison (via `hmac::Mac::verify_slice`), validates the expiry,
//! and optionally cross-checks the room and subject. The HMAC key is
//! derived from the server's Ed25519 signing key with a dedicated
//! domain-separation prefix (`b"annex-voice-token-v1:"`) so it cannot
//! be substituted for the WS-token secret.
//!
//! ## Defence in depth
//!
//! In the current in-process SFU architecture, the WS WebRtcOffer
//! handler ALSO runs a membership check (see
//! `crates/annex-server/src/ws/commands/webrtc.rs`). That means the
//! token is not the only gate today. But it is the right primitive for
//! a multi-SFU deployment where the SFU process must independently
//! authorise a peer's join, and it closes the gap between the field
//! name "token" and what the token actually proved.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Decoded voice-join claims surfaced to the verifier's caller. Bound
/// to a specific room and pseudonym at sign time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceClaims {
    /// The channel/room identifier the token authorises.
    pub room: String,
    /// The pseudonym the token was issued to.
    pub sub: String,
    /// Unix seconds at which the token expires (inclusive).
    pub exp: u64,
}

/// Errors surfaced by [`verify_join_token`]. `Tampered` covers both a
/// failed HMAC check and a structurally malformed token — the caller
/// should not distinguish them, since both indicate forgery.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum VoiceTokenError {
    #[error("voice token is malformed or tampered with")]
    Tampered,
    #[error("voice token expired at {expires_unix_secs}; now is {now_unix_secs}")]
    Expired {
        expires_unix_secs: u64,
        now_unix_secs: u64,
    },
    #[error("voice token room {actual:?} does not match expected {expected:?}")]
    WrongRoom { expected: String, actual: String },
    #[error("voice token subject {actual:?} does not match expected {expected:?}")]
    WrongSubject { expected: String, actual: String },
}

/// Derive a 32-byte HMAC key for voice-join tokens from the server's
/// Ed25519 signing key. Uses SHA-256 with a domain-separation prefix so
/// the derived key is independent of the WS-token secret and of any
/// other use of the signing key.
pub fn derive_voice_token_secret(signing_key: &ed25519_dalek::SigningKey) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"annex-voice-token-v1:");
    hasher.update(signing_key.as_bytes());
    let result = hasher.finalize();
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&result);
    secret
}

/// Default voice-join token TTL: 5 minutes. Long enough to cover the
/// HTTP→WS round-trip and a slow SFU handshake; short enough that a
/// leaked token cannot be used to lurk in a room after the user has
/// been removed from membership.
pub const VOICE_TOKEN_DEFAULT_TTL_SECS: u64 = 300;

/// Generate an HMAC-SHA256 signed voice-join token.
///
/// `room` is the channel id; `sub` is the pseudonym the token is bound
/// to; `ttl_secs` controls how long the token is valid. The function
/// rejects `|` in `room` / `sub` because the token format is pipe-
/// delimited; without this check a caller could smuggle `pseudonym=x|exp=…`
/// into the room field and confuse the parser. In practice the channel
/// id is a UUID and the pseudonym is hex, neither of which contains
/// `|`, so this rejection is purely a defensive guard.
pub fn generate_join_token(
    room: &str,
    sub: &str,
    secret: &[u8; 32],
    ttl_secs: u64,
) -> Result<String, VoiceTokenError> {
    if room.contains('|') || sub.contains('|') {
        return Err(VoiceTokenError::Tampered);
    }

    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let expires = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + ttl_secs;

    let payload = format!("voice|{room}|{sub}|{expires}");

    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key length is valid");
    mac.update(payload.as_bytes());
    let signature = mac.finalize().into_bytes();

    let token_bytes = format!("{}|{}", payload, hex::encode(signature));
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes.as_bytes()))
}

/// Verify a voice-join token and return the decoded claims.
///
/// * `expected_room` — when `Some`, the token's room must match exactly.
///   Pass `None` when the caller cannot constrain the room ahead of
///   time (e.g. tooling that inspects a token).
/// * `expected_sub` — same semantics for the pseudonym binding.
///
/// Verification is constant-time: the HMAC signature is checked with
/// `hmac::Mac::verify_slice` before any structural claim is returned.
pub fn verify_join_token(
    token: &str,
    secret: &[u8; 32],
    expected_room: Option<&str>,
    expected_sub: Option<&str>,
) -> Result<VoiceClaims, VoiceTokenError> {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .map_err(|_| VoiceTokenError::Tampered)?;
    let token_str = String::from_utf8(decoded).map_err(|_| VoiceTokenError::Tampered)?;

    // Format: voice|room|sub|expires|sig_hex
    let parts: Vec<&str> = token_str.splitn(5, '|').collect();
    if parts.len() != 5 || parts[0] != "voice" {
        return Err(VoiceTokenError::Tampered);
    }
    let room = parts[1];
    let sub = parts[2];
    let expires_str = parts[3];
    let sig_hex = parts[4];

    // Verify HMAC before any other check so tampered tokens that happen
    // to parse cleanly are still rejected.
    let payload = format!("voice|{room}|{sub}|{expires_str}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key length is valid");
    mac.update(payload.as_bytes());
    let provided_sig = hex::decode(sig_hex).map_err(|_| VoiceTokenError::Tampered)?;
    mac.verify_slice(&provided_sig)
        .map_err(|_| VoiceTokenError::Tampered)?;

    let expires: u64 = expires_str.parse().map_err(|_| VoiceTokenError::Tampered)?;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now > expires {
        return Err(VoiceTokenError::Expired {
            expires_unix_secs: expires,
            now_unix_secs: now,
        });
    }

    if let Some(expected) = expected_room {
        if expected != room {
            return Err(VoiceTokenError::WrongRoom {
                expected: expected.to_string(),
                actual: room.to_string(),
            });
        }
    }
    if let Some(expected) = expected_sub {
        if expected != sub {
            return Err(VoiceTokenError::WrongSubject {
                expected: expected.to_string(),
                actual: sub.to_string(),
            });
        }
    }

    Ok(VoiceClaims {
        room: room.to_string(),
        sub: sub.to_string(),
        exp: expires,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn fresh_secret() -> [u8; 32] {
        let key = SigningKey::generate(&mut OsRng);
        derive_voice_token_secret(&key)
    }

    #[test]
    fn round_trip_valid_token() {
        let secret = fresh_secret();
        let token = generate_join_token("ch-1", "alice", &secret, 60).unwrap();
        let claims = verify_join_token(&token, &secret, Some("ch-1"), Some("alice")).unwrap();
        assert_eq!(claims.room, "ch-1");
        assert_eq!(claims.sub, "alice");
    }

    #[test]
    fn wrong_room_rejected() {
        let secret = fresh_secret();
        let token = generate_join_token("ch-1", "alice", &secret, 60).unwrap();
        let err = verify_join_token(&token, &secret, Some("ch-2"), None).unwrap_err();
        assert!(matches!(err, VoiceTokenError::WrongRoom { .. }));
    }

    #[test]
    fn wrong_subject_rejected() {
        let secret = fresh_secret();
        let token = generate_join_token("ch-1", "alice", &secret, 60).unwrap();
        let err = verify_join_token(&token, &secret, None, Some("bob")).unwrap_err();
        assert!(matches!(err, VoiceTokenError::WrongSubject { .. }));
    }

    #[test]
    fn tampered_signature_rejected() {
        let secret = fresh_secret();
        let token = generate_join_token("ch-1", "alice", &secret, 60).unwrap();
        // Decode → mutate last byte → re-encode.
        use base64::Engine;
        let mut raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token.as_bytes())
            .unwrap();
        let len = raw.len();
        raw[len - 1] ^= 0x01;
        let tampered = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw);
        let err = verify_join_token(&tampered, &secret, None, None).unwrap_err();
        assert_eq!(err, VoiceTokenError::Tampered);
    }

    #[test]
    fn wrong_secret_rejected() {
        let secret_a = fresh_secret();
        let secret_b = fresh_secret();
        let token = generate_join_token("ch-1", "alice", &secret_a, 60).unwrap();
        let err = verify_join_token(&token, &secret_b, None, None).unwrap_err();
        assert_eq!(err, VoiceTokenError::Tampered);
    }

    #[test]
    fn expired_token_rejected() {
        let secret = fresh_secret();
        // Construct an already-expired token directly with the same code
        // path, then verify. We can't easily backdate `generate_join_token`
        // without injecting a clock, so synthesise one inline using the
        // same wire format.
        use base64::Engine;
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let expires = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 10;
        let payload = format!("voice|ch-1|alice|{expires}");
        let mut mac = Hmac::<Sha256>::new_from_slice(&secret).unwrap();
        mac.update(payload.as_bytes());
        let sig = hex::encode(mac.finalize().into_bytes());
        let body = format!("{payload}|{sig}");
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(body.as_bytes());

        let err = verify_join_token(&token, &secret, None, None).unwrap_err();
        assert!(matches!(err, VoiceTokenError::Expired { .. }));
    }

    #[test]
    fn malformed_token_rejected() {
        let secret = fresh_secret();
        let err = verify_join_token("not-base64-???", &secret, None, None).unwrap_err();
        assert_eq!(err, VoiceTokenError::Tampered);

        use base64::Engine;
        let nonsense =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"voice|only|three|parts");
        let err = verify_join_token(&nonsense, &secret, None, None).unwrap_err();
        assert_eq!(err, VoiceTokenError::Tampered);
    }

    #[test]
    fn pipe_in_room_or_sub_rejected_at_sign_time() {
        let secret = fresh_secret();
        // `|` is the delimiter, so we never want to ship a token whose
        // room or sub could be re-parsed as another field.
        let err = generate_join_token("ch|nope", "alice", &secret, 60).unwrap_err();
        assert_eq!(err, VoiceTokenError::Tampered);
        let err = generate_join_token("ch-1", "alice|bob", &secret, 60).unwrap_err();
        assert_eq!(err, VoiceTokenError::Tampered);
    }

    #[test]
    fn ws_token_secret_does_not_verify_voice_token() {
        // Domain-separation regression. The voice secret is derived from
        // the signing key with a different prefix; using the WS-shaped
        // secret directly must NOT verify a voice token.
        let key = SigningKey::generate(&mut OsRng);
        let voice_secret = derive_voice_token_secret(&key);
        let token = generate_join_token("ch-1", "alice", &voice_secret, 60).unwrap();

        // Construct a WS-style secret using a different prefix.
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"annex-ws-token-v1:");
        hasher.update(key.as_bytes());
        let mut ws_secret = [0u8; 32];
        ws_secret.copy_from_slice(&hasher.finalize());

        let err = verify_join_token(&token, &ws_secret, None, None).unwrap_err();
        assert_eq!(err, VoiceTokenError::Tampered);
    }
}
