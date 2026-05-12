//! Identity plane primitives for the Annex platform.
//!
//! This crate implements the first phase of the Annex identity plane with
//! deterministic, topic-scoped pseudonym derivation helpers.

pub use annex_types::RoleCode;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub mod commitment;
pub mod merkle;
pub mod nullifier;
pub mod platform;
pub mod poseidon;
pub mod registry;
pub mod zk;

pub use commitment::generate_commitment;
pub use merkle::MerkleTree;
pub use nullifier::{check_nullifier_exists, insert_nullifier};
pub use platform::{
    create_platform_identity, deactivate_platform_identity, ensure_founder, get_platform_identity,
    update_capabilities, Capabilities, PlatformIdentity,
};
pub use poseidon::hash_inputs;
pub use registry::{
    get_all_roles, get_all_topics, get_path_for_commitment, register_identity, RegistrationResult,
    VrpRoleEntry, VrpTopic,
};

/// Errors produced by identity derivation operations.
#[derive(Debug, Error)]
pub enum IdentityError {
    /// The caller provided an empty commitment string.
    #[error("commitment hex cannot be empty")]
    EmptyCommitment,
    /// The caller provided an empty topic string.
    #[error("topic cannot be empty")]
    EmptyTopic,
    /// The caller provided a topic string exceeding the maximum length.
    #[error("topic exceeds maximum length of {0} bytes")]
    TopicTooLong(usize),
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
    /// The role label is invalid.
    #[error("invalid role label: {0}")]
    InvalidRoleLabel(String),
    /// Poseidon hashing failed.
    #[error("poseidon error: {0}")]
    PoseidonError(String),
    /// Merkle tree is full.
    #[error("merkle tree is full")]
    TreeFull,
    /// Invalid leaf index.
    #[error("invalid leaf index: {0}")]
    InvalidIndex(usize),
    /// Nullifier already exists for this topic.
    #[error("nullifier already exists for topic '{0}'")]
    DuplicateNullifier(String),
    /// Commitment already registered.
    #[error("duplicate commitment: {0}")]
    DuplicateCommitment(String),
    /// Commitment not found in the registry.
    #[error("commitment not found: {0}")]
    CommitmentNotFound(String),
    /// Merkle root mismatch between stored and computed values.
    #[error("merkle root mismatch: stored={stored}, computed={computed}")]
    MerkleRootMismatch { stored: String, computed: String },
    /// Persisted Merkle tree depth differs from the depth requested at boot.
    /// Refusing to silently re-shard a tree of identities into a tree of a
    /// different size — that would invalidate every previously-issued proof.
    #[error("merkle tree depth mismatch: persisted={stored}, configured={configured}")]
    MerkleTreeDepthMismatch { stored: usize, configured: usize },
    /// Database error.
    #[error("database error: {0}")]
    DatabaseError(#[from] rusqlite::Error),
}

impl PartialEq for IdentityError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::EmptyCommitment, Self::EmptyCommitment) => true,
            (Self::EmptyTopic, Self::EmptyTopic) => true,
            (Self::TopicTooLong(a), Self::TopicTooLong(b)) => a == b,
            (Self::EmptyNullifier, Self::EmptyNullifier) => true,
            (Self::InvalidNullifierFormat, Self::InvalidNullifierFormat) => true,
            (Self::InvalidCommitmentFormat, Self::InvalidCommitmentFormat) => true,
            (Self::InvalidHex, Self::InvalidHex) => true,
            (Self::InvalidRoleCode(a), Self::InvalidRoleCode(b)) => a == b,
            (Self::InvalidRoleLabel(a), Self::InvalidRoleLabel(b)) => a == b,
            (Self::PoseidonError(a), Self::PoseidonError(b)) => a == b,
            (Self::TreeFull, Self::TreeFull) => true,
            (Self::InvalidIndex(a), Self::InvalidIndex(b)) => a == b,
            (Self::DuplicateNullifier(a), Self::DuplicateNullifier(b)) => a == b,
            (Self::DuplicateCommitment(a), Self::DuplicateCommitment(b)) => a == b,
            (Self::CommitmentNotFound(a), Self::CommitmentNotFound(b)) => a == b,
            (
                Self::MerkleRootMismatch {
                    stored: s1,
                    computed: c1,
                },
                Self::MerkleRootMismatch {
                    stored: s2,
                    computed: c2,
                },
            ) => s1 == s2 && c1 == c2,
            (
                Self::MerkleTreeDepthMismatch {
                    stored: s1,
                    configured: c1,
                },
                Self::MerkleTreeDepthMismatch {
                    stored: s2,
                    configured: c2,
                },
            ) => s1 == s2 && c1 == c2,
            (Self::DatabaseError(a), Self::DatabaseError(b)) => a.to_string() == b.to_string(),
            _ => false,
        }
    }
}

impl Eq for IdentityError {}

