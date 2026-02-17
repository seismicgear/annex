//! Error types for the RTX knowledge exchange system.

/// Errors that can occur during RTX bundle operations.
#[derive(Debug, thiserror::Error)]
pub enum RtxError {
    /// Transfer denied due to VRP transfer scope restrictions.
    #[error("RTX transfer denied: {0}")]
    TransferDenied(String),

    /// Bundle contains a redacted topic that the sender is prohibited from sharing.
    #[error("RTX redacted topic: {0}")]
    RedactedTopic(String),

    /// Bundle structure is invalid (missing or empty required fields).
    #[error("RTX invalid bundle: {0}")]
    InvalidBundle(String),

    /// Signature verification failed.
    #[error("RTX signature verification failed: {0}")]
    InvalidSignature(String),

    /// The sender does not have an active agent registration.
    #[error("RTX sender not registered: {0}")]
    SenderNotRegistered(String),

    /// No active federation agreement permits this relay.
    #[error("RTX federation not authorized: {0}")]
    FederationNotAuthorized(String),

    /// Database error during RTX operations.
    #[error("RTX database error: {0}")]
    Database(String),
}
