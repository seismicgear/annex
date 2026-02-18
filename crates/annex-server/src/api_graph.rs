//! Graph API handlers.

use crate::AppState;
use annex_graph::{find_path_bfs, get_visible_profile, BfsPath, GraphError, GraphProfile};
use axum::{
    extract::{Extension, Path, Query},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;
use thiserror::Error;

/// Maximum allowed value for `max_depth` in BFS queries. Prevents
/// combinatorial explosion and long-running queries.
const MAX_BFS_DEPTH: u32 = 10;

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
    #[error("not found: {0}")]
    NotFound(String),
    #[error("internal server error: {0}")]
    InternalServerError(String),
}

impl IntoResponse for GraphApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            GraphApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            GraphApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
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
    if params.max_depth > MAX_BFS_DEPTH {
        return Err(GraphApiError::BadRequest(format!(
            "max_depth must be <= {MAX_BFS_DEPTH}, got {}",
            params.max_depth
        )));
    }

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

/// Handler for `GET /api/graph/profile/{targetPseudonym}`.
pub async fn get_profile_handler(
    Extension(state): Extension<Arc<AppState>>,
    Path(target_pseudonym): Path<String>,
    headers: HeaderMap,
) -> Result<Json<GraphProfile>, GraphApiError> {
    // In Phase 5, we expect X-Annex-Viewer header to determine visibility.
    // Future phases may integrate this with standard auth middleware.
    let viewer_pseudonym = if let Some(val) = headers.get("X-Annex-Viewer") {
        val.to_str()
            .map_err(|_| GraphApiError::BadRequest("Invalid X-Annex-Viewer header".into()))?
            .to_string()
    } else {
        return Err(GraphApiError::BadRequest(
            "Missing X-Annex-Viewer header".into(),
        ));
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = state.pool.get().map_err(|e| {
            GraphApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        get_visible_profile(&conn, state.server_id, &viewer_pseudonym, &target_pseudonym).map_err(
            |e| match e {
                GraphError::NodeNotFound(_) => GraphApiError::NotFound(e.to_string()),
                _ => GraphApiError::InternalServerError(e.to_string()),
            },
        )
    })
    .await
    .map_err(|e| GraphApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(result))
}
