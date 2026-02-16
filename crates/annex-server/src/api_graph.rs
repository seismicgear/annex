//! Graph API handlers.

use crate::AppState;
use annex_graph::{find_path_bfs, BfsPath};
use axum::{
    extract::{Extension, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Deserialize)]
pub struct GetDegreesParams {
    pub from: String,
    pub to: String,
    #[serde(rename = "maxDepth")]
    pub max_depth: u32,
}

#[derive(Debug, Error)]
pub enum GraphApiError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("internal server error: {0}")]
    InternalServerError(String),
}

impl IntoResponse for GraphApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            GraphApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            GraphApiError::InternalServerError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = Json(serde_json::json!({
            "error": message
        }));

        (status, body).into_response()
    }
}

/// Handler for `GET /api/graph/degrees`.
pub async fn get_degrees_handler(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<GetDegreesParams>,
) -> Result<Json<BfsPath>, GraphApiError> {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state.pool.get().map_err(|e| {
            GraphApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        find_path_bfs(
            &conn,
            state.server_id,
            &params.from,
            &params.to,
            params.max_depth,
        )
        .map_err(|e| GraphApiError::InternalServerError(e.to_string()))
    })
    .await
    .map_err(|e| GraphApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(result))
}
