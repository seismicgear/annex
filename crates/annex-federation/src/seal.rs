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

/// Domain-separation label for federation signaling seals (Ed25519 recipients).
const HKDF_INFO: &[u8] = b"annex-signal-seal-v1";
/// Domain-separation label for end-to-end channel-key wrapping (raw X25519
/// recipients). MUST stay byte-identical to the TS client (`client/src/lib/e2e.ts`)
/// and is covered by a cross-language Known-Answer-Test.
pub const E2E_INFO: &[u8] = b"annex-e2e-seal-v1";
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

fn derive_key(shared: &[u8], epk: &[u8; 32], recipient_pub: &[u8; 32], info: &[u8]) -> Key {
    let mut salt = [0u8; 64];
    salt[..32].copy_from_slice(epk);
    salt[32..].copy_from_slice(recipient_pub);
    let hk = Hkdf::<Sha256>::new(Some(&salt), shared);
    let mut okm = [0u8; 32];
    hk.expand(info, &mut okm)
        .expect("hkdf expand of 32 bytes never fails");
    *Key::from_slice(&okm)
}

/// Core sealing routine over X25519 keys. The ephemeral secret and nonce are
/// injected so callers can be either randomized (production) or deterministic
/// (Known-Answer-Tests). `info` is the HKDF domain-separation label.
fn seal_x25519_inner(
    plaintext: &[u8],
    recipient_pub: &XPublic,
    esk: &XSecret,
    nonce_bytes: &[u8; NONCE_LEN],
    info: &[u8],
) -> Result<Vec<u8>, SealError> {
    let epk = XPublic::from(esk);
    let shared = esk.diffie_hellman(recipient_pub);

    let key = derive_key(
        shared.as_bytes(),
        epk.as_bytes(),
        recipient_pub.as_bytes(),
        info,
    );
    let cipher = ChaCha20Poly1305::new(&key);
    let ct = cipher
        .encrypt(
            Nonce::from_slice(nonce_bytes),
            Payload {
                msg: plaintext,
                aad: epk.as_bytes(),
            },
        )
        .map_err(|_| SealError::Encrypt)?;

    let mut out = Vec::with_capacity(EPK_LEN + NONCE_LEN + ct.len());
    out.extend_from_slice(epk.as_bytes());
    out.extend_from_slice(nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Core opening routine over an X25519 recipient secret.
fn open_x25519_inner(
    blob: &[u8],
    recipient_secret: &XSecret,
    info: &[u8],
) -> Result<Vec<u8>, SealError> {
    if blob.len() < EPK_LEN + NONCE_LEN {
        return Err(SealError::TooShort);
    }
    let mut epk_bytes = [0u8; 32];
    epk_bytes.copy_from_slice(&blob[..EPK_LEN]);
    let epk = XPublic::from(epk_bytes);
    let nonce_bytes = &blob[EPK_LEN..EPK_LEN + NONCE_LEN];
    let ct = &blob[EPK_LEN + NONCE_LEN..];

    let recipient_pub = XPublic::from(recipient_secret);
    let shared = recipient_secret.diffie_hellman(&epk);

    let key = derive_key(
        shared.as_bytes(),
        epk.as_bytes(),
        recipient_pub.as_bytes(),
        info,
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

/// Seal `plaintext` to a raw X25519 public key (end-to-end channel-key
/// wrapping). Byte-compatible with `client/src/lib/e2e.ts::sealTo`. Produces a
/// fresh ephemeral key + nonce on every call.
pub fn seal_x25519(plaintext: &[u8], recipient_pub: &[u8; 32]) -> Result<Vec<u8>, SealError> {
    let mut esk_bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut esk_bytes);
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    seal_x25519_inner(
        plaintext,
        &XPublic::from(*recipient_pub),
        &XSecret::from(esk_bytes),
        &nonce_bytes,
        E2E_INFO,
    )
}

/// Open a blob sealed with [`seal_x25519`] using the recipient's X25519 secret.
/// Byte-compatible with `client/src/lib/e2e.ts::openFrom`.
pub fn open_x25519(blob: &[u8], recipient_secret: &[u8; 32]) -> Result<Vec<u8>, SealError> {
    open_x25519_inner(blob, &XSecret::from(*recipient_secret), E2E_INFO)
}

/// Derive the X25519 public key for a raw 32-byte X25519 secret. Used by the
/// member key directory so a peer can be addressed without exchanging Ed25519
/// material. Matches `client/src/lib/e2e.ts::publicKeyFromSecret`.
pub fn x25519_public_key(secret: &[u8; 32]) -> [u8; 32] {
    XPublic::from(&XSecret::from(*secret)).to_bytes()
}

/// Seal `plaintext` so only the holder of `recipient`'s Ed25519 secret key can
/// open it. Safe to hand to an untrusted relay.
pub fn seal(plaintext: &[u8], recipient: &VerifyingKey) -> Result<Vec<u8>, SealError> {
    let recipient_x = x25519_public_from_ed25519(recipient);

    let mut esk_bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut esk_bytes);
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    seal_x25519_inner(
        plaintext,
        &recipient_x,
        &XSecret::from(esk_bytes),
        &nonce_bytes,
        HKDF_INFO,
    )
}

/// Open a sealed blob with the recipient's Ed25519 signing key.
pub fn open(blob: &[u8], recipient: &SigningKey) -> Result<Vec<u8>, SealError> {
    open_x25519_inner(blob, &x25519_secret_from_ed25519(recipient), HKDF_INFO)
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

    // ---- X25519 raw-key seal (E2E channel-key wrapping) ----

    #[test]
    fn x25519_round_trip() {
        let recipient_secret = [0x11u8; 32];
        let recipient_pub = x25519_public_key(&recipient_secret);
        let cek = b"this-is-a-32-byte-channel-keyyyy";
        let blob = seal_x25519(cek, &recipient_pub).unwrap();
        assert_eq!(open_x25519(&blob, &recipient_secret).unwrap(), cek);
    }

    #[test]
    fn x25519_wrong_recipient_fails() {
        let recipient_pub = x25519_public_key(&[0x22u8; 32]);
        let blob = seal_x25519(b"secret cek", &recipient_pub).unwrap();
        assert!(matches!(
            open_x25519(&blob, &[0x33u8; 32]),
            Err(SealError::Decrypt)
        ));
    }

    /// Cross-language Known-Answer-Test. These exact bytes are reproduced by the
    /// TypeScript client (`client/src/lib/e2e.test.ts`). Any drift in the
    /// construction (HKDF salt/info, AAD, wire layout, cipher) breaks this in
    /// BOTH languages — that is the point: it pins Rust↔TS interop.
    const KAT_RECIPIENT_SECRET: [u8; 32] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f, 0x20,
    ];
    const KAT_EPHEMERAL_SECRET: [u8; 32] = [
        0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae,
        0xaf, 0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbb, 0xbc, 0xbd,
        0xbe, 0xbf,
    ];
    const KAT_NONCE: [u8; NONCE_LEN] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b,
    ];
    const KAT_PLAINTEXT: &[u8] = b"annex-e2e-kat-v1 channel content key payload";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn x25519_kat_is_stable_and_cross_language() {
        let recipient_pub = x25519_public_key(&KAT_RECIPIENT_SECRET);
        let blob = seal_x25519_inner(
            KAT_PLAINTEXT,
            &XPublic::from(recipient_pub),
            &XSecret::from(KAT_EPHEMERAL_SECRET),
            &KAT_NONCE,
            E2E_INFO,
        )
        .unwrap();

        // Emit the vector when run with --nocapture so the TS test can mirror it.
        println!("KAT recipient_pub = {}", hex(&recipient_pub));
        println!("KAT wire          = {}", hex(&blob));

        // Pinned expected output (generated by this very test, then frozen).
        const EXPECTED_RECIPIENT_PUB: &str =
            "07a37cbc142093c8b755dc1b10e86cb426374ad16aa853ed0bdfc0b2b86d1c7c";
        const EXPECTED_WIRE: &str = "605a725d2a4adfeeb1a29e17edd621c1b7593ee8cdbc44ac6c4ab6e2f805d23c000102030405060708090a0bc70438103cf37965facd5e288820f2e8ee205588a4da314bb857d2ed407e95f8abd7be0b6bec226711e8ba00657a946ad787ac6c6af1877c65cd6f3b";

        assert_eq!(hex(&recipient_pub), EXPECTED_RECIPIENT_PUB);
        assert_eq!(hex(&blob), EXPECTED_WIRE);

        // And it must round-trip.
        assert_eq!(
            open_x25519(&blob, &KAT_RECIPIENT_SECRET).unwrap(),
            KAT_PLAINTEXT
        );
    }
}
