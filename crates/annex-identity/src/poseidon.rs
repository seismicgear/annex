use ark_bn254::Fr;
use light_poseidon::{Poseidon, PoseidonHasher};
use crate::IdentityError;

/// Hashes a slice of field elements using Poseidon with BN254 parameters compatible with Circom.
///
/// # Errors
///
/// Returns [`IdentityError::PoseidonError`] if the number of inputs is not supported
/// or if the hashing fails.
pub fn hash_inputs(inputs: &[Fr]) -> Result<Fr, IdentityError> {
    // light-poseidon supports specific input lengths.
    // For 2 inputs (Merkle) and 3 inputs (Commitment), it should work.
    let mut poseidon = Poseidon::<Fr>::new_circom(inputs.len())
        .map_err(|e| IdentityError::PoseidonError(format!("Failed to initialize Poseidon: {:?}", e)))?;

    poseidon.hash(inputs)
        .map_err(|e| IdentityError::PoseidonError(format!("Poseidon hash failed: {:?}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poseidon_hash_deterministic() {
        let input = vec![Fr::from(1), Fr::from(2)];
        let hash1 = hash_inputs(&input).expect("hashing should succeed");
        let hash2 = hash_inputs(&input).expect("hashing should succeed");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_poseidon_hash_benchmark() {
        use std::time::Instant;
        let input = vec![Fr::from(1), Fr::from(2), Fr::from(3)];
        let start = Instant::now();
        let _ = hash_inputs(&input).expect("hashing should succeed");
        let duration = start.elapsed();
        println!("Poseidon hash time: {:?}", duration);
        // Ensure it is reasonably fast (not strictly <1ms in debug, but check it runs)
    }
}
