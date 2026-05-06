//! HTTP-level building blocks: CORS, static-file serving, and the global
//! middleware layer chain. These are kept separate from the route table
//! (`crate::routes`) so route changes and layer/middleware changes can move
//! independently.

pub mod cors;
pub mod layers;
pub mod static_files;
