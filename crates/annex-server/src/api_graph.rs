//! Graph API handlers.

use crate::AppState;
use crate::{api::ApiError, middleware::IdentityContext, services::graph_service};
use annex_graph::{find_path_bfs, BfsPath, GraphProfile};
use axum::{
    extract::{Extension, Path, Query},
    http::StatusCode,
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
            GraphApiError::InternalServerError(msg) => {
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
            GraphApiError::InternalServerError(format!("db connection failed: {e}"))
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
    .map_err(|e| GraphApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(result))
}

/// Handler for `GET /api/graph/profile/{targetPseudonym}`.
///
/// Uses the authenticated identity (from auth middleware) as the viewer,
/// rather than trusting a client-supplied header. This prevents spoofing
/// the viewer pseudonym to access profile data not visible to the caller.
pub async fn get_profile_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(crate::middleware::IdentityContext(identity)): Extension<
        crate::middleware::IdentityContext,
    >,
    Path(target_pseudonym): Path<String>,
) -> Result<Json<GraphProfile>, ApiError> {
    Ok(Json(
        graph_service::profile(state, identity, target_pseudonym).await?,
    ))
}

pub async fn get_metadata_grants_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Json<graph_service::MetadataGrants>, ApiError> {
    Ok(Json(graph_service::grants(state, identity).await?))
}

pub async fn put_metadata_grant_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(grantee): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    graph_service::set_grant(state, identity, grantee, true).await?;
    Ok(Json(serde_json::json!({"granted":true})))
}

pub async fn delete_metadata_grant_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(grantee): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    graph_service::set_grant(state, identity, grantee, false).await?;
    Ok(Json(serde_json::json!({"granted":false})))
}
