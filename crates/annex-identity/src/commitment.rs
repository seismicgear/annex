use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use crate::IdentityError;
use crate::poseidon::hash_inputs;

/// Roles defined in the system.
///
/// Human = 1, AiAgent = 2, Collective = 3, Bridge = 4, Service = 5
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RoleCode {
    Human = 1,
    AiAgent = 2,
    Collective = 3,
    Bridge = 4,
    Service = 5,
}

impl TryFrom<u8> for RoleCode {
    type Error = IdentityError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(RoleCode::Human),
            2 => Ok(RoleCode::AiAgent),
            3 => Ok(RoleCode::Collective),
            4 => Ok(RoleCode::Bridge),
            5 => Ok(RoleCode::Service),
            _ => Err(IdentityError::InvalidRoleCode(value)),
        }
    }
}

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
pub fn generate_commitment(sk_hex: &str, role: RoleCode, node_id: u64) -> Result<String, IdentityError> {
    // Parse sk_hex to bytes.
    let sk_bytes = hex::decode(sk_hex)
        .map_err(|_| IdentityError::InvalidHex(sk_hex.to_string()))?;

    // Convert to Fr. interpret bytes as big-endian integer.
    // If bytes length > 32 or value >= modulus, it's reduced modulo order.
    // Ideally, sk should be within field.
    let sk_fr = Fr::from_be_bytes_mod_order(&sk_bytes);

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

        let comm1 = generate_commitment(sk, role1, node_id).expect("should generate commitment for role 1");
        let comm2 = generate_commitment(sk, role2, node_id).expect("should generate commitment for role 2");

        assert_ne!(comm1, comm2);
    }

    #[test]
    fn test_invalid_sk_hex() {
        let role = RoleCode::Human;
        let node_id = 42;
        let err = generate_commitment("invalid-hex", role, node_id);
        assert!(matches!(err, Err(IdentityError::InvalidHex(_))), "Expected InvalidHex error, got {:?}", err);
    }
}
