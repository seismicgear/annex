//! Content-blind sealing for federation signaling.
//!
//! Wraps an SDP (or any payload) so that ONLY the addressed peer can read it.
//! The rendezvous relay (`api/signal.js`) and anyone who can observe the
//! signaling queue see only ciphertext — never the ICE candidates / IP
//! addresses inside the SDP, and never the slug graph's contents. Authenticity
//! of the signaling *envelope* is provided separately by the Ed25519
//! `vrp_signature`; this layer provides *confidentiality* of the payload.
//!
//! Construction: an anonymous sealed box.
//!   * The sender generates an EPHEMERAL X25519 keypair (forward secrecy; the
//!     sender's long-term identity never appears in the ciphertext).
//!   * ECDH against the recipient's X25519 key — derived deterministically from
//!     the recipient's Ed25519 identity key (the libsodium
//!     `crypto_sign_ed25519_*_to_curve25519` mapping), so no new key material
//!     has to be provisioned or distributed.
//!   * HKDF-SHA256(shared, salt = epk‖recipient_xpub) → ChaCha20-Poly1305 key.
//!
//! Wire layout: `ephemeral_pub(32) ‖ nonce(12) ‖ ciphertext+tag`, with the
//! ephemeral public key also bound in as AEAD associated data.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use ed25519_dalek::{SigningKey, VerifyingKey};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::{Digest, Sha256, Sha512};
use x25519_dalek::{PublicKey as XPublic, StaticSecret as XSecret};

const HKDF_INFO: &[u8] = b"annex-signal-seal-v1";
const EPK_LEN: usize = 32;
const NONCE_LEN: usize = 12;

#[derive(Debug, thiserror::Error)]
pub enum SealError {
    #[error("sealed blob too short")]
    TooShort,
    #[error("decryption failed (wrong recipient key or tampered ciphertext)")]
    Decrypt,
    #[error("encryption failed")]
    Encrypt,
}

/// Recipient's X25519 public key, derived from its Ed25519 verifying key
/// (Edwards → Montgomery `u` coordinate).
fn x25519_public_from_ed25519(vk: &VerifyingKey) -> XPublic {
    XPublic::from(vk.to_montgomery().to_bytes())
}

/// Recipient's X25519 secret, derived from its Ed25519 signing key
/// (`crypto_sign_ed25519_sk_to_curve25519`: clamped low half of SHA-512(seed)).
fn x25519_secret_from_ed25519(sk: &SigningKey) -> XSecret {
    let h = Sha512::digest(sk.to_bytes());
    let mut s = [0u8; 32];
    s.copy_from_slice(&h[..32]);
    // X25519 clamp (idempotent; X25519 also clamps on use).
    s[0] &= 248;
    s[31] &= 127;
    s[31] |= 64;
    XSecret::from(s)
}

fn derive_key(shared: &[u8], epk: &[u8; 32], recipient_pub: &[u8; 32]) -> Key {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(epk);
    salt[32..].copy_from_slice(recipient_pub);
    let hk = Hkdf::<Sha256>::new(Some(&salt), shared);
    let mut okm = [0u8; 32];
    hk.expand(HKDF_INFO, &mut okm)
        .expect("hkdf expand of 32 bytes never fails");
    *Key::from_slice(&okm)
}

/// Seal `plaintext` so only the holder of `recipient`'s Ed25519 secret key can
/// open it. Safe to hand to an untrusted relay.
pub fn seal(plaintext: &[u8], recipient: &VerifyingKey) -> Result<Vec<u8>, SealError> {
    let recipient_x = x25519_public_from_ed25519(recipient);

    let mut esk_bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut esk_bytes);
    let esk = XSecret::from(esk_bytes);
    let epk = XPublic::from(&esk);
    let shared = esk.diffie_hellman(&recipient_x);

    let key = derive_key(shared.as_bytes(), epk.as_bytes(), recipient_x.as_bytes());
    let cipher = ChaCha20Poly1305::new(&key);

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload {
                msg: plaintext,
                aad: epk.as_bytes(),
            },
        )
        .map_err(|_| SealError::Encrypt)?;

    let mut out = Vec::with_capacity(EPK_LEN + NONCE_LEN + ct.len());
    out.extend_from_slice(epk.as_bytes());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Open a sealed blob with the recipient's Ed25519 signing key.
