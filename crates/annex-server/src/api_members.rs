//! Minimal authenticated member directory for self-service visibility grants.
use crate::{api::ApiError, middleware::IdentityContext, AppState};
use axum::{extract::Extension, Json};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
pub struct DirectoryMember {
    pseudonym_id: String,
    participant_type: String,
}

#[derive(Serialize)]
pub struct MemberDirectory {
    members: Vec<DirectoryMember>,
}

/// Active server members may discover grant recipients. Administrative
/// capabilities, identity commitments, timestamps, and usernames stay private.
pub async fn list_members_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(_identity)): Extension<IdentityContext>,
) -> Result<Json<MemberDirectory>, ApiError> {
    let members = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT pseudonym_id, participant_type FROM platform_identities
             WHERE server_id = ?1 AND active = 1 ORDER BY pseudonym_id",
            )
            .map_err(|e| ApiError::InternalServerError(format!("query failed: {e}")))?;
        let rows = stmt
            .query_map([state.server_id], |row| {
                Ok(DirectoryMember {
                    pseudonym_id: row.get(0)?,
                    participant_type: row.get(1)?,
                })
            })
            .map_err(|e| ApiError::InternalServerError(format!("query failed: {e}")))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| ApiError::InternalServerError(format!("row error: {e}")))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;
    Ok(Json(MemberDirectory { members }))
}
