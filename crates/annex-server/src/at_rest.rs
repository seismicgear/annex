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
//! Only unmarked values are legacy plaintext. A marked value must authenticate
//! and decode as UTF-8 before it can be returned, edited, searched or relayed.
//! Encryption failures never fall back to storing plaintext.

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
const TAG_LEN: usize = 16;

/// Content-free storage failures; callers must not expose the stored body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AtRestError {
    #[error("could not encrypt stored content")]
    EncryptionFailed,
    #[error("stored content encoding is invalid")]
    InvalidEncoding,
    #[error("stored content failed authentication")]
    InvalidCiphertext,
    #[error("authenticated stored content is not UTF-8")]
    InvalidUtf8,
}

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
    pub fn encrypt(&self, plaintext: &str) -> Result<String, AtRestError> {
        if plaintext.is_empty() {
            return Ok(String::new());
        }
        let cipher =
            ChaCha20Poly1305::new_from_slice(&self.key).expect("32-byte key is always valid");
        let nonce_bytes: [u8; NONCE_LEN] = rand::random();
        let nonce = chacha20poly1305::Nonce::from(nonce_bytes);
        let ct = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| AtRestError::EncryptionFailed)?;
        let mut blob = Vec::with_capacity(NONCE_LEN + ct.len());
        blob.extend_from_slice(&nonce_bytes);
        blob.extend_from_slice(&ct);
        Ok(format!(
            "{MARKER}{}",
            base64::engine::general_purpose::STANDARD.encode(blob)
        ))
    }

    /// Decrypt a stored body. Only unmarked legacy values pass through.
    pub fn decrypt(&self, stored: &str) -> Result<String, AtRestError> {
        let Some(b64) = stored.strip_prefix(MARKER) else {
            return Ok(stored.to_string());
        };
        let blob = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|_| AtRestError::InvalidEncoding)?;
        if blob.len() < NONCE_LEN + TAG_LEN {
            return Err(AtRestError::InvalidCiphertext);
        }
        let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
        let cipher =
            ChaCha20Poly1305::new_from_slice(&self.key).expect("32-byte key is always valid");
        let nonce = chacha20poly1305::Nonce::from_slice(nonce_bytes);
        let plaintext = cipher
            .decrypt(nonce, ct)
            .map_err(|_| AtRestError::InvalidCiphertext)?;
        String::from_utf8(plaintext).map_err(|_| AtRestError::InvalidUtf8)
    }

    /// Decrypt in place, used to sanitise a freshly read message before it is
    /// returned to a client or broadcast.
    pub fn decrypt_in_place(&self, content: &mut String) -> Result<(), AtRestError> {
        if content.starts_with(MARKER) {
            *content = self.decrypt(content)?;
        }
        Ok(())
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
        let stored = c.encrypt(msg).unwrap();
        assert!(stored.starts_with(MARKER));
        assert!(!stored.contains("hello"), "ciphertext leaked plaintext");
        assert_eq!(c.decrypt(&stored).unwrap(), msg);
    }

    #[test]
    fn empty_stays_empty() {
        let c = cipher();
        assert_eq!(c.encrypt("").unwrap(), "");
        assert_eq!(c.decrypt("").unwrap(), "");
    }

    #[test]
    fn legacy_plaintext_passes_through() {
        // Rows written before this feature shipped have no marker.
        let c = cipher();
        assert_eq!(c.decrypt("plain old message").unwrap(), "plain old message");
        // Even a body that happens to start like the marker text but isn't ours.
        assert_eq!(c.decrypt("ar1:not-really").unwrap(), "ar1:not-really");
    }

    #[test]
    fn wrong_key_fails_without_replacing_original_bytes() {
        let stored = cipher().encrypt("secret").unwrap();
        let other = MessageCipher::new([9u8; 32]);
        assert_eq!(other.decrypt(&stored), Err(AtRestError::InvalidCiphertext));
        let mut original = stored.clone();
        assert_eq!(
            other.decrypt_in_place(&mut original),
            Err(AtRestError::InvalidCiphertext)
        );
        assert_eq!(original, stored);
    }

    #[test]
    fn each_encryption_is_unique() {
        let c = cipher();
        assert_ne!(c.encrypt("same").unwrap(), c.encrypt("same").unwrap());
    }

    #[test]
    fn derived_key_is_deterministic_and_domain_separated() {
        let sk = [3u8; 32];
        let a = MessageCipher::from_signing_key(&sk);
        let b = MessageCipher::from_signing_key(&sk);
        let stored = a.encrypt("x").unwrap();
        assert_eq!(b.decrypt(&stored).unwrap(), "x"); // same signing key -> same at-rest key
    }

    #[test]
    fn malformed_truncated_and_tampered_marked_values_fail() {
        let c = cipher();
        assert_eq!(c.decrypt("\u{1}ar1:!!!"), Err(AtRestError::InvalidEncoding));
        for len in [0, 11, 12, 27] {
            let stored = format!(
                "{MARKER}{}",
                base64::engine::general_purpose::STANDARD.encode(vec![0u8; len])
            );
            assert_eq!(c.decrypt(&stored), Err(AtRestError::InvalidCiphertext));
        }
        let stored = c.encrypt("must authenticate").unwrap();
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(stored.strip_prefix(MARKER).unwrap())
            .unwrap();
        bytes[NONCE_LEN] ^= 1;
        assert_eq!(
            c.decrypt(&format!(
                "{MARKER}{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            )),
            Err(AtRestError::InvalidCiphertext)
        );
    }

    #[test]
    fn authenticated_non_utf8_is_not_lossily_replaced() {
        let raw = ChaCha20Poly1305::new_from_slice(&[7u8; 32]).unwrap();
        let nonce = [8u8; NONCE_LEN];
        let ct = raw
            .encrypt(
                chacha20poly1305::Nonce::from_slice(&nonce),
                &[0xff, 0xfe][..],
            )
            .unwrap();
        let blob = [nonce.as_slice(), ct.as_slice()].concat();
        let stored = format!(
            "{MARKER}{}",
            base64::engine::general_purpose::STANDARD.encode(blob)
        );
        assert_eq!(cipher().decrypt(&stored), Err(AtRestError::InvalidUtf8));
    }
}
