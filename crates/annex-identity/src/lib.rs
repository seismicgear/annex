//! Identity plane primitives for the Annex platform.
//!
//! This crate implements the first phase of the Annex identity plane with
//! deterministic, topic-scoped pseudonym derivation helpers.

use sha2::{Digest, Sha256};
use thiserror::Error;

pub mod commitment;
pub mod merkle;
pub mod poseidon;

pub use annex_types::RoleCode;
pub use ark_bn254::Fr;
pub use commitment::generate_commitment;
pub use merkle::MerkleTree;
pub use poseidon::hash_inputs;

/// Errors produced by identity derivation operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdentityError {
    /// The caller provided an empty commitment string.
    #[error("commitment hex cannot be empty")]
    EmptyCommitment,
    /// The caller provided an empty topic string.
    #[error("topic cannot be empty")]
    EmptyTopic,
    /// The caller provided an empty nullifier string.
    #[error("nullifier hex cannot be empty")]
    EmptyNullifier,
    /// The caller provided a nullifier that is not 64-char lowercase hex.
    #[error("nullifier hex must be 64 lowercase hex characters")]
    InvalidNullifierFormat,
    /// The caller provided a commitment that is not 64-char lowercase hex.
    #[error("commitment hex must be 64 lowercase hex characters")]
    InvalidCommitmentFormat,
    /// The input hex string is invalid.
    #[error("invalid hex string")]
    InvalidHex,
    /// The role code is invalid.
    #[error("invalid role code: {0}")]
    InvalidRoleCode(u8),
    /// Poseidon hashing failed.
    #[error("poseidon error: {0}")]
    PoseidonError(String),
    /// Merkle tree is full.
    #[error("merkle tree is full")]
    TreeFull,
    /// Invalid leaf index.
    #[error("invalid leaf index: {0}")]
    InvalidIndex(usize),
    /// Database error.
    #[error("database error: {0}")]
    DatabaseError(String),
}

/// Deterministically derives the nullifier hex for a commitment and topic.
///
/// Formula: `nullifierHex = sha256(commitmentHex + ":" + topic)`
///
/// # Errors
///
/// Returns [`IdentityError::EmptyCommitment`] if `commitment_hex` is empty.
/// Returns [`IdentityError::EmptyTopic`] if `topic` is empty.
/// Returns [`IdentityError::InvalidCommitmentFormat`] if `commitment_hex` is not
/// a 64-character lowercase hexadecimal string.
pub fn derive_nullifier_hex(commitment_hex: &str, topic: &str) -> Result<String, IdentityError> {
    if commitment_hex.is_empty() {
        return Err(IdentityError::EmptyCommitment);
    }
    if topic.is_empty() {
        return Err(IdentityError::EmptyTopic);
    }
    if !is_lower_hex_64(commitment_hex) {
        return Err(IdentityError::InvalidCommitmentFormat);
    }

    Ok(sha256_hex(&format!("{commitment_hex}:{topic}")))
}

/// Deterministically derives a pseudonym identifier from a topic and nullifier.
///
/// Formula: `pseudonymId = sha256(topic + ":" + nullifierHex)`
///
/// # Errors
///
/// Returns [`IdentityError::EmptyTopic`] if `topic` is empty.
/// Returns [`IdentityError::EmptyNullifier`] if `nullifier_hex` is empty.
/// Returns [`IdentityError::InvalidNullifierFormat`] if `nullifier_hex` is not
/// a 64-character lowercase hexadecimal string.
pub fn derive_pseudonym_id(topic: &str, nullifier_hex: &str) -> Result<String, IdentityError> {
    if topic.is_empty() {
        return Err(IdentityError::EmptyTopic);
    }
    if nullifier_hex.is_empty() {
        return Err(IdentityError::EmptyNullifier);
    }
    if !is_lower_hex_64(nullifier_hex) {
        return Err(IdentityError::InvalidNullifierFormat);
    }

    Ok(sha256_hex(&format!("{topic}:{nullifier_hex}")))
}

