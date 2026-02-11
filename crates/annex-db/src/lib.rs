//! Database layer for the Annex platform.
//!
//! Provides SQLite connection pooling (via `r2d2`), WAL-mode initialization,
//! embedded SQL migrations, and query helpers. Every database table in Annex
//! is created through versioned migrations managed by this crate.
//!
//! # Design decisions
//!
//! - **SQLite with WAL mode**: chosen for single-server sovereignty — no
//!   external database process required. WAL mode allows concurrent readers
//!   with a single writer, which matches the Annex access pattern.
//! - **`r2d2` connection pool**: provides bounded connection reuse without
//!   manual lifetime management.
//! - **Embedded migrations**: SQL files are compiled into the binary via
//!   `include_str!`, ensuring migrations ship with the server and cannot
//!   drift from the code that depends on them.

mod migrations;
mod pool;

pub use migrations::run_migrations;
pub use pool::{create_pool, DbPool, DbRuntimeSettings};
