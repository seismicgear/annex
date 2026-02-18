use crate::poseidon::hash_inputs;
use crate::IdentityError;
use annex_types::RoleCode;
use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};

/// Generates a Poseidon commitment: `Poseidon(sk, roleCode, nodeId)`.
///
/// Output is a BN254 scalar serialized as 32-byte big-endian hex string.
///
/// # Arguments
///
/// * `sk_hex`: The secret key as a big-endian hex string (without 0x prefix, or handled by hex::decode).
/// * `role`: The role of the identity.
/// * `node_id`: A unique node ID (u64).
///
/// # Errors
///
/// Returns [`IdentityError::InvalidHex`] if `sk_hex` is not valid hex.
/// Returns [`IdentityError::PoseidonError`] if hashing fails.
pub fn generate_commitment(
    sk_hex: &str,
    role: RoleCode,
    node_id: u64,
) -> Result<String, IdentityError> {
    // Parse sk_hex to bytes.
    let sk_bytes = hex::decode(sk_hex).map_err(|_| IdentityError::InvalidHex)?;

    // Reject keys that are too short (< 16 bytes = 128 bits) or too long (> 32 bytes).
    // Keys outside [16, 32] bytes are likely bugs rather than intentional.
    if sk_bytes.len() < 16 || sk_bytes.len() > 32 {
        return Err(IdentityError::InvalidHex);
    }

    // Convert to Fr and verify it was not silently reduced modulo the field order.
    // If the value is >= the BN254 scalar field modulus, from_be_bytes_mod_order
    // silently reduces it, which means two different secret keys would produce the
    // same commitment — a security-critical collision.
    let sk_fr = Fr::from_be_bytes_mod_order(&sk_bytes);
    let roundtrip_bytes = sk_fr.into_bigint().to_bytes_be();
    // Pad sk_bytes to 32 bytes for comparison
    let mut padded = vec![0u8; 32 - sk_bytes.len()];
    padded.extend_from_slice(&sk_bytes);
    if padded != roundtrip_bytes {
        return Err(IdentityError::InvalidHex);
    }

    let role_fr = Fr::from(role as u8);
    let node_id_fr = Fr::from(node_id);

    // Hash inputs: [sk, role, nodeId]
    let commitment_fr = hash_inputs(&[sk_fr, role_fr, node_id_fr])?;

    // Convert commitment (Fr) to 32-byte big-endian hex string.
    // into_bigint returns BigInteger256 (little-endian usually in memory), but to_bytes_be produces big-endian bytes.
    let commitment_bytes = commitment_fr.into_bigint().to_bytes_be();
    Ok(hex::encode(commitment_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_commitment_generation() {
        // Valid 32-byte hex string
        let sk = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
        let role = RoleCode::Human;
        let node_id = 42;

        let comm1 = generate_commitment(sk, role, node_id).expect("should generate commitment 1");
        let comm2 = generate_commitment(sk, role, node_id).expect("should generate commitment 2");

        assert_eq!(comm1, comm2);
        assert_eq!(comm1.len(), 64); // 32 bytes hex * 2 chars/byte = 64
    }

    #[test]
    fn test_commitment_differs_for_diff_inputs() {
        let sk = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
        let role1 = RoleCode::Human;
        let role2 = RoleCode::AiAgent;
        let node_id = 42;

        let comm1 =
            generate_commitment(sk, role1, node_id).expect("should generate commitment for role 1");
        let comm2 =
            generate_commitment(sk, role2, node_id).expect("should generate commitment for role 2");

        assert_ne!(comm1, comm2);
    }

    #[test]
    fn test_invalid_sk_hex() {
        let role = RoleCode::Human;
        let node_id = 42;
        let err = generate_commitment("invalid-hex", role, node_id);
        assert!(
            matches!(err, Err(IdentityError::InvalidHex)),
            "Expected InvalidHex error, got {:?}",
            err
        );
    }

    #[test]
    fn test_secret_key_too_short() {
        // 8 bytes = 16 hex chars, below the 16-byte minimum
        let sk = "0102030405060708";
        let err = generate_commitment(sk, RoleCode::Human, 42);
        assert!(
            matches!(err, Err(IdentityError::InvalidHex)),
            "Expected InvalidHex for too-short key, got {:?}",
            err
        );
    }

    #[test]
    fn test_secret_key_too_long() {
        // 33 bytes = 66 hex chars, above the 32-byte maximum
        let sk = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f2021";
        let err = generate_commitment(sk, RoleCode::Human, 42);
        assert!(
            matches!(err, Err(IdentityError::InvalidHex)),
            "Expected InvalidHex for too-long key, got {:?}",
            err
        );
    }

    #[test]
    fn test_secret_key_above_field_modulus_rejected() {
        // BN254 scalar field modulus is ~2^254. An all-0xFF 32-byte value exceeds it.
        let sk = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        let err = generate_commitment(sk, RoleCode::Human, 42);
        assert!(
            matches!(err, Err(IdentityError::InvalidHex)),
            "Expected InvalidHex for sk >= field modulus, got {:?}",
            err
        );
    }

    #[test]
    fn test_secret_key_16_bytes_accepted() {
        // Exactly 16 bytes (minimum length)
        let sk = "0102030405060708090a0b0c0d0e0f10";
        let result = generate_commitment(sk, RoleCode::Human, 42);
        assert!(result.is_ok(), "16-byte key should be accepted");
    }
}
