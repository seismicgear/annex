//! Service layer for the annex-server crate.
//!
//! Each service module owns one orchestration concern and exposes async
//! methods that take parsed request DTOs and return parsed response DTOs.
//! HTTP handlers in `api.rs` are intentionally thin: extract the shared
//! state, parse the request body, call the service, and map the typed
//! result back to JSON. Everything that touches the DB pool, the Merkle
//! lock, the ZK verifier, the `vrp_*` tables, the platform_identities
//! row, the graph node, the observe bus, or the presence broadcast bus
//! lives below this line.
//!
//! The split exists for two reasons:
//!   1. Handler files were drifting toward 1000 LOC of intertwined HTTP
//!      and storage code, which made the policy-critical orchestration
//!      hard to audit.
//!   2. Service methods can be unit-tested without standing up an axum
//!      router, an HTTP client, or a serde round-trip.

use crate::api::ApiError;
pub use channel_service::{ChannelService, ChannelServiceError};
pub use federation_service::{FederationError, FederationService};
pub use identity_service::{IdentityService, IdentityServiceError};

pub mod channel_service;
pub mod federation_repository;
pub mod federation_service;
pub mod identity_service;

/// Map an [`IdentityServiceError`] into the wire-facing [`ApiError`] used
/// by the axum handlers. Defined here (and not on `IdentityServiceError`
/// directly) so the service layer stays free of the HTTP-status type.
impl From<IdentityServiceError> for ApiError {
    fn from(err: IdentityServiceError) -> Self {
        match err {
            IdentityServiceError::BadRequest(msg) => ApiError::BadRequest(msg),
            IdentityServiceError::Forbidden(msg) => ApiError::Forbidden(msg),
            IdentityServiceError::NotFound(msg) => ApiError::NotFound(msg),
            IdentityServiceError::Conflict(msg) => ApiError::Conflict(msg),
            IdentityServiceError::Unauthorized(msg) => ApiError::Unauthorized(msg),
            IdentityServiceError::Internal(msg) => ApiError::InternalServerError(msg),
        }
    }
}

/// Map a [`ChannelServiceError`] into the wire-facing [`ApiError`].
///
/// Note: the channel HTTP handlers in `api_channels.rs` historically
/// returned bare `StatusCode` (with empty body), and the voice handlers
/// shipped a structured `{error, message, setup_hint}` body for the
/// `VoiceDisabled` / `VoiceNotConfigured` cases. To preserve those wire
/// shapes the handlers use [`ChannelServiceError::status_code`] (and
/// match those two variants explicitly) rather than this `From` impl.
/// The impl exists so future call sites that want the standard
/// `{"error": …}` body can opt in.
impl From<ChannelServiceError> for ApiError {
    fn from(err: ChannelServiceError) -> Self {
        match err {
            ChannelServiceError::BadRequest(msg) => ApiError::BadRequest(msg),
            ChannelServiceError::Forbidden(msg) => ApiError::Forbidden(msg),
            ChannelServiceError::NotFound(msg) => ApiError::NotFound(msg),
            ChannelServiceError::Conflict(msg) => ApiError::Conflict(msg),
            ChannelServiceError::ServiceUnavailable(msg) => ApiError::InternalServerError(msg),
            ChannelServiceError::VoiceDisabled => ApiError::Forbidden("voice disabled".to_string()),
            ChannelServiceError::VoiceNotConfigured => {
                ApiError::InternalServerError("voice not configured".to_string())
            }
            ChannelServiceError::Internal(msg) => ApiError::InternalServerError(msg),
        }
    }
}
