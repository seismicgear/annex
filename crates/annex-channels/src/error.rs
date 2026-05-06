//! Error type returned by every public function in this crate.

use thiserror::Error;

/// Errors that can occur during channel operations.
#[derive(Debug, Error)]
pub enum ChannelError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("channel not found: {0}")]
    NotFound(String),
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),
}
