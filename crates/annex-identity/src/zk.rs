pub use ark_bn254::Bn254;
pub use ark_bn254::Fr;
use ark_bn254::{Fq, Fq2};
pub use ark_bn254::{G1Affine, G2Affine};
use ark_ec::AffineRepr;
use ark_ff::{BigInteger, PrimeField};
use ark_groth16::Groth16;
pub use ark_groth16::{Proof, VerifyingKey};
use ark_snark::SNARK;
use serde::Deserialize;
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ZkError {
    #[error("json parse error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("field element parse error")]
    FieldElementError,
    #[error("point parse error")]
    PointError,
    #[error("verification failed")]
    VerificationFailed,
    #[error("arkworks error: {0}")]
    ArkError(#[from] ark_serialize::SerializationError),
    #[error("snark error: {0}")]
    SnarkError(String),
}

#[derive(Deserialize)]
struct SnarkJsProof {
    pi_a: Vec<String>,
    pi_b: Vec<Vec<String>>,
    pi_c: Vec<String>,
}

#[derive(Deserialize)]
struct SnarkJsVKey {
    vk_alpha_1: Vec<String>,
    vk_beta_2: Vec<Vec<String>>,
    vk_gamma_2: Vec<Vec<String>>,
    vk_delta_2: Vec<Vec<String>>,
    #[serde(rename = "IC")]
    ic: Vec<Vec<String>>,
}

pub fn parse_fr(s: &str) -> Result<Fr, ZkError> {
    Fr::from_str(s).map_err(|_| ZkError::FieldElementError)
}

/// Canonical hex serialisation for a BN254 scalar field element.
///
/// Always produces a fixed-width **64-character lowercase** hex string with
/// no `0x` prefix and no leading-zero stripping. This is the single
/// canonical wire and DB encoding for every `Fr` exposed to clients,
/// stored in `vrp_leaves` / `vrp_roots`, returned in registry responses,
/// or fed into the membership-proof public-input parser.
///
/// Implementation note: `BigInteger256::to_bytes_be` always emits exactly
/// 32 bytes — the canonical 256-bit big-endian representation — so the
/// debug assertion below is a tripwire for an arkworks behaviour change,
/// not a runtime branch.
pub fn fr_to_canonical_hex(fr: Fr) -> String {
    let bytes = fr.into_bigint().to_bytes_be();
    debug_assert_eq!(
        bytes.len(),
        32,
        "BN254 Fr must serialise to exactly 32 bytes",
    );
    hex::encode(bytes)
}

/// Strict canonical-hex parser for a BN254 scalar field element.
///
/// Accepts **only** a 64-character lowercase hex string with no `0x`
/// prefix, and only when the encoded value is `< BN254 scalar field
/// modulus`. Rejects:
///   - empty input
///   - non-hex characters
///   - any string of length other than 64
///   - uppercase hex digits (`A-F`)
///   - 64 chars whose value `>=` the field modulus (would be silently
///     reduced by `from_be_bytes_mod_order`, breaking 1:1 hex ↔ Fr)
///
/// This is the parser to use on every NEW boundary that owns its own
/// canonical encoding (writers, freshly-defined APIs, freshly-defined DB
/// columns). Boundaries that must remain backward-compatible with
/// pre-canonical hex (e.g. the membership middleware reading
/// proof.public_signals from a third-party prover) should keep using
/// [`parse_fr_from_hex`].
pub fn parse_canonical_fr_hex(s: &str) -> Result<Fr, ZkError> {
    if s.len() != 64 {
        return Err(ZkError::FieldElementError);
    }
    for c in s.chars() {
        match c {
            '0'..='9' | 'a'..='f' => {}
            _ => return Err(ZkError::FieldElementError),
        }
    }
    parse_fr_from_hex(s)
}

/// Tolerant hex parser for a BN254 scalar field element.
///
/// Accepts any even-length hex up to 64 characters, lowercase or
/// uppercase, decodes big-endian, and rejects values that would be
/// silently reduced modulo the field order. Retained for boundaries that
/// historically accepted variable-length / mixed-case hex from
/// third-party provers and from rows persisted before the canonical
/// helpers existed. New code should use [`parse_canonical_fr_hex`].
pub fn parse_fr_from_hex(hex: &str) -> Result<Fr, ZkError> {
    let bytes = hex::decode(hex).map_err(|_| ZkError::FieldElementError)?;
    let fr = Fr::from_be_bytes_mod_order(&bytes);

    // Verify the value was not silently reduced modulo the field order.
    // If the input >= BN254 scalar field modulus, from_be_bytes_mod_order
    // silently reduces it, creating ambiguity where two different hex strings
    // map to the same field element.
    let roundtrip = fr.into_bigint().to_bytes_be();
    let mut padded = vec![0u8; 32usize.saturating_sub(bytes.len())];
    padded.extend_from_slice(&bytes);
    if padded.len() > 32 {
        return Err(ZkError::FieldElementError);
    }
    if padded != roundtrip {
        return Err(ZkError::FieldElementError);
    }

    Ok(fr)
}

pub fn parse_fq(s: &str) -> Result<Fq, ZkError> {
    Fq::from_str(s).map_err(|_| ZkError::FieldElementError)
}

/// Validates that a G1 affine point lies on the BN254 curve and belongs
/// to the correct prime-order subgroup. Rejecting off-curve or
/// wrong-subgroup points prevents invalid-curve attacks on Groth16.
fn validate_g1(point: &G1Affine) -> Result<(), ZkError> {
    if point.is_zero() {
        // The identity (point at infinity) is a valid group element.
        return Ok(());
    }
    if !point.is_on_curve() {
        return Err(ZkError::PointError);
    }
    if !point.is_in_correct_subgroup_assuming_on_curve() {
        return Err(ZkError::PointError);
    }
    Ok(())
}

/// Validates that a G2 affine point lies on the BN254 twist curve and
/// belongs to the correct prime-order subgroup.
fn validate_g2(point: &G2Affine) -> Result<(), ZkError> {
    if point.is_zero() {
        return Ok(());
    }
    if !point.is_on_curve() {
        return Err(ZkError::PointError);
    }
    if !point.is_in_correct_subgroup_assuming_on_curve() {
        return Err(ZkError::PointError);
    }
    Ok(())
}

fn parse_g1(v: &[String]) -> Result<G1Affine, ZkError> {
    if v.len() < 2 {
        return Err(ZkError::PointError);
    }
    let x = parse_fq(&v[0])?;
    let y = parse_fq(&v[1])?;
    let point = G1Affine::new_unchecked(x, y);
    validate_g1(&point)?;
    Ok(point)
}

fn parse_g2(v: &[Vec<String>]) -> Result<G2Affine, ZkError> {
    if v.len() < 2 {
        return Err(ZkError::PointError);
    }
    if v[0].len() < 2 || v[1].len() < 2 {
        return Err(ZkError::PointError);
    }
    // G2 in SnarkJS is [ [x_c0, x_c1], [y_c0, y_c1], ... ]
    // arkworks Fq2 is c0 + c1*u

    let x_c0 = parse_fq(&v[0][0])?;
    let x_c1 = parse_fq(&v[0][1])?;
    let x = Fq2::new(x_c0, x_c1);

    let y_c0 = parse_fq(&v[1][0])?;
    let y_c1 = parse_fq(&v[1][1])?;
    let y = Fq2::new(y_c0, y_c1);

    let point = G2Affine::new_unchecked(x, y);
    validate_g2(&point)?;
    Ok(point)
}

pub fn parse_proof(json: &str) -> Result<Proof<Bn254>, ZkError> {
    let raw: SnarkJsProof = serde_json::from_str(json)?;

    let a = parse_g1(&raw.pi_a)?;
    let b = parse_g2(&raw.pi_b)?;
    let c = parse_g1(&raw.pi_c)?;

    Ok(Proof { a, b, c })
}

pub fn parse_verification_key(json: &str) -> Result<VerifyingKey<Bn254>, ZkError> {
    let raw: SnarkJsVKey = serde_json::from_str(json)?;

    let alpha_g1 = parse_g1(&raw.vk_alpha_1)?;
    let beta_g2 = parse_g2(&raw.vk_beta_2)?;
    let gamma_g2 = parse_g2(&raw.vk_gamma_2)?;
    let delta_g2 = parse_g2(&raw.vk_delta_2)?;

    let mut gamma_abc_g1 = Vec::with_capacity(raw.ic.len());
    for p in raw.ic {
        gamma_abc_g1.push(parse_g1(&p)?);
    }

    Ok(VerifyingKey {
        alpha_g1,
        beta_g2,
        gamma_g2,
        delta_g2,
        gamma_abc_g1,
    })
}

pub fn parse_public_signals(json: &str) -> Result<Vec<Fr>, ZkError> {
    let raw: Vec<String> = serde_json::from_str(json)?;
    let mut out = Vec::with_capacity(raw.len());
    for s in raw {
        out.push(parse_fr(&s)?);
    }
    Ok(out)
}

pub fn verify_proof(
    vk: &VerifyingKey<Bn254>,
    proof: &Proof<Bn254>,
    public_inputs: &[Fr],
) -> Result<bool, ZkError> {
    Groth16::<Bn254>::verify(vk, public_inputs, proof)
        .map_err(|e| ZkError::SnarkError(e.to_string()))
}

/// Domain-separator tag mixed into v2 topic-hash derivation.
///
/// Prevents pre-image collisions across hashing contexts that use the same
/// SHA-256 primitive (signing-key derivation, nullifier hashing, message
/// digests, etc.). Changing this constant invalidates every previously
/// produced v2 topicHash and is therefore a hard wire-format break.
pub const V2_TOPIC_HASH_DOMAIN: &str = "annex/v2/topicHash:";

/// Canonical topic → BN254 field-element mapping for v2 membership proofs.
///
/// Returns `Fr::from_be_bytes_mod_order(SHA256("annex/v2/topicHash:" + topic))`.
/// The byte input is the raw UTF-8 bytes of the topic string with no
/// canonicalisation: callers must agree on byte equality. Empty topics are
/// rejected because empty topics cannot identify a routing context and are
/// invalid throughout the rest of the API surface (`derive_pseudonym_id`
/// already rejects empty topics).
///
/// # Why this exists
///
/// The `topicHash` public input of `zk/circuits/membership_v2.circom` is
/// supplied by the verifier — the prover binds the proof to whatever value
/// they put there. Without a server-side rule that says "topicHash MUST
/// equal `topic_hash_for_v2(payload.topic)`", a malicious prover can produce
/// a v2 proof for topic A and submit it as a v2 proof for topic B, getting
/// a nullifier-bound pseudonym in topic B without ever having proved
/// membership for topic B. Closing that gap is what this function is for.
///
/// # Errors
///
/// Returns [`ZkError::FieldElementError`] when `topic` is empty.
pub fn topic_hash_for_v2(topic: &str) -> Result<Fr, ZkError> {
    use sha2::{Digest, Sha256};
    if topic.is_empty() {
        return Err(ZkError::FieldElementError);
    }
    let mut hasher = Sha256::new();
    hasher.update(V2_TOPIC_HASH_DOMAIN.as_bytes());
    hasher.update(topic.as_bytes());
    let digest = hasher.finalize();
    // SHA-256 → 32 bytes; reduce big-endian into BN254 Fr. The mod-reduction
    // is uniform-enough for cryptographic purposes (the bias is < 2^-253).
    Ok(Fr::from_be_bytes_mod_order(&digest))
}

/// Generates a dummy verifying key for testing purposes.
/// This key is mathematically valid (points on curve) but useless for verification.
/// It corresponds to an empty circuit.
pub fn generate_dummy_vkey() -> VerifyingKey<Bn254> {
    // Use generator points which are guaranteed to be on the curve
    let g1 = G1Affine::generator();
    let g2 = G2Affine::generator();

    VerifyingKey {
        alpha_g1: g1,
        beta_g2: g2,
        gamma_g2: g2,
        delta_g2: g2,
        gamma_abc_g1: vec![g1; 2], // 2 public inputs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_g1_accepts_generator() {
        let g1 = G1Affine::generator();
        assert!(validate_g1(&g1).is_ok());
    }

    #[test]
    fn validate_g1_accepts_identity() {
        let zero = G1Affine::zero();
        assert!(validate_g1(&zero).is_ok());
    }

    #[test]
    fn validate_g1_rejects_off_curve_point() {
        // Construct a point with arbitrary coordinates not on the curve.
        let x = Fq::from(1u64);
        let y = Fq::from(1u64);
        let bad = G1Affine::new_unchecked(x, y);
        assert!(validate_g1(&bad).is_err());
    }

    #[test]
    fn validate_g2_accepts_generator() {
        let g2 = G2Affine::generator();
        assert!(validate_g2(&g2).is_ok());
    }

    #[test]
    fn validate_g2_accepts_identity() {
        let zero = G2Affine::zero();
        assert!(validate_g2(&zero).is_ok());
    }

    #[test]
    fn validate_g2_rejects_off_curve_point() {
        let x = Fq2::new(Fq::from(1u64), Fq::from(1u64));
        let y = Fq2::new(Fq::from(1u64), Fq::from(1u64));
        let bad = G2Affine::new_unchecked(x, y);
        assert!(validate_g2(&bad).is_err());
    }

    #[test]
    fn parse_proof_rejects_off_curve_pi_a() {
        // Valid JSON structure but invalid curve point
        let json =
            r#"{"pi_a":["1","1","1"],"pi_b":[["1","0"],["0","1"],["1","0"]],"pi_c":["1","1","1"]}"#;
        let result = parse_proof(json);
        assert!(result.is_err(), "off-curve pi_a should be rejected");
    }

    #[test]
    fn parse_g2_rejects_short_inner_arrays() {
        let v: Vec<Vec<String>> = vec![vec!["1".to_string()], vec!["1".to_string()]];
        assert!(parse_g2(&v).is_err());
    }

    #[test]
    fn parse_g1_rejects_too_few_elements() {
        let v: Vec<String> = vec!["1".to_string()];
        assert!(parse_g1(&v).is_err());
    }

    #[test]
    fn parse_g2_rejects_too_few_elements() {
        let v: Vec<Vec<String>> = vec![vec!["1".to_string(), "0".to_string()]];
        assert!(parse_g2(&v).is_err());
    }

    #[test]
    fn parse_fr_from_hex_accepts_valid_field_element() {
        // Small value well within field order
        let hex = "0000000000000000000000000000000000000000000000000000000000000001";
        assert!(parse_fr_from_hex(hex).is_ok());
    }

    #[test]
    fn parse_fr_from_hex_rejects_value_exceeding_field_order() {
        // BN254 scalar field order is ~2^254. This is 2^256-1 (all ff bytes),
        // which exceeds the field order and would be silently reduced.
        let hex = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        assert!(
            parse_fr_from_hex(hex).is_err(),
            "values >= field modulus should be rejected"
        );
    }

    #[test]
    fn parse_fr_from_hex_rejects_oversized_input() {
        // 33 bytes — longer than 32 bytes
        let hex = "00ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        assert!(
            parse_fr_from_hex(hex).is_err(),
            "inputs > 32 bytes should be rejected"
        );
    }

    #[test]
    fn fr_to_canonical_hex_one_is_64_chars_ending_01() {
        let h = fr_to_canonical_hex(Fr::from(1u64));
        assert_eq!(h.len(), 64, "canonical hex must always be 64 characters");
        assert!(
            h.ends_with("01"),
            "Fr::from(1) canonical hex should end in '01': got {h}"
        );
        assert!(
            h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "canonical hex must be lowercase: got {h}"
        );
    }

    #[test]
    fn fr_to_canonical_hex_zero_is_64_zeros() {
        let h = fr_to_canonical_hex(Fr::from(0u64));
        assert_eq!(h.len(), 64);
        assert_eq!(h, "0".repeat(64));
    }

    #[test]
    fn fr_to_canonical_hex_roundtrips_via_canonical_parser() {
        for v in [0u64, 1, 7, 42, 0xdead_beef] {
            let fr = Fr::from(v);
            let h = fr_to_canonical_hex(fr);
            let back = parse_canonical_fr_hex(&h).expect("canonical hex must parse");
            assert_eq!(fr, back, "round-trip lost value {v}");
        }
    }

    #[test]
    fn parse_canonical_fr_hex_rejects_non_hex() {
        let bad = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        assert_eq!(bad.len(), 64);
        assert!(
            parse_canonical_fr_hex(bad).is_err(),
            "non-hex input must be rejected"
        );
    }

    #[test]
    fn parse_canonical_fr_hex_rejects_too_short() {
        let short = "abcd";
        assert!(
            parse_canonical_fr_hex(short).is_err(),
            "shorter than 64 chars must be rejected"
        );
    }

    #[test]
    fn parse_canonical_fr_hex_rejects_oversized() {
        let too_long = "0".repeat(65);
        assert!(
            parse_canonical_fr_hex(&too_long).is_err(),
            "longer than 64 chars must be rejected"
        );
        let way_too_long = "0".repeat(128);
        assert!(
            parse_canonical_fr_hex(&way_too_long).is_err(),
            "much-longer-than-64 must be rejected"
        );
    }

    #[test]
    fn parse_canonical_fr_hex_rejects_uppercase() {
        // Same value, but with uppercase A-F. Strict canonical refuses these
        // to keep DB / wire byte-comparisons unambiguous; callers that need
        // to accept mixed case must lowercase up front.
        let upper = "0000000000000000000000000000000000000000000000000000000000ABCDEF";
        assert!(
            parse_canonical_fr_hex(upper).is_err(),
            "uppercase must be rejected by the strict canonical parser"
        );
        // The legacy tolerant parser still accepts it.
        assert!(
            parse_fr_from_hex(upper).is_ok(),
            "legacy parser still accepts uppercase for backwards compat"
        );
    }

    #[test]
    fn parse_canonical_fr_hex_rejects_0x_prefix() {
        let prefixed = "0x00000000000000000000000000000000000000000000000000000000000000ab";
        assert_eq!(prefixed.len(), 66);
        assert!(
            parse_canonical_fr_hex(prefixed).is_err(),
            "0x prefix must be rejected"
        );
    }

    #[test]
    fn parse_canonical_fr_hex_rejects_value_above_modulus() {
        // 64 lowercase hex chars but the value >= BN254 scalar modulus.
        let above = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        assert!(
            parse_canonical_fr_hex(above).is_err(),
            "values >= field modulus must be rejected even when canonically encoded"
        );
    }

    #[test]
    fn parse_canonical_fr_hex_accepts_lowercase_64() {
        let ok = "0000000000000000000000000000000000000000000000000000000000000001";
        assert_eq!(ok.len(), 64);
        let fr = parse_canonical_fr_hex(ok).expect("lowercase 64-char hex must parse");
        assert_eq!(fr, Fr::from(1u64));
    }

    #[test]
    fn topic_hash_for_v2_is_deterministic() {
        let a = topic_hash_for_v2("annex:topic:test").expect("topic_hash should succeed");
        let b = topic_hash_for_v2("annex:topic:test").expect("topic_hash should succeed");
        assert_eq!(a, b, "same topic must produce the same hash");
    }

    #[test]
    fn topic_hash_for_v2_different_topics_yield_different_hashes() {
        let a = topic_hash_for_v2("annex:topic:alpha").expect("topic_hash should succeed");
        let b = topic_hash_for_v2("annex:topic:beta").expect("topic_hash should succeed");
        assert_ne!(
            a, b,
            "different topics must produce different hashes (privacy invariant)"
        );
    }

    #[test]
    fn topic_hash_for_v2_byte_sensitive() {
        let lower = topic_hash_for_v2("foo").expect("topic_hash should succeed");
        let upper = topic_hash_for_v2("FOO").expect("topic_hash should succeed");
        assert_ne!(lower, upper, "topic_hash is byte-sensitive by design");
    }

    #[test]
    fn topic_hash_for_v2_rejects_empty() {
        assert!(
            topic_hash_for_v2("").is_err(),
            "empty topic must be rejected"
        );
    }

    #[test]
    fn topic_hash_for_v2_outputs_canonical_64_char_hex() {
        let h = fr_to_canonical_hex(
            topic_hash_for_v2("annex:topic:test").expect("topic_hash should succeed"),
        );
        assert_eq!(h.len(), 64, "topic hash hex must be 64 chars");
        assert!(h
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    }

    #[test]
    fn topic_hash_for_v2_uses_domain_separator() {
        // Domain separator means the topic literally "annex/v2/topicHash:foo"
        // does NOT collide with topic "foo".
        let plain = topic_hash_for_v2("foo").expect("topic_hash should succeed");
        let prefix_collision =
            topic_hash_for_v2("annex/v2/topicHash:foo").expect("topic_hash should succeed");
        assert_ne!(
            plain, prefix_collision,
            "domain separator must not collide with a topic that happens to embed it"
        );
    }
}
