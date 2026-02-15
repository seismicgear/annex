//! API handlers for the Annex server.

use crate::AppState;
use annex_identity::{register_identity, RoleCode};
use axum::{
    extract::{Extension, Json},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

/// Request body for identity registration.
#[derive(Debug, Deserialize)]
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

/// API error type mapping to HTTP status codes.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("invalid input: {0}")]
    BadRequest(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("internal server error: {0}")]
    InternalServerError(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            ApiError::InternalServerError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
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
    // Validate role code
    let role = RoleCode::from_u8(payload.role_code)
        .ok_or_else(|| ApiError::BadRequest(format!("invalid role code: {}", payload.role_code)))?;

    let result = tokio::task::spawn_blocking(move || {
        // Get DB connection
        let mut conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {}", e)))?;

        // Lock Merkle Tree
        let mut tree = state
            .merkle_tree
            .lock()
            .map_err(|_| ApiError::InternalServerError("merkle tree lock poisoned".to_string()))?;

        // Perform registration
        register_identity(
            &mut tree,
            &mut conn,
            &payload.commitment_hex,
            role,
            payload.node_id,
        )
        .map_err(|e| match e {
            annex_identity::IdentityError::InvalidCommitmentFormat
            | annex_identity::IdentityError::InvalidRoleCode(_)
            | annex_identity::IdentityError::InvalidHex => ApiError::BadRequest(e.to_string()),
            annex_identity::IdentityError::DuplicateNullifier(_) => {
                ApiError::Conflict(e.to_string())
            }
            annex_identity::IdentityError::TreeFull => {
                // Tree full is conceptually a 507 Insufficient Storage, but 500 is fine too
                ApiError::InternalServerError(e.to_string())
            }
            _ => ApiError::InternalServerError(e.to_string()),
        })
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(RegisterResponse {
        identity_id: result.identity_id,
        leaf_index: result.leaf_index,
        root_hex: result.root_hex,
        path_elements: result.path_elements,
        path_indices: result.path_indices,
    }))
}
