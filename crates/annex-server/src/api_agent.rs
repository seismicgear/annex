//! Agent API handlers.

use crate::{api::ApiError, middleware::IdentityContext, AppState};
use annex_vrp::{VrpAlignmentStatus, VrpCapabilitySharingContract, VrpTransferScope};
use axum::{
    extract::{Extension, Path},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

type ConfiguredVoiceRow = (
    i64,
    String,
    String,
    String,
    Option<String>,
    f32,
    f32,
    Option<u32>,
);

/// Response body for agent profile inspection.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentProfileResponse {
    pub pseudonym_id: String,
    pub alignment_status: VrpAlignmentStatus,
    pub transfer_scope: VrpTransferScope,
    pub capability_contract: VrpCapabilitySharingContract,
    pub reputation_score: f64,
    /// Current accepted decision to bind into an RTX author signature.
    #[serde(rename = "handshakeRef", default)]
    pub handshake_ref: Option<String>,
    /// Operator-assigned profile, resolved within this server's namespace.
    #[serde(default)]
    pub voice_profile_id: Option<String>,
    /// Actual loaded synthesis backend, including the default when unassigned.
    /// None means no corresponding profile is loaded; it never implies Piper.
    #[serde(default)]
    pub voice_backend: Option<annex_types::voice::VoiceModel>,
}

/// Request body for updating an agent's voice profile.
#[derive(Debug, Deserialize)]
pub struct UpdateAgentVoiceProfileRequest {
    pub voice_profile_id: Option<String>,
}

/// Handler for `GET /api/agents/:pseudonymId`.
pub async fn get_agent_profile_handler(
    Extension(state): Extension<Arc<AppState>>,
    Path(pseudonym_id): Path<String>,
) -> Result<Json<AgentProfileResponse>, ApiError> {
    let pid = pseudonym_id.clone();
    let tts_service = state.tts_service.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        let mut stmt = conn.prepare(
            "SELECT a.alignment_status, a.transfer_scope, a.capability_contract_json, a.reputation_score,
                    CASE WHEN a.active = 1 THEN h.id END, vp.profile_id, a.voice_profile_id
             FROM agent_registrations a
             LEFT JOIN vrp_handshake_log h ON h.id = a.current_handshake_id
                AND h.server_id = a.server_id AND h.peer_pseudonym = a.pseudonym_id
                AND h.peer_type = 'AI_AGENT' AND h.alignment_status IN ('ALIGNED', 'PARTIAL')
             LEFT JOIN voice_profiles vp ON vp.id = a.voice_profile_id AND vp.server_id = a.server_id
             WHERE a.server_id = ?1 AND a.pseudonym_id = ?2"
        ).map_err(|e| ApiError::InternalServerError(format!("prepare failed: {e}")))?;

        let mut rows = stmt
            .query(rusqlite::params![state.server_id, pid])
            .map_err(|e| ApiError::InternalServerError(format!("query failed: {e}")))?;

        if let Some(row) = rows
            .next()
            .map_err(|e| ApiError::InternalServerError(format!("row failed: {e}")))?
        {
            let alignment_str: String = row
                .get(0)
                .map_err(|e| ApiError::InternalServerError(format!("column 0 read error: {e}")))?;
            let scope_str: String = row
                .get(1)
                .map_err(|e| ApiError::InternalServerError(format!("column 1 read error: {e}")))?;
            let contract_json: String = row
                .get(2)
                .map_err(|e| ApiError::InternalServerError(format!("column 2 read error: {e}")))?;
            let score: f64 = row
                .get(3)
                .map_err(|e| ApiError::InternalServerError(format!("column 3 read error: {e}")))?;

            let alignment_status = match alignment_str.as_str() {
                "ALIGNED" => VrpAlignmentStatus::Aligned,
                "PARTIAL" => VrpAlignmentStatus::Partial,
                "CONFLICT" => VrpAlignmentStatus::Conflict,
                _ => {
                    return Err(ApiError::InternalServerError(format!(
                        "unknown alignment status: {alignment_str}"
                    )))
                }
            };

            let transfer_scope = match scope_str.as_str() {
                "NO_TRANSFER" => VrpTransferScope::NoTransfer,
                "REFLECTION_SUMMARIES_ONLY" => VrpTransferScope::ReflectionSummariesOnly,
                "FULL_KNOWLEDGE_BUNDLE" => VrpTransferScope::FullKnowledgeBundle,
                _ => {
                    return Err(ApiError::InternalServerError(format!(
                        "unknown transfer scope: {scope_str}"
                    )))
                }
            };

            let capability_contract: VrpCapabilitySharingContract =
                serde_json::from_str(&contract_json).map_err(|e| {
                    ApiError::InternalServerError(format!("contract json parse error: {e}"))
                })?;

            let has_explicit_assignment = row.get::<_, Option<i64>>(6)
                .map_err(|e| ApiError::InternalServerError(format!("voice assignment read error: {e}")))?
                .is_some();
            Ok(Some((AgentProfileResponse {
                pseudonym_id: pid,
                alignment_status,
                transfer_scope,
                capability_contract,
                reputation_score: score,
                handshake_ref: row.get::<_, Option<i64>>(4)
                    .map_err(|e| ApiError::InternalServerError(format!("handshake reference read error: {e}")))?
                    .map(|id| crate::api_vrp::accepted_handshake_ref(state.server_id, id)),
                voice_profile_id: row.get::<_, Option<String>>(5)
                    .map_err(|e| ApiError::InternalServerError(format!("voice profile read error: {e}")))?,
                voice_backend: None,
            }, has_explicit_assignment)))
        } else {
            Ok(None)
        }
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    match result {
        Some((mut profile, has_explicit_assignment)) => {
            // A broken or foreign explicit assignment is unavailable. Only
            // an actually unassigned agent selects the runtime default.
            if !has_explicit_assignment || profile.voice_profile_id.is_some() {
                profile.voice_backend = tts_service
                    .get_profile(profile.voice_profile_id.as_deref().unwrap_or("default"))
                    .await
                    .map(|loaded| loaded.model);
            }
            Ok(Json(profile))
        }
        None => Err(ApiError::NotFound(format!(
            "agent not found: {pseudonym_id}"
        ))),
    }
}

