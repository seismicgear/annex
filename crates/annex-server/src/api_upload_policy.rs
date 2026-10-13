//! Upload limits visible to channel members without exposing operator policy.
use crate::{
    middleware::IdentityContext,
    services::e2e_service::{require_member, E2eError},
    AppState,
};
use axum::{
    extract::{Extension, Path},
    Json,
};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
pub struct UploadPolicy {
    images_enabled: bool,
    videos_enabled: bool,
    files_enabled: bool,
    max_image_size_mb: u32,
    max_video_size_mb: u32,
    max_file_size_mb: u32,
}

pub async fn get_upload_policy(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel): Path<String>,
) -> Result<Json<UploadPolicy>, E2eError> {
    let policy =
        tokio::task::spawn_blocking(move || {
            let conn = state
                .pool
                .get()
                .map_err(|e| E2eError::Internal(e.to_string()))?;
            let tx = annex_db::read_transaction(&conn)?;
            require_member(&tx, &state, &channel, &identity.pseudonym_id)?;
            let current_epoch: i64 = tx.query_row(
            "SELECT token_epoch FROM platform_identities WHERE server_id=?1 AND pseudonym_id=?2",
            rusqlite::params![state.server_id, identity.pseudonym_id], |row| row.get(0))?;
            if current_epoch != identity.token_epoch {
                return Err(E2eError::Forbidden("session was revoked".into()));
            }
            let json: String = tx.query_row(
                "SELECT policy_json FROM servers WHERE id=?1",
                [state.server_id],
                |row| row.get(0),
            )?;
            let policy: annex_types::ServerPolicy = serde_json::from_str(&json)
                .map_err(|e| E2eError::Internal(format!("invalid stored upload policy: {e}")))?;
            tx.commit()?;
            Ok(UploadPolicy {
                images_enabled: policy.images_enabled,
                videos_enabled: policy.videos_enabled,
                files_enabled: policy.files_enabled,
                max_image_size_mb: policy.max_image_size_mb,
                max_video_size_mb: policy.max_video_size_mb,
                max_file_size_mb: policy.max_file_size_mb,
            })
        })
        .await
        .map_err(|e| E2eError::Internal(e.to_string()))??;
    Ok(Json(policy))
}
