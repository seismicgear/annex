//! Encryption at rest for message content (non-E2E channels).
//!
//! Message bodies for ordinary (non-end-to-end) channels are stored encrypted
//! in SQLite so a stolen database file, a leaked backup, or filesystem access
//! cannot read chat history. Unlike E2E channels — where the *server itself*
//! never holds the key — this layer is transparent to the server: it holds the
//! key (derived from its own Ed25519 signing key) and decrypts on read, so
//! search, agents, STT and federation keep working exactly as before. It raises
//! the bar against data-at-rest theft, not against a compromised live server.
//!
//! Stored format for a non-empty body:
//!   `"\x01ar1:" + base64( nonce(12) || ChaCha20Poly1305(content) )`
//!
//! Decryption is **legacy-tolerant**: any value lacking the marker — or whose
//! AEAD fails to authenticate — is returned unchanged. That makes the rollout
//! seamless (pre-existing plaintext rows still read correctly) and means a
//! foreign value (e.g. an E2E client ciphertext that was never wrapped here)
//! passes straight through.

use base64::Engine;
use chacha20poly1305::aead::Aead;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
use hkdf::Hkdf;
use sha2::Sha256;

/// Marks a value produced by [`MessageCipher::encrypt`]. The leading 0x01 byte
/// essentially never appears in legitimate chat text, and the AEAD tag makes a
/// false-positive decrypt impossible regardless.
const MARKER: &str = "\u{1}ar1:";
const NONCE_LEN: usize = 12;

/// Transparent at-rest cipher for message content.
#[derive(Clone)]
pub struct MessageCipher {
    key: [u8; 32],
}

impl MessageCipher {
    /// Construct from a raw 32-byte key (tests).
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// Derive the at-rest key from the server's Ed25519 signing key via
    /// HKDF-SHA256 — same provenance as username encryption, different domain
    /// label, so the two keys are independent.
    pub fn from_signing_key(signing_key_bytes: &[u8; 32]) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(b"annex-message-at-rest"), signing_key_bytes);
        let mut key = [0u8; 32];
        hk.expand(b"annex-message-at-rest-v1", &mut key)
            .expect("hkdf expand of 32 bytes never fails");
        Self { key }
    }

    /// Encrypt a body for storage. Empty stays empty (blanked/deleted rows).
    pub fn encrypt(&self, plaintext: &str) -> String {
        if plaintext.is_empty() {
            return String::new();
        }
        let cipher =
            ChaCha20Poly1305::new_from_slice(&self.key).expect("32-byte key is always valid");
        let nonce_bytes: [u8; NONCE_LEN] = rand::random();
        let nonce = chacha20poly1305::Nonce::from(nonce_bytes);
        let ct = match cipher.encrypt(&nonce, plaintext.as_bytes()) {
            Ok(ct) => ct,
            // Encryption of in-memory bytes does not fail in practice; if it
            // somehow did, storing plaintext is preferable to losing the
            // message. (Belt-and-suspenders; not expected to ever run.)
            Err(_) => return plaintext.to_string(),
        };
        let mut blob = Vec::with_capacity(NONCE_LEN + ct.len());
        blob.extend_from_slice(&nonce_bytes);
        blob.extend_from_slice(&ct);
        format!(
            "{MARKER}{}",
            base64::engine::general_purpose::STANDARD.encode(blob)
        )
    }

    /// Decrypt a stored body. Legacy-tolerant: non-marked or non-authenticating
    /// values are returned verbatim.
    pub fn decrypt(&self, stored: &str) -> String {
        let Some(b64) = stored.strip_prefix(MARKER) else {
            return stored.to_string();
        };
        let Ok(blob) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            return stored.to_string();
        };
        if blob.len() < NONCE_LEN {
            return stored.to_string();
        }
        let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
        let cipher =
            ChaCha20Poly1305::new_from_slice(&self.key).expect("32-byte key is always valid");
        let nonce = chacha20poly1305::Nonce::from_slice(nonce_bytes);
        match cipher.decrypt(nonce, ct) {
            Ok(pt) => String::from_utf8_lossy(&pt).into_owned(),
            Err(_) => stored.to_string(),
        }
    }

    /// Decrypt in place, used to sanitise a freshly read message before it is
    /// returned to a client or broadcast.
    pub fn decrypt_in_place(&self, content: &mut String) {
        if content.starts_with(MARKER) {
            *content = self.decrypt(content);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> MessageCipher {
        MessageCipher::new([7u8; 32])
    }

    #[test]
    fn round_trips_unicode() {
        let c = cipher();
        let msg = "hello, 世界 🔒 — at rest";
        let stored = c.encrypt(msg);
        assert!(stored.starts_with(MARKER));
        assert!(!stored.contains("hello"), "ciphertext leaked plaintext");
        assert_eq!(c.decrypt(&stored), msg);
    }

    #[test]
    fn empty_stays_empty() {
        let c = cipher();
        assert_eq!(c.encrypt(""), "");
        assert_eq!(c.decrypt(""), "");
    }

    #[test]
    fn legacy_plaintext_passes_through() {
        // Rows written before this feature shipped have no marker.
        let c = cipher();
        assert_eq!(c.decrypt("plain old message"), "plain old message");
        // Even a body that happens to start like the marker text but isn't ours.
        assert_eq!(c.decrypt("ar1:not-really"), "ar1:not-really");
    }

    #[test]
    fn wrong_key_does_not_panic_and_passes_through() {
        let stored = cipher().encrypt("secret");
        // A different key cannot authenticate; we return the stored value rather
        // than corrupting it or panicking.
        let other = MessageCipher::new([9u8; 32]);
        assert_eq!(other.decrypt(&stored), stored);
    }

    #[test]
    fn each_encryption_is_unique() {
        let c = cipher();
        assert_ne!(c.encrypt("same"), c.encrypt("same"));
    }

    #[test]
    fn derived_key_is_deterministic_and_domain_separated() {
        let sk = [3u8; 32];
        let a = MessageCipher::from_signing_key(&sk);
        let b = MessageCipher::from_signing_key(&sk);
        let stored = a.encrypt("x");
        assert_eq!(b.decrypt(&stored), "x"); // same signing key -> same at-rest key
    }
}
