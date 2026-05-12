//! API handlers for the Annex server.
//!
//! Handlers in this file follow the rule documented in
//! `crates/annex-server/src/services/mod.rs`: each one
//!   1. extracts `Extension<Arc<AppState>>`,
//!   2. accepts a parsed request body / path parameter,
//!   3. delegates to a service in `crate::services::*`,
//!   4. wraps the typed response in `Json(...)`.
//!
//! Storage / Merkle / ZK / observe-bus orchestration lives in the
//! services module. See `IdentityService` for the
//! register / path / current-root / verify-membership flows that used
//! to live inline in this file.

use crate::AppState;
use annex_identity::{
    ensure_founder, get_all_roles, get_all_topics, get_platform_identity, Capabilities,
    PlatformIdentity, RoleCode, VrpRoleEntry, VrpTopic,
};
use axum::{
    extract::{Extension, Json, Path},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

/// Request body for identity registration.
#[derive(Debug, Clone, Deserialize)]
pub struct RegisterRequest {
    /// The identity commitment (64-char hex string).
    #[serde(rename = "commitmentHex")]
    pub commitment_hex: String,
    /// The role code of the participant (1..=5).
    #[serde(rename = "roleCode")]
    pub role_code: u8,
    /// The node ID used in the commitment derivation.
    #[serde(rename = "nodeId")]
    pub node_id: i64,
    /// Optional invite code for invite-only servers.
    #[serde(default, rename = "inviteCode")]
    pub invite_code: Option<String>,
    /// Optional server password for password-protected servers.
    #[serde(default, rename = "serverPassword")]
    pub server_password: Option<String>,
}

/// Response body for successful registration.
#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    /// The assigned database ID for the identity.
    #[serde(rename = "identityId")]
    pub identity_id: i64,
    /// The assigned Merkle tree leaf index.
    #[serde(rename = "leafIndex")]
    pub leaf_index: usize,
    /// The new Merkle root (hex string).
    #[serde(rename = "rootHex")]
    pub root_hex: String,
    /// The Merkle path elements (hex strings) for proof generation.
    #[serde(rename = "pathElements")]
    pub path_elements: Vec<String>,
    /// The Merkle path indices (0 or 1).
    #[serde(rename = "pathIndexBits")]
    pub path_indices: Vec<u8>,
}

/// Response body for Merkle path retrieval.
#[derive(Debug, Serialize, Deserialize)]
pub struct GetPathResponse {
    /// The Merkle tree leaf index.
    #[serde(rename = "leafIndex")]
    pub leaf_index: usize,
    /// The current Merkle root (hex string).
    #[serde(rename = "rootHex")]
    pub root_hex: String,
    /// The Merkle path elements (hex strings).
    #[serde(rename = "pathElements")]
    pub path_elements: Vec<String>,
    /// The Merkle path indices (0 or 1).
    #[serde(rename = "pathIndexBits")]
    pub path_indices: Vec<u8>,
}

/// Response body for current root retrieval.
#[derive(Debug, Serialize, Deserialize)]
pub struct GetRootResponse {
    /// The current Merkle root (hex string).
    #[serde(rename = "rootHex")]
    pub root_hex: String,
    /// The number of leaves currently in the tree.
    #[serde(rename = "leafCount")]
    pub leaf_count: usize,
    /// Timestamp when this root was created (if persisted).
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
}

/// Request body for ZK membership verification.
///
/// Note on privacy: This endpoint requires the public identity commitment to be
/// submitted alongside the proof. This allows the server to verify that the
/// proof corresponds to the claimed identity (via public signals) and to derive
/// the deterministic pseudonym. While the proof demonstrates membership in the
/// Merkle tree without revealing the private key or Merkle path to *observers*
/// of the proof alone, the server here acts as the verifier and issuer of the
/// topic-scoped pseudonym, and thus learns the mapping between commitment and
/// pseudonym for this interaction. This is consistent with the Phase 1 identity model.
#[derive(Debug, Deserialize)]
pub struct VerifyMembershipRequest {
    /// The Merkle root against which the proof was generated.
    pub root: String,
    /// The identity commitment.
    pub commitment: String,
    /// The topic for which the pseudonym is being derived.
    pub topic: String,
    /// The Groth16 proof (JSON object).
    pub proof: serde_json::Value,
    /// The public signals (array of strings).
    ///
    /// v1: ordering is `[root, commitment]` (length 2).
    /// v2: ordering is `[root, commitment, nullifier, topicHash]` (length 4).
    #[serde(rename = "publicSignals")]
    pub public_signals: Vec<String>,

