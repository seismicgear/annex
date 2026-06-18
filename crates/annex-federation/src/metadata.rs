//! Metadata hardening for the signaling rendezvous.
//!
//! The rendezvous relay (`api/signal.js`) is already *content*-blind — the SDP
//! is sealed (see [`crate::seal`]) so it never sees ICE candidates or IPs. This
//! module hardens the remaining *metadata* the relay would otherwise observe:
//!
//!  * **Who-talks-to-whom (the slug graph).** Instead of addressing a queue by a
//!    stable server slug, peers address it by a [`rendezvous_tag`] — a rotating,
//!    one-way function of the recipient's X25519 key and a coarse time bucket.
//!    The relay sees opaque tags that change every bucket and cannot be linked
//!    across buckets or back to a server without the recipient's public key
//!    (which it never holds).
//!
//!  * **Payload length.** SDP size varies with the number of ICE candidates,
//!    network interfaces, etc. [`seal_padded`] pads every payload up to a fixed
//!    block before sealing, so the ciphertext length is constant and leaks
//!    nothing about the contents.
//!
//!  * **Traffic presence.** [`decoy_payload`] produces indistinguishable cover
//!    traffic so a peer can post/poll on a steady cadence whether or not it has
//!    a real session, hiding *when* federation actually happens.

use crate::seal::{
    open as seal_open, open_x25519, seal as seal_seal, seal_x25519,
    x25519_public_from_verifying_key, SealError,
};
use base64::Engine;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Rendezvous tags rotate on this cadence (seconds). One hour balances
/// unlinkability against clock skew between peers.
pub const BUCKET_SECONDS: u64 = 3600;

/// Fixed padded plaintext block. SDPs comfortably fit in 4 KiB; larger payloads
/// round up to the next multiple, so length is quantised rather than exact.
pub const PAD_BLOCK: usize = 4096;

const TAG_DOMAIN: &[u8] = b"annex-rendezvous-tag-v1";
const LEN_PREFIX: usize = 4;

/// The current time bucket (UTC seconds / [`BUCKET_SECONDS`]).
pub fn current_bucket() -> u64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs / BUCKET_SECONDS
}

