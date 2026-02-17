//! Error types for the observability layer.

/// Errors that can occur during event log operations.
#[derive(Debug, thiserror::Error)]
pub enum ObserveError {
    /// A database operation failed.
    #[error("observe database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// JSON serialization or deserialization failed.
    #[error("observe serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}