    /// Membership-circuit version this proof was produced for.
    ///
    /// `None` or `Some("v1")` selects the legacy v1 verifier (commitment-derived
    /// nullifier). `Some("v2")` selects the secret-derived nullifier verifier and
    /// requires the v2 vkey to be loaded (i.e. `"v2"` in
    /// `Config::security.enabled_zk_versions`).
    ///
    /// Any other value is rejected with `400 Bad Request`. The server never
    /// silently downgrades or upgrades a proof's protocol version.
    #[serde(rename = "protocolVersion", default)]
    pub protocol_version: Option<String>,

    /// v2-only: claimed nullifier (hex). When present and `protocol_version`
    /// is `"v2"`, the server checks that this matches `public_signals[2]`
    /// after canonicalisation, so an attacker cannot swap the nullifier in
    /// the response without producing a fresh valid proof. Optional for v1.
    #[serde(rename = "nullifierHex", default)]
    pub nullifier_hex: Option<String>,

    /// v2-only: the topicHash (hex BN254 scalar) the proof was produced for.
    /// When `protocol_version` is `"v2"`, this is the value the verifier
    /// passed in as the public input; the server checks that
    /// `public_signals[3]` matches.
    #[serde(rename = "topicHashHex", default)]
    pub topic_hash_hex: Option<String>,
}

/// Response body for successful membership verification.
#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyMembershipResponse {
    /// Whether verification succeeded.
    pub ok: bool,
    /// The derived pseudonym ID.
    #[serde(rename = "pseudonymId")]
    pub pseudonym_id: String,
    /// HMAC-signed session token for authenticated API calls.
    /// Clients must send this as `Authorization: Bearer <token>`.
    #[serde(rename = "sessionToken")]
    pub session_token: String,
}

/// Response body for identity query.
#[derive(Debug, Serialize, Deserialize)]
pub struct GetIdentityResponse {
    /// The pseudonym ID.
    #[serde(rename = "pseudonymId")]
    pub pseudonym_id: String,
    /// The participant type (role).
    #[serde(rename = "participantType")]
    pub participant_type: RoleCode,
    /// Whether the identity is active.
    pub active: bool,
    /// Capability flags.
    pub capabilities: Capabilities,
}

/// Response body for identity capabilities query.
#[derive(Debug, Serialize, Deserialize)]
pub struct GetCapabilitiesResponse {
    /// Capability flags.
    pub capabilities: Capabilities,
}

/// API error type mapping to HTTP status codes.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("invalid input: {0}")]
    BadRequest(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("internal server error: {0}")]
    InternalServerError(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            ApiError::InternalServerError(msg) => {
                // Log the real error server-side but return a generic message
                // to the client to prevent leaking internal implementation details
                // (DB errors, pool state, file paths, etc.)
                tracing::error!("internal server error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };

        let body = Json(serde_json::json!({
            "error": message
        }));

        (status, body).into_response()
    }
}

/// Handler for `POST /api/registry/register`.
pub async fn register_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, ApiError> {
    let svc = crate::services::IdentityService::new(state);
    let resp = svc.register_identity(payload).await?;
    Ok(Json(resp))
}

/// Handler for `GET /api/registry/path/:commitmentHex`.
pub async fn get_path_handler(
    Extension(state): Extension<Arc<AppState>>,
    Path(commitment_hex): Path<String>,
) -> Result<Json<GetPathResponse>, ApiError> {
    let svc = crate::services::IdentityService::new(state);
    let resp = svc.get_merkle_path(commitment_hex).await?;
    Ok(Json(resp))
}

/// Handler for `GET /api/registry/current-root`.
pub async fn get_current_root_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<GetRootResponse>, ApiError> {
    let svc = crate::services::IdentityService::new(state);
    let resp = svc.get_current_root().await?;
    Ok(Json(resp))
}

/// Handler for `POST /api/zk/verify-membership`.
pub async fn verify_membership_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<VerifyMembershipRequest>,
) -> Result<Json<VerifyMembershipResponse>, ApiError> {
    let svc = crate::services::IdentityService::new(state);
    let resp = svc.verify_membership(payload).await?;
    Ok(Json(resp))
}

