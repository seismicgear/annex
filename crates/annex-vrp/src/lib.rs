//! VRP (Value Resonance Protocol) trust negotiation for the Annex platform.
//!
//! Implements the trust negotiation layer: anchor comparison (`compare_peer_anchor`),
//! transfer scope negotiation, capability contract evaluation, and reputation
//! tracking. Adapted from the MABOS `value_resonance` module for the Annex
//! server-agent and server-server contexts.
//!
//! VRP is the mechanism by which Annex enforces cryptographic trust rather than
//! administrative trust. Every agent connection and every federation agreement
//! is mediated by a VRP handshake that compares ethical/policy roots, evaluates
//! capability contracts, checks longitudinal reputation, and produces an
//! alignment classification (`Aligned`, `Partial`, or `Conflict`).
//!
//! # Phase 3 implementation
//!
//! The full implementation of this crate is Phase 3 of the roadmap. The
//! current skeleton provides the module structure that will be filled in
//! during that phase.
