//! Agent API handlers.

use crate::{api::ApiError, AppState};
use annex_vrp::{VrpAlignmentStatus, VrpCapabilitySharingContract, VrpTransferScope};
use axum::{
    extract::{Extension, Path},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Response body for agent profile inspection.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentProfileResponse {
    pub pseudonym_id: String,
    pub alignment_status: VrpAlignmentStatus,
    pub transfer_scope: VrpTransferScope,
    pub capability_contract: VrpCapabilitySharingContract,
    pub reputation_score: f32,
}

/// Handler for `GET /api/agents/:pseudonymId`.
pub async fn get_agent_profile_handler(
    Extension(state): Extension<Arc<AppState>>,
    Path(pseudonym_id): Path<String>,
) -> Result<Json<AgentProfileResponse>, ApiError> {
    let pid = pseudonym_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {}", e)))?;

        let mut stmt = conn.prepare(
            "SELECT alignment_status, transfer_scope, capability_contract_json, reputation_score
             FROM agent_registrations
             WHERE server_id = ?1 AND pseudonym_id = ?2"
        ).map_err(|e| ApiError::InternalServerError(format!("prepare failed: {}", e)))?;

        let mut rows = stmt
            .query(rusqlite::params![state.server_id, pid])
            .map_err(|e| ApiError::InternalServerError(format!("query failed: {}", e)))?;

        if let Some(row) = rows
            .next()
            .map_err(|e| ApiError::InternalServerError(format!("row failed: {}", e)))?
        {
            let alignment_str: String = row.get(0).unwrap();
            let scope_str: String = row.get(1).unwrap();
            let contract_json: String = row.get(2).unwrap();
            let score: f64 = row.get(3).unwrap();

            let alignment_status = match alignment_str.as_str() {
                "ALIGNED" => VrpAlignmentStatus::Aligned,
                "PARTIAL" => VrpAlignmentStatus::Partial,
                "CONFLICT" => VrpAlignmentStatus::Conflict,
                _ => {
                    return Err(ApiError::InternalServerError(format!(
                        "unknown alignment status: {}",
                        alignment_str
                    )))
                }
            };

            let transfer_scope = match scope_str.as_str() {
                "NO_TRANSFER" => VrpTransferScope::NoTransfer,
                "REFLECTION_SUMMARIES_ONLY" => VrpTransferScope::ReflectionSummariesOnly,
                "FULL_KNOWLEDGE_BUNDLE" => VrpTransferScope::FullKnowledgeBundle,
                _ => {
                    return Err(ApiError::InternalServerError(format!(
                        "unknown transfer scope: {}",
                        scope_str
                    )))
                }
            };

            let capability_contract: VrpCapabilitySharingContract =
                serde_json::from_str(&contract_json).map_err(|e| {
                    ApiError::InternalServerError(format!("contract json parse error: {}", e))
                })?;

            Ok(Some(AgentProfileResponse {
                pseudonym_id: pid,
                alignment_status,
                transfer_scope,
                capability_contract,
                reputation_score: score as f32,
            }))
        } else {
            Ok(None)
        }
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    match result {
        Some(profile) => Ok(Json(profile)),
        None => Err(ApiError::NotFound(format!(
            "agent not found: {}",
            pseudonym_id
        ))),
    }
}
