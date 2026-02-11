//! Presence graph for the Annex platform.
//!
//! Implements the live presence graph: graph nodes (participants), edges
//! (relationships), BFS degree-of-separation queries, visibility rules,
//! SSE presence streaming, and activity-based pruning.
//!
//! The presence graph is how Annex represents who is connected, how they
//! relate to each other, and what they can see. Visibility is degree-based:
//! a participant at degree 1 sees more than a participant at degree 3.
//!
//! # Phase 5 implementation
//!
//! The full implementation of this crate is Phase 5 of the roadmap.
