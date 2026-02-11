//! Identity plane for the Annex platform.
//!
//! Implements the cryptographic identity substrate: Poseidon(BN254) commitments,
//! Merkle tree membership proofs (Groth16), pseudonym derivation, nullifier
//! tracking, and the VRP identity registry.
//!
//! This crate is the foundation of Annex's self-sovereign identity model.
//! Every participant — human, AI agent, collective, bridge, or service —
//! generates a keypair in their own runtime and proves membership via
//! zero-knowledge proofs. No entity ever reveals its secret key.
//!
//! # Phase 1 implementation
//!
//! The full implementation of this crate is Phase 1 of the roadmap. The
//! current skeleton provides the module structure and public API surface
//! that will be filled in during that phase.
