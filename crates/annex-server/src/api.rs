//! API handlers for the Annex server.

use crate::AppState;
use annex_identity::{get_path_for_commitment, register_identity, RoleCode};
use axum::{
    extract::{Extension, Json, Path},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rusqlite::OptionalExtension;
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

/// API error type mapping to HTTP status codes.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("invalid input: {0}")]
    BadRequest(String),
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
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
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

    let result =
        tokio::task::spawn_blocking(move || {
            // Get DB connection
            let mut conn = state.pool.get().map_err(|e| {
                ApiError::InternalServerError(format!("db connection failed: {}", e))
            })?;

            // Lock Merkle Tree
            let mut tree = state.merkle_tree.lock().map_err(|_| {
                ApiError::InternalServerError("merkle tree lock poisoned".to_string())
            })?;

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

/// Handler for `GET /api/registry/path/:commitmentHex`.
pub async fn get_path_handler(
    Extension(state): Extension<Arc<AppState>>,
    Path(commitment_hex): Path<String>,
) -> Result<Json<GetPathResponse>, ApiError> {
    let result =
        tokio::task::spawn_blocking(move || {
            let conn = state.pool.get().map_err(|e| {
                ApiError::InternalServerError(format!("db connection failed: {}", e))
            })?;

            let tree = state.merkle_tree.lock().map_err(|_| {
                ApiError::InternalServerError("merkle tree lock poisoned".to_string())
            })?;

            get_path_for_commitment(&tree, &conn, &commitment_hex).map_err(|e| match e {
                annex_identity::IdentityError::CommitmentNotFound(_) => {
                    ApiError::NotFound(format!("commitment not found: {}", commitment_hex))
                }
                _ => ApiError::InternalServerError(e.to_string()),
            })
        })
        .await
        .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(GetPathResponse {
        leaf_index: result.0,
        root_hex: result.1,
        path_elements: result.2,
        path_indices: result.3,
    }))
}

/// Handler for `GET /api/registry/current-root`.
pub async fn get_current_root_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<GetRootResponse>, ApiError> {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {}", e)))?;

        let (root_hex, leaf_count) = {
            let tree = state.merkle_tree.lock().map_err(|_| {
                ApiError::InternalServerError("merkle tree lock poisoned".to_string())
            })?;
            (tree.root_hex(), tree.next_index)
        };

        let updated_at: Option<String> = conn
            .query_row(
                "SELECT created_at FROM vrp_roots WHERE root_hex = ?1",
                [&root_hex],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| ApiError::InternalServerError(format!("db query failed: {}", e)))?;

        Ok((root_hex, leaf_count, updated_at))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(GetRootResponse {
        root_hex: result.0,
        leaf_count: result.1,
        updated_at: result.2,
    }))
}