pub fn open(blob: &[u8], recipient: &SigningKey) -> Result<Vec<u8>, SealError> {
    if blob.len() < EPK_LEN + NONCE_LEN {
        return Err(SealError::TooShort);
    }
    let mut epk_bytes = [0u8; 32];
    epk_bytes.copy_from_slice(&blob[..EPK_LEN]);
    let epk = XPublic::from(epk_bytes);
    let nonce_bytes = &blob[EPK_LEN..EPK_LEN + NONCE_LEN];
    let ct = &blob[EPK_LEN + NONCE_LEN..];

    let xsec = x25519_secret_from_ed25519(recipient);
    let recipient_x_pub = XPublic::from(&xsec);
    let shared = xsec.diffie_hellman(&epk);

    let key = derive_key(
        shared.as_bytes(),
        epk.as_bytes(),
        recipient_x_pub.as_bytes(),
    );
    let cipher = ChaCha20Poly1305::new(&key);
    cipher
        .decrypt(
            Nonce::from_slice(nonce_bytes),
            Payload {
                msg: ct,
                aad: epk.as_bytes(),
            },
        )
        .map_err(|_| SealError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn keypair() -> (SigningKey, VerifyingKey) {
        let sk = SigningKey::generate(&mut OsRng);
        let vk = sk.verifying_key();
        (sk, vk)
    }

    #[test]
    fn ed25519_to_x25519_key_agreement_is_consistent() {
        // The public derived from the verifying key MUST equal the public of the
        // secret derived from the signing key — otherwise ECDH would not agree.
        let (sk, vk) = keypair();
        let from_public = x25519_public_from_ed25519(&vk);
        let from_secret = XPublic::from(&x25519_secret_from_ed25519(&sk));
        assert_eq!(from_public.as_bytes(), from_secret.as_bytes());
    }

    #[test]
    fn round_trip_recovers_plaintext() {
        let (recip_sk, recip_vk) = keypair();
        let sdp = b"v=0\r\no=- 12345 2 IN IP4 203.0.113.7\r\na=candidate:1 1 udp 2122260223 203.0.113.7 54321 typ host";
        let blob = seal(sdp, &recip_vk).unwrap();
        // The sealed blob must not leak the IP that's in the SDP.
        assert!(
            !blob
                .windows(b"203.0.113.7".len())
                .any(|w| w == b"203.0.113.7"),
            "sealed blob leaked the IP address from the SDP"
        );
        let opened = open(&blob, &recip_sk).unwrap();
        assert_eq!(opened, sdp);
    }

    #[test]
    fn a_third_party_cannot_open_it() {
        // Models the relay (or any eavesdropper): has the ciphertext, not the key.
        let (_recip_sk, recip_vk) = keypair();
        let (attacker_sk, _attacker_vk) = keypair();
        let blob = seal(b"secret offer", &recip_vk).unwrap();
        assert!(matches!(open(&blob, &attacker_sk), Err(SealError::Decrypt)));
    }

    #[test]
    fn tampering_is_detected() {
        let (recip_sk, recip_vk) = keypair();
        let mut blob = seal(b"secret offer", &recip_vk).unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0x01; // flip a ciphertext/tag bit
        assert!(matches!(open(&blob, &recip_sk), Err(SealError::Decrypt)));
    }

    #[test]
    fn each_seal_is_unique() {
        // Ephemeral keys + random nonce => distinct ciphertexts for identical input.
        let (_sk, vk) = keypair();
        let a = seal(b"same", &vk).unwrap();
        let b = seal(b"same", &vk).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn truncated_blob_errors() {
        assert!(matches!(
            open(b"short", &keypair().0),
            Err(SealError::TooShort)
        ));
    }
}