/// Handler for `GET /api/registry/topics`.
pub async fn get_topics_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<VrpTopic>>, ApiError> {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        get_all_topics(&conn).map_err(|e| ApiError::InternalServerError(e.to_string()))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(result))
}

/// Helper to fetch platform identity. Blocking.
///
/// When the fetched identity lacks moderator capabilities, this also runs
/// [`ensure_founder`] to self-heal servers that have no moderator (e.g. due
/// to stale identities preventing the normal founder bootstrap). If a
/// promotion occurs the identity is re-fetched so the caller sees the
/// updated capabilities.
fn fetch_platform_identity(
    state: &AppState,
    pseudonym_id: &str,
) -> Result<PlatformIdentity, ApiError> {
    let conn = state
        .pool
        .get()
        .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

    let identity =
        get_platform_identity(&conn, state.server_id, pseudonym_id).map_err(|e| match e {
            annex_identity::IdentityError::DatabaseError(rusqlite::Error::QueryReturnedNoRows) => {
                ApiError::NotFound(format!("identity not found: {pseudonym_id}"))
            }
            _ => ApiError::InternalServerError(e.to_string()),
        })?;

    // Self-heal: if the identity has no moderator flag, check whether the
    // server has *any* moderator. If not, promote the earliest active identity
    // and re-fetch in case this identity was the one promoted.
    //
    // SECURITY: Only the `ensure_founder` path (which promotes the EARLIEST
    // active identity, not the requester) is used here. The previous
    // stale-moderator auto-promotion was removed because it allowed any
    // identity to escalate to admin by waiting for moderators to go offline.
    if !identity.can_moderate {
        let promoted = ensure_founder(&conn, state.server_id)
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
        if promoted {
            return get_platform_identity(&conn, state.server_id, pseudonym_id)
                .map_err(|e| ApiError::InternalServerError(e.to_string()));
        }
    }

    Ok(identity)
}

/// Handler for `GET /api/identity/:pseudonymId`.
pub async fn get_identity_handler(
    Extension(state): Extension<Arc<AppState>>,
    Path(pseudonym_id): Path<String>,
) -> Result<Json<GetIdentityResponse>, ApiError> {
    let result =
        tokio::task::spawn_blocking(move || fetch_platform_identity(&state, &pseudonym_id))
            .await
            .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(GetIdentityResponse {
        pseudonym_id: result.pseudonym_id,
        participant_type: result.participant_type,
        active: result.active,
        capabilities: Capabilities {
            can_voice: result.can_voice,
            can_moderate: result.can_moderate,
            can_invite: result.can_invite,
            can_federate: result.can_federate,
            can_bridge: result.can_bridge,
        },
    }))
}

/// Handler for `GET /api/identity/:pseudonymId/capabilities`.
pub async fn get_identity_capabilities_handler(
    Extension(state): Extension<Arc<AppState>>,
    Path(pseudonym_id): Path<String>,
) -> Result<Json<GetCapabilitiesResponse>, ApiError> {
    let result =
        tokio::task::spawn_blocking(move || fetch_platform_identity(&state, &pseudonym_id))
            .await
            .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(GetCapabilitiesResponse {
        capabilities: Capabilities {
            can_voice: result.can_voice,
            can_moderate: result.can_moderate,
            can_invite: result.can_invite,
            can_federate: result.can_federate,
            can_bridge: result.can_bridge,
        },
    }))
}

/// Handler for `GET /api/registry/roles`.
pub async fn get_roles_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<VrpRoleEntry>>, ApiError> {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        get_all_roles(&conn).map_err(|e| ApiError::InternalServerError(e.to_string()))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(result))
}
