use crate::AppState;
use annex_federation::{process_incoming_handshake, HandshakeError};
use annex_vrp::{VrpFederationHandshake, VrpValidationReport};
use axum::{extract::Extension, Json};
use rusqlite::params;
use serde::Deserialize;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FederationError {
    #[error("Handshake failed: {0}")]
    Handshake(#[from] HandshakeError),
    #[error("Database error: {0}")]
    DbError(#[from] rusqlite::Error),
    #[error("Unknown remote instance: {0}")]
    UnknownRemote(String),
    #[error("Server policy lock poisoned")]
    LockPoisoned,
}

impl axum::response::IntoResponse for FederationError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            FederationError::Handshake(HandshakeError::UnknownRemoteInstance) => {
                (axum::http::StatusCode::NOT_FOUND, self.to_string())
            }
            FederationError::UnknownRemote(_) => {
                (axum::http::StatusCode::NOT_FOUND, self.to_string())
            }
            _ => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                self.to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

#[derive(Deserialize)]
pub struct HandshakeRequest {
    /// Base URL of the requesting server (to identify the instance).
    pub base_url: String,
    /// The VRP handshake payload.
    #[serde(flatten)]
    pub handshake: VrpFederationHandshake,
}

pub async fn federation_handshake_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<HandshakeRequest>,
) -> Result<Json<VrpValidationReport>, FederationError> {
    let state_clone = state.clone();

    // Perform database operations in blocking thread
    let result = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?; // Wrap pool error

        // 1. Resolve remote instance ID from base_url
        tracing::debug!("Resolving instance for base_url: {}", payload.base_url);
        let remote_instance_id: i64 = conn
            .query_row(
                "SELECT id FROM instances WHERE base_url = ?1",
                params![payload.base_url],
                |row| row.get(0),
            )
            .map_err(|e| {
                tracing::error!("Instance resolution failed: {:?}", e);
                if e == rusqlite::Error::QueryReturnedNoRows {
                    FederationError::UnknownRemote(payload.base_url.clone())
                } else {
                    FederationError::DbError(e)
                }
            })?;

        // 2. Process handshake
        tracing::debug!(
            "Processing handshake for instance id: {}",
            remote_instance_id
        );
        let policy = state_clone.policy.read().unwrap();

        process_incoming_handshake(
            &conn,
            state_clone.server_id,
            &policy,
            remote_instance_id,
            &payload.handshake,
        )
        .map_err(|e| {
            tracing::error!("Handshake failed: {:?}", e);
            FederationError::Handshake(e)
        })
    })
    .await
    .map_err(|e| {
        FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })??;

    Ok(Json(result))
}
