//! Federation layer for the Annex platform.
//!
//! Implements server-to-server VRP handshakes, bilateral trust agreements,
//! cross-server identity attestation, federated channel management, and
//! message relay with cryptographic verification.
//!
//! Federation in Annex is sovereign and bilateral: each server independently
//! negotiates trust with each peer via VRP. There is no central registry,
//! no global authority, and no implicit trust. Policy changes on either
//! side trigger automatic re-evaluation of the federation agreement.
//!
//! # Phase 8 implementation
//!
//! The full implementation of this crate is Phase 8 of the roadmap.

pub mod db;
pub mod handshake;
pub mod types;

pub use db::{create_agreement, get_agreement};
pub use handshake::{process_incoming_handshake, HandshakeError};
pub use types::{
    AttestationRequest, FederatedMessageEnvelope, FederatedRtxEnvelope, FederationAgreement,
};
