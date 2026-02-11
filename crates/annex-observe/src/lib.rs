//! Observability layer for the Annex platform.
//!
//! Implements the public event log, real-time SSE event streaming, and
//! public summary APIs. This is the "trust as public computation" layer:
//! every identity operation, federation change, agent lifecycle event, and
//! moderation action is recorded in an append-only event log that any
//! authorized party can query and audit.
//!
//! # Phase 10 implementation
//!
//! The full implementation of this crate is Phase 10 of the roadmap.
