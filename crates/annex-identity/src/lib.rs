//! Identity plane primitives for the Annex platform.
//!
//! This crate implements the first phase of the Annex identity plane with
//! deterministic, topic-scoped pseudonym derivation helpers.

use sha2::{Digest, Sha256};
use thiserror::Error;

/// Errors produced by identity derivation operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdentityError {
    /// The caller provided an empty commitment string.
    #[error("commitment hex cannot be empty")]
    EmptyCommitment,
    /// The caller provided an empty topic string.
    #[error("topic cannot be empty")]
    EmptyTopic,
}

/// Deterministically derives the nullifier hex for a commitment and topic.
///
/// Formula: `nullifierHex = sha256(commitmentHex + ":" + topic)`
///
/// # Errors
///
/// Returns [`IdentityError::EmptyCommitment`] if `commitment_hex` is empty.
/// Returns [`IdentityError::EmptyTopic`] if `topic` is empty.
pub fn derive_nullifier_hex(commitment_hex: &str, topic: &str) -> Result<String, IdentityError> {
    if commitment_hex.is_empty() {
        return Err(IdentityError::EmptyCommitment);
    }
    if topic.is_empty() {
        return Err(IdentityError::EmptyTopic);
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
pub fn derive_pseudonym_id(topic: &str, nullifier_hex: &str) -> Result<String, IdentityError> {
    if topic.is_empty() {
        return Err(IdentityError::EmptyTopic);
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
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push(hex_char((byte >> 4) & 0x0f));
        hex.push(hex_char(byte & 0x0f));
    }
    hex
}

fn hex_char(nibble: u8) -> char {
    const HEX: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    HEX[(nibble & 0x0f) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pseudonym_derivation_is_deterministic_for_same_input() {
        let commitment = "0xabc123";
        let topic = "annex:server:v1";

        let first = derive_topic_scoped_pseudonym(commitment, topic);
        let second = derive_topic_scoped_pseudonym(commitment, topic);

        assert!(first.is_ok());
        assert_eq!(first, second);
    }

    #[test]
    fn pseudonym_changes_across_topics() {
        let commitment = "0xabc123";

        let server = derive_topic_scoped_pseudonym(commitment, "annex:server:v1");
        let channel = derive_topic_scoped_pseudonym(commitment, "annex:channel:v1");

        assert!(server.is_ok());
        assert!(channel.is_ok());
        assert_ne!(server, channel);
    }

    #[test]
    fn returns_error_for_empty_inputs() {
        assert_eq!(
            derive_topic_scoped_pseudonym("", "annex:server:v1"),
            Err(IdentityError::EmptyCommitment)
        );
        assert_eq!(
            derive_topic_scoped_pseudonym("0xabc123", ""),
            Err(IdentityError::EmptyTopic)
        );
    }
}