/// Handler for `PUT /api/agents/:pseudonymId/voice-profile`.
pub async fn update_agent_voice_profile_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(identity): Extension<IdentityContext>,
    Path(pseudonym_id): Path<String>,
    Json(payload): Json<UpdateAgentVoiceProfileRequest>,
) -> Result<Json<Value>, ApiError> {
    // Check permissions
    if !identity.0.can_moderate {
        return Err(ApiError::Forbidden(
            "only operators can assign voice profiles".to_string(),
        ));
    }

    use rusqlite::OptionalExtension;
    let requested_id = payload.voice_profile_id.as_deref().unwrap_or("default");
    let loaded = state
        .tts_service
        .get_profile(requested_id)
        .await
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "voice profile is not loaded in the running speech service: {requested_id}"
            ))
        })?;
    state
        .tts_service
        .validate_profile(&loaded)
        .await
        .map_err(|error| {
            ApiError::BadRequest(format!(
                "voice profile is unavailable or has invalid backend configuration: {error}"
            ))
        })?;
    tokio::task::spawn_blocking(move || {
        let failure=|e:rusqlite::Error|ApiError::InternalServerError(format!("voice assignment database: {e}"));
        let mut conn=state.pool.get().map_err(|e|ApiError::InternalServerError(e.to_string()))?;
        let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(failure)?;
        let current:Option<(bool,i64,bool)>=tx.query_row("SELECT active,token_epoch,can_moderate FROM platform_identities WHERE server_id=?1 AND pseudonym_id=?2",rusqlite::params![state.server_id,identity.0.pseudonym_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(failure)?;
        let Some((true,epoch,moderator))=current else{return Err(ApiError::Unauthorized("moderator session is no longer active".into()));};
        if epoch!=identity.0.token_epoch{return Err(ApiError::Unauthorized("moderator session is no longer active".into()));}
        if !moderator{return Err(ApiError::Forbidden("only current operators can assign voice profiles".into()));}
        let profile_id=if let Some(requested)=payload.voice_profile_id {
            let row:Option<ConfiguredVoiceRow>=tx.query_row("SELECT id,name,model,model_path,config_path,speed,pitch,speaker_id FROM voice_profiles WHERE server_id=?1 AND profile_id=?2",rusqlite::params![state.server_id,requested],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional().map_err(failure)?;
            let Some((id,name,model,model_path,config_path,speed,pitch,speaker_id))=row else{return Err(ApiError::BadRequest("voice profile is not configured for this server".into()));};
            let model=serde_json::from_value(json!(model)).map_err(|_|ApiError::BadRequest("voice profile backend is unsupported".into()))?;
            let configured=annex_types::VoiceProfile{id:requested,name,model,model_path,config_path,speed,pitch,speaker_id};
            if configured!=loaded{return Err(ApiError::BadRequest("voice profile changed or is not loaded with its current configuration; reload the speech service first".into()));}
            Some(id)
        }else{None};
        let changed=tx.execute("UPDATE agent_registrations SET voice_profile_id=?1 WHERE server_id=?2 AND pseudonym_id=?3 AND active=1",rusqlite::params![profile_id,state.server_id,pseudonym_id]).map_err(failure)?;
        if changed==0{return Err(ApiError::NotFound("active agent not found".into()));}
        tx.commit().map_err(failure)?;Ok(())
    }).await.map_err(|e|ApiError::InternalServerError(e.to_string()))??;

    Ok(Json(json!({ "status": "updated" })))
}