/// A rotating, unlinkable rendezvous address for a recipient in a given bucket.
///
/// `tag = base64url( SHA256( domain ‖ recipient_pub ‖ bucket_le ) )`.
///
/// The sender computes it from the recipient's published X25519 key (which it
/// already needs to seal the payload); the recipient computes it from its own
/// key. Pre-image resistance means the relay cannot recover the recipient key
/// from the tag, and the per-bucket input means tags for the same recipient are
/// unlinkable across buckets.
pub fn rendezvous_tag(recipient_pub: &[u8; 32], bucket: u64) -> String {
    let mut h = Sha256::new();
    h.update(TAG_DOMAIN);
    h.update(recipient_pub);
    h.update(bucket.to_le_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(h.finalize())
}

/// Convenience: the recipient's tag for the current bucket.
pub fn rendezvous_tag_now(recipient_pub: &[u8; 32]) -> String {
    rendezvous_tag(recipient_pub, current_bucket())
}

/// The rendezvous tag for a peer identified by its Ed25519 verifying key — the
/// same identity key used to seal payloads to it. Lets the signaling transport
/// address a peer's rotating queue without any extra key exchange.
pub fn rendezvous_tag_for(recipient: &VerifyingKey, bucket: u64) -> String {
    rendezvous_tag(&x25519_public_from_verifying_key(recipient), bucket)
}

/// Convenience: [`rendezvous_tag_for`] in the current bucket.
pub fn rendezvous_tag_for_now(recipient: &VerifyingKey) -> String {
    rendezvous_tag_for(recipient, current_bucket())
}

/// Pad `plaintext` to a multiple of [`PAD_BLOCK`] with a length prefix so the
/// original can be recovered exactly. Layout: `len(4, LE) ‖ plaintext ‖ zeros`.
fn pad(plaintext: &[u8]) -> Vec<u8> {
    let needed = LEN_PREFIX + plaintext.len();
    let blocks = needed.div_ceil(PAD_BLOCK).max(1);
    let total = blocks * PAD_BLOCK;
    let mut out = vec![0u8; total];
    out[..LEN_PREFIX].copy_from_slice(&(plaintext.len() as u32).to_le_bytes());
    out[LEN_PREFIX..LEN_PREFIX + plaintext.len()].copy_from_slice(plaintext);
    out
}

fn unpad(padded: &[u8]) -> Result<Vec<u8>, SealError> {
    if padded.len() < LEN_PREFIX {
        return Err(SealError::TooShort);
    }
    let len = u32::from_le_bytes([padded[0], padded[1], padded[2], padded[3]]) as usize;
    if LEN_PREFIX + len > padded.len() {
        return Err(SealError::Decrypt);
    }
    Ok(padded[LEN_PREFIX..LEN_PREFIX + len].to_vec())
}

/// Seal `plaintext` to the recipient with length-hiding padding. The resulting
/// blob has a constant size for any input up to [`PAD_BLOCK`] (and quantised
/// size beyond), so the relay learns nothing from the ciphertext length.
/// Byte-compatible recipient model with [`seal_x25519`].
pub fn seal_padded(plaintext: &[u8], recipient_pub: &[u8; 32]) -> Result<Vec<u8>, SealError> {
    seal_x25519(&pad(plaintext), recipient_pub)
}

/// Open a blob produced by [`seal_padded`], stripping the padding.
pub fn open_padded(blob: &[u8], recipient_secret: &[u8; 32]) -> Result<Vec<u8>, SealError> {
    let padded = open_x25519(blob, recipient_secret)?;
    unpad(&padded)
}

/// Length-hiding seal to a peer's Ed25519 identity key (the recipient model the
/// federation transport uses). Pads to a fixed block, then seals with the
/// Ed25519→X25519 sealed box, so the relay sees a constant-size opaque blob.
pub fn seal_padded_to(plaintext: &[u8], recipient: &VerifyingKey) -> Result<Vec<u8>, SealError> {
    seal_seal(&pad(plaintext), recipient)
}

/// Open a blob produced by [`seal_padded_to`] with our Ed25519 signing key.
pub fn open_padded_from(blob: &[u8], recipient: &SigningKey) -> Result<Vec<u8>, SealError> {
    unpad(&seal_open(blob, recipient)?)
}

/// Cover traffic: a sealed blob addressed to a random ephemeral key that no one
/// can open, but is indistinguishable on the wire (same construction, same
/// padded length) from a real [`seal_padded`] payload. Posting these on a steady
/// cadence hides whether/when real federation is happening.
pub fn decoy_payload() -> Vec<u8> {
    let mut throwaway_pub = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut throwaway_pub);
    // A random "public key" is almost surely a valid X25519 u-coordinate; even
    // if not, sealing still produces a correctly-shaped, unopenable blob.
    let mut filler = [0u8; 64];
    rand::rngs::OsRng.fill_bytes(&mut filler);
    seal_padded(&filler, &throwaway_pub).unwrap_or_else(|_| {
        // Construction never fails for fixed-size input; fall back to noise of
        // the expected padded+overhead length so cover traffic still matches.
        let mut v = vec![0u8; 32 + 12 + PAD_BLOCK + 16];
        rand::rngs::OsRng.fill_bytes(&mut v);
        v
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seal::x25519_public_key;

    #[test]
    fn tag_is_stable_within_a_bucket_and_rotates_across_buckets() {
        let pubk = x25519_public_key(&[5u8; 32]);
        assert_eq!(rendezvous_tag(&pubk, 100), rendezvous_tag(&pubk, 100));
        assert_ne!(rendezvous_tag(&pubk, 100), rendezvous_tag(&pubk, 101));
    }

    #[test]
    fn tag_differs_per_recipient() {
        let a = x25519_public_key(&[1u8; 32]);
        let b = x25519_public_key(&[2u8; 32]);
        assert_ne!(rendezvous_tag(&a, 7), rendezvous_tag(&b, 7));
    }

    #[test]
    fn tag_is_a_fixed_width_opaque_digest() {
        // base64url(no-pad) of a 32-byte SHA-256 digest is 43 chars and reveals
        // nothing about the recipient key (one-way).
        let pubk = x25519_public_key(&[9u8; 32]);
        let tag = rendezvous_tag(&pubk, 42);
        assert_eq!(tag.len(), 43);
        assert!(tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    }

    #[test]
    fn padded_seal_has_constant_length_regardless_of_input() {
        let recip_secret = [0x21u8; 32];
        let recip_pub = x25519_public_key(&recip_secret);
        let big = [b'x'; 1500];
        let short = seal_padded(b"v=0", &recip_pub).unwrap();
        let long = seal_padded(&big, &recip_pub).unwrap();
        assert_eq!(
            short.len(),
            long.len(),
            "padded ciphertext length must not leak payload size"
        );
        // And it round-trips.
        assert_eq!(open_padded(&short, &recip_secret).unwrap(), b"v=0");
        assert_eq!(open_padded(&long, &recip_secret).unwrap(), big);
    }

    #[test]
    fn padding_round_trips_empty_and_block_boundary() {
        let s = [0x33u8; 32];
        let p = x25519_public_key(&s);
        for n in [
            0usize,
            1,
            PAD_BLOCK - LEN_PREFIX - 1,
            PAD_BLOCK,
            PAD_BLOCK + 1,
        ] {
            let msg = vec![7u8; n];
            let blob = seal_padded(&msg, &p).unwrap();
            assert_eq!(open_padded(&blob, &s).unwrap(), msg, "n={n}");
        }
    }

    #[test]
    fn larger_payloads_quantise_to_blocks() {
        let p = x25519_public_key(&[0x44u8; 32]);
        let one_block = seal_padded(&[1u8; 100], &p).unwrap();
        let two_block = seal_padded(&[1u8; PAD_BLOCK], &p).unwrap();
        assert_eq!(two_block.len(), one_block.len() + PAD_BLOCK);
    }

    #[test]
    fn ed25519_padded_seal_round_trips_and_hides_length() {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;
        let sk = SigningKey::generate(&mut OsRng);
        let vk = sk.verifying_key();
        let short = seal_padded_to(b"v=0", &vk).unwrap();
        let long = seal_padded_to(&[b'y'; 2000], &vk).unwrap();
        assert_eq!(short.len(), long.len(), "padded length must not leak size");
        assert_eq!(open_padded_from(&short, &sk).unwrap(), b"v=0");
        assert_eq!(open_padded_from(&long, &sk).unwrap(), vec![b'y'; 2000]);
    }

    #[test]
    fn tag_for_verifying_key_matches_raw_tag() {
        use ed25519_dalek::SigningKey;
        use rand::rngs::OsRng;
        let sk = SigningKey::generate(&mut OsRng);
        let vk = sk.verifying_key();
        let raw_pub = x25519_public_from_verifying_key(&vk);
        assert_eq!(rendezvous_tag_for(&vk, 9), rendezvous_tag(&raw_pub, 9));
    }

    #[test]
    fn decoy_matches_real_payload_length_and_is_unopenable() {
        let recip_secret = [0x55u8; 32];
        let recip_pub = x25519_public_key(&recip_secret);
        let real = seal_padded(b"real offer", &recip_pub).unwrap();
        let decoy = decoy_payload();
        assert_eq!(decoy.len(), real.len(), "decoy must match real on the wire");
        // The real recipient cannot open the decoy (it isn't addressed to them).
        assert!(open_padded(&decoy, &recip_secret).is_err());
    }
}