/// Computes a full topic-scoped pseudonym from a commitment.
///
/// This helper applies both roadmap formulas:
/// 1. `nullifierHex = sha256(commitmentHex + ":" + topic)`
/// 2. `pseudonymId = sha256(topic + ":" + nullifierHex)`
///
/// # Errors
///
/// Returns [`IdentityError::EmptyCommitment`] if `commitment_hex` is empty.
/// Returns [`IdentityError::EmptyTopic`] if `topic` is empty.
pub fn derive_topic_scoped_pseudonym(
    commitment_hex: &str,
    topic: &str,
) -> Result<String, IdentityError> {
    let nullifier_hex = derive_nullifier_hex(commitment_hex, topic)?;
    derive_pseudonym_id(topic, &nullifier_hex)
}

fn sha256_hex(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    hex::encode(digest)
}

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pseudonym_derivation_is_deterministic_for_same_input() {
        let commitment = "0000000000000000000000000000000000000000000000000000000000abc123";
        let topic = "annex:server:v1";

        let first = derive_topic_scoped_pseudonym(commitment, topic);
        let second = derive_topic_scoped_pseudonym(commitment, topic);

        assert!(first.is_ok());
        assert_eq!(first, second);
    }

    #[test]
    fn pseudonym_changes_across_topics() {
        let commitment = "0000000000000000000000000000000000000000000000000000000000abc123";

        let server = derive_topic_scoped_pseudonym(commitment, "annex:server:v1");
        let channel = derive_topic_scoped_pseudonym(commitment, "annex:channel:v1");

        assert!(server.is_ok());
        assert!(channel.is_ok());
        assert_ne!(server, channel);
    }

    #[test]
    fn returns_error_for_empty_inputs() {
        let valid_commitment = "0000000000000000000000000000000000000000000000000000000000abc123";
        assert_eq!(
            derive_topic_scoped_pseudonym("", "annex:server:v1"),
            Err(IdentityError::EmptyCommitment)
        );
        assert_eq!(
            derive_topic_scoped_pseudonym(valid_commitment, ""),
            Err(IdentityError::EmptyTopic)
        );
    }

    #[test]
    fn derive_pseudonym_id_rejects_empty_nullifier() {
        assert_eq!(
            derive_pseudonym_id("annex:server:v1", ""),
            Err(IdentityError::EmptyNullifier)
        );
    }

    #[test]
    fn derive_pseudonym_id_rejects_malformed_nullifier() {
        assert_eq!(
            derive_pseudonym_id("annex:server:v1", "not-a-hex-value"),
            Err(IdentityError::InvalidNullifierFormat)
        );
        assert_eq!(
            derive_pseudonym_id(
                "annex:server:v1",
                "ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789"
            ),
            Err(IdentityError::InvalidNullifierFormat)
        );
        assert_eq!(
            derive_pseudonym_id("annex:server:v1", "0123456789abcdef"),
            Err(IdentityError::InvalidNullifierFormat)
        );
    }

    #[test]
    fn derive_nullifier_hex_rejects_invalid_commitment() {
        assert_eq!(
            derive_nullifier_hex("invalid", "annex:server:v1"),
            Err(IdentityError::InvalidCommitmentFormat)
        );
        assert_eq!(
            derive_nullifier_hex("0xabc123", "annex:server:v1"),
            Err(IdentityError::InvalidCommitmentFormat)
        );
        assert_eq!(
            derive_nullifier_hex(
                "ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789",
                "annex:server:v1"
            ),
            Err(IdentityError::InvalidCommitmentFormat)
        );
    }

    #[test]
    fn derive_pseudonym_id_is_deterministic_for_valid_inputs() {
        let topic = "annex:server:v1";
        let nullifier = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        let first = derive_pseudonym_id(topic, nullifier);
        let second = derive_pseudonym_id(topic, nullifier);

        assert!(first.is_ok());
        assert_eq!(first, second);
    }
}