/// Deterministically derives the nullifier hex for a v1 commitment and topic.
///
/// Formula: `nullifierHex = sha256(commitmentHex + ":" + topic)`
///
/// # PRIVACY LIMITATION (v1 only)
///
/// Because the inputs are the **public** commitment and the **public** topic,
/// any external observer who has seen the commitment (it appears in
/// `/api/registry/path`, federation handshakes, public agent listings, the
/// observe stream, and channel listings) can compute the same nullifier for
/// any topic. That means the nullifier — and therefore the topic-scoped
/// pseudonym derived from it — does NOT hide the link between a
/// commitment and the topic-specific identity, and a censor who knows your
/// commitment can enumerate all of your topic pseudonyms across the network.
///
/// The v2 protocol path closes this gap: v2 binds the nullifier to a secret
/// witnessed inside the membership circuit (see
/// `zk/circuits/membership_v2.circom` + `annex_identity::zk::topic_hash_for_v2`),
/// so external observers cannot recompute it. v2 is opt-in via
/// `Config::security.enabled_zk_versions`. Servers that need the privacy
/// property should enable v2 and migrate clients off the v1 path.
///
/// This function is the v1 derivation and intentionally preserves the
/// public-derivable property for compatibility with v1 clients.
///
/// # Errors
///
/// Returns [`IdentityError::EmptyCommitment`] if `commitment_hex` is empty.
/// Returns [`IdentityError::EmptyTopic`] if `topic` is empty.
/// Returns [`IdentityError::InvalidCommitmentFormat`] if `commitment_hex` is not
/// a 64-character lowercase hexadecimal string.
/// Maximum allowed topic length in bytes.
const MAX_TOPIC_LEN: usize = 256;

pub fn derive_nullifier_hex(commitment_hex: &str, topic: &str) -> Result<String, IdentityError> {
    if commitment_hex.is_empty() {
        return Err(IdentityError::EmptyCommitment);
    }
    if topic.is_empty() {
        return Err(IdentityError::EmptyTopic);
    }
    if topic.len() > MAX_TOPIC_LEN {
        return Err(IdentityError::TopicTooLong(MAX_TOPIC_LEN));
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
    if topic.len() > MAX_TOPIC_LEN {
        return Err(IdentityError::TopicTooLong(MAX_TOPIC_LEN));
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

    #[test]
    fn derive_nullifier_hex_rejects_topic_exceeding_max_length() {
        let commitment = "0000000000000000000000000000000000000000000000000000000000abc123";
        let long_topic = "a".repeat(257);
        assert_eq!(
            derive_nullifier_hex(commitment, &long_topic),
            Err(IdentityError::TopicTooLong(MAX_TOPIC_LEN))
        );
    }

    #[test]
    fn derive_pseudonym_id_rejects_topic_exceeding_max_length() {
        let nullifier = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let long_topic = "a".repeat(257);
        assert_eq!(
            derive_pseudonym_id(&long_topic, nullifier),
            Err(IdentityError::TopicTooLong(MAX_TOPIC_LEN))
        );
    }

    #[test]
    fn derive_nullifier_hex_accepts_max_length_topic() {
        let commitment = "0000000000000000000000000000000000000000000000000000000000abc123";
        let max_topic = "a".repeat(256);
        assert!(derive_nullifier_hex(commitment, &max_topic).is_ok());
    }

    #[test]
    fn v1_nullifier_is_publicly_derivable_from_commitment() {
        // **DOCUMENTATION TEST — INTENTIONAL PROPERTY OF v1.**
        //
        // The v1 nullifier is `sha256(commitment_hex + ":" + topic)`. Both
        // inputs are public — the commitment is exposed by every API surface
        // that returns a Merkle path / federated identity / observe event,
        // and the topic is part of the request URI. So any external observer
        // can recompute the nullifier (and therefore the topic-scoped
        // pseudonym) for any (commitment, topic) pair they observe.
        //
        // Concretely: if Eve sees Alice's commitment, Eve can enumerate
        // Alice's per-topic pseudonyms across the entire network and de-
        // anonymise her cross-topic activity. v1 does NOT provide unlinkable
        // topic identities.
        //
        // The v2 protocol (`zk/circuits/membership_v2.circom` +
        // `annex_identity::zk::topic_hash_for_v2`) closes this gap by binding
        // the nullifier to a SECRET inside the membership circuit; external
        // observers can no longer recompute it. v2 is opt-in via
        // `Config::security.enabled_zk_versions = ["v1", "v2"]` and is the
        // recommended posture for any deployment that needs topic
        // unlinkability.
        //
        // This test exists so a reader of the codebase finds the property
        // documented in code, not just in docs/issue trackers. If a future
        // refactor accidentally swaps in a secret-based v1 nullifier, this
        // test will fail loudly and the engineer will need to choose
        // explicitly between (a) introducing a wire break (delete this test
        // and update the v1 spec) or (b) restoring the documented v1
        // semantics.
        let commitment = "1111111111111111111111111111111111111111111111111111111111111111";
        let topic = "annex:server:v1";

        // Property 1: deterministic. Same (commitment, topic) → same nullifier.
        let n_first = derive_nullifier_hex(commitment, topic).unwrap();
        let n_second = derive_nullifier_hex(commitment, topic).unwrap();
        assert_eq!(
            n_first, n_second,
            "v1 nullifier derivation must be deterministic for replay-safety"
        );

        // Property 2: PUBLIC. Recomputing it requires only public inputs.
        let recomputed = sha256_hex(&format!("{commitment}:{topic}"));
        assert_eq!(
            n_first, recomputed,
            "v1 nullifier MUST equal sha256(commitment + ':' + topic) — \
             this is the documented (and privacy-limiting) v1 property; \
             switch to v2 for secret-derived nullifiers."
        );

        // Property 3: per-topic (so cross-topic linkability requires
        // computing the formula per topic, not free).
        let other = derive_nullifier_hex(commitment, "annex:channel:v1").unwrap();
        assert_ne!(
            n_first, other,
            "v1 nullifier must vary across topics so per-topic pseudonyms differ"
        );
    }
}
