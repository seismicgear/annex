//! Admin API handlers for the Annex server.

use crate::{
    api::ApiError, middleware::IdentityContext, policy::recalculate_all_alignments, AppState,
};
use annex_observe::{emit_event, EventDomain, EventPayload};
use annex_types::ServerPolicy;
use axum::{
    extract::{Extension, Json},
    response::{IntoResponse, Response},
    Json as AxumJson,
};
use std::sync::Arc;
use uuid::Uuid;

/// Handler for `PUT /api/admin/policy`.
///
/// Updates the server's policy, persists it to the database, logs the version,
/// and triggers re-evaluation of all agent and federation alignments.
///
/// Requires `can_moderate` permission.
pub async fn update_policy_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Json(new_policy): Json<ServerPolicy>,
) -> Result<Response, ApiError> {
    // 1. Check Permissions
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to update policy".to_string(),
        ));
    }

    // 2. Persist Policy
    let version_id = Uuid::new_v4().to_string();
    let policy_json = serde_json::to_string(&new_policy)
        .map_err(|e| ApiError::BadRequest(format!("failed to serialize policy: {}", e)))?;

    let state_clone = state.clone();
    let policy_clone = new_policy.clone();
    let version_id_clone = version_id.clone();
    let policy_json_clone = policy_json.clone();
    let moderator_pseudonym = identity.pseudonym_id.clone();

    tokio::task::spawn_blocking(move || {
        let mut conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {}", e))
        })?;

        let tx = conn.transaction().map_err(|e| {
            ApiError::InternalServerError(format!("failed to start transaction: {}", e))
        })?;

        // Update servers table (current policy)
        tx.execute(
            "UPDATE servers SET policy_json = ?1 WHERE id = ?2",
            rusqlite::params![policy_json_clone, state_clone.server_id],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to update servers table: {}", e)))?;

        // Insert into server_policy_versions
        tx.execute(
            "INSERT INTO server_policy_versions (server_id, version_id, policy_json) VALUES (?1, ?2, ?3)",
            rusqlite::params![state_clone.server_id, version_id_clone, policy_json_clone],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to insert policy version: {}", e)))?;

        tx.commit().map_err(|e| {
            ApiError::InternalServerError(format!("failed to commit transaction: {}", e))
        })?;

        // Emit MODERATION_ACTION to persistent event log
        let observe_payload = EventPayload::ModerationAction {
            moderator_pseudonym: moderator_pseudonym.clone(),
            action_type: "policy_update".to_string(),
            target_pseudonym: None,
            description: format!("Server policy updated to version {}", version_id_clone),
        };
        if let Err(e) = emit_event(
            &conn,
            state_clone.server_id,
            EventDomain::Moderation,
            observe_payload.event_type(),
            observe_payload.entity_type(),
            &moderator_pseudonym,
            &observe_payload,
        ) {
            tracing::warn!("failed to emit MODERATION_ACTION event: {}", e);
        }

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    // 3. Update In-Memory State
    {
        let mut policy_lock = state
            .policy
            .write()
            .map_err(|_| ApiError::InternalServerError("policy lock poisoned".to_string()))?;
        *policy_lock = new_policy;
    }

    // 4. Trigger Re-evaluation
    // We do this asynchronously but wait for completion to report errors if any.
    // In a real production system, this might be a fire-and-forget background task if it takes too long,
    // but for now, we want confirmation.
    recalculate_all_alignments(state.clone()).await?;

    tracing::info!(
        version_id = %version_id,
        moderator = %identity.pseudonym_id,
        "server policy updated and alignments recalculated"
    );

    Ok(AxumJson(serde_json::json!({
        "status": "ok",
        "version_id": version_id,
        "policy": policy_clone
    }))
    .into_response())
}
