//! Channel model and text communication for the Annex platform.
//!
//! Implements channel CRUD, message persistence, WebSocket real-time
//! delivery, message history retrieval, and retention policy enforcement.
//!
//! Channels are the primary communication primitive in Annex. They support
//! multiple types (`Text`, `Voice`, `Hybrid`, `Agent`, `Broadcast`), each
//! with distinct capability requirements and federation scoping.
//!
//! # Phase 4 implementation
//!
//! The full implementation of this crate is Phase 4 of the roadmap.
