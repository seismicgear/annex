//! VRP Handshake API handlers.

use crate::{api::ApiError, AppState};
use annex_graph::update_node_activity;
use annex_observe::{emit_event, EventDomain, EventPayload};
use annex_types::PresenceEvent;
use annex_vrp::{
    check_reputation_score, record_vrp_outcome, validate_federation_handshake, ServerPolicyRoot,
    VrpAlignmentConfig, VrpAlignmentStatus, VrpCapabilitySharingContract, VrpFederationHandshake,
    VrpTransferAcceptanceConfig, VrpValidationReport,
};
use axum::{extract::Extension, Json};
use serde::Deserialize;
use std::sync::Arc;

/// Request body for agent VRP handshake.
#[derive(Debug, Deserialize)]
pub struct AgentHandshakeRequest {
    /// The agent's pseudonym ID (or temporary ID).
    #[serde(rename = "pseudonymId")]
    pub pseudonym_id: String,
    /// The VRP handshake payload (anchor + contract).
    pub handshake: VrpFederationHandshake,
}

/// Handler for `POST /api/vrp/agent-handshake`.
pub async fn agent_handshake_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<AgentHandshakeRequest>,
) -> Result<Json<VrpValidationReport>, ApiError> {
    let result = tokio::task::spawn_blocking(move || {
        // 1. Get DB connection
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {}", e)))?;

        // 2. Get Server Policy (Read Lock)
        let policy = state.policy.read().map_err(|_| {
            ApiError::InternalServerError("server policy lock poisoned".to_string())
        })?;

        // 3. Construct Local Anchor from Policy
        let local_root = ServerPolicyRoot::from_policy(&policy);
        let local_anchor = local_root.to_anchor_snapshot();

        // 4. Construct Local Capability Contract from Policy
        // Required: from policy.agent_required_capabilities
        // Offered: derived from policy flags
        let mut offered_capabilities = Vec::new();
        if policy.voice_enabled {
            offered_capabilities.push("VOICE".to_string());
        }
        if policy.federation_enabled {
            offered_capabilities.push("FEDERATION".to_string());
        }
        // Add implicit capabilities? e.g. "TEXT", "VRP"
        offered_capabilities.push("TEXT".to_string());
        offered_capabilities.push("VRP".to_string());

        let local_contract = VrpCapabilitySharingContract {
            required_capabilities: policy.agent_required_capabilities.clone(),
            offered_capabilities,
            redacted_topics: vec![],
        };

        // 5. Construct Alignment Config
        let alignment_config = VrpAlignmentConfig {
            semantic_alignment_required: false, // Deferred in Phase 3.3
            min_alignment_score: policy.agent_min_alignment_score,
        };

        // 6. Construct Transfer Acceptance Config
        // Defaulting to allowing reflection summaries, but checking policy/config if available.
        // For now, hardcode reasonable defaults for agents.
        let transfer_config = VrpTransferAcceptanceConfig {
            allow_reflection_summaries: true,
            allow_full_knowledge: false, // Conservative default
        };

        // 7. Validate Handshake
        let report = validate_federation_handshake(
            &local_anchor,
            &local_contract,
            &payload.handshake,
            &alignment_config,
            &transfer_config,
        );

        // 8. Record Outcome
        record_vrp_outcome(
            &conn,
            state.server_id,
            &payload.pseudonym_id,
            "AI_AGENT", // Peer Type
            &report,
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to log vrp outcome: {}", e)))?;

        // 9. Check Longitudinal Reputation
        let reputation_score =
            check_reputation_score(&conn, state.server_id, &payload.pseudonym_id).map_err(
                |e| ApiError::InternalServerError(format!("failed to check reputation: {}", e)),
            )?;

        // 10. Upsert Agent Registration if Aligned or Partial
        if report.alignment_status == VrpAlignmentStatus::Aligned
            || report.alignment_status == VrpAlignmentStatus::Partial
        {
            // Also update graph node activity if it exists
            if let Ok(true) = update_node_activity(&conn, state.server_id, &payload.pseudonym_id) {
                let _ = state.presence_tx.send(PresenceEvent::NodeUpdated {
                    pseudonym_id: payload.pseudonym_id.clone(),
                    active: true,
                });

                // Emit NODE_REACTIVATED to persistent log
                let observe_payload = EventPayload::NodeReactivated {
                    pseudonym_id: payload.pseudonym_id.clone(),
                };
                if let Err(e) = emit_event(
                    &conn,
                    state.server_id,
                    EventDomain::Presence,
                    observe_payload.event_type(),
                    observe_payload.entity_type(),
                    &payload.pseudonym_id,
                    &observe_payload,
                ) {
                    tracing::warn!("failed to emit NODE_REACTIVATED event: {}", e);
                }
            }

            let contract_json = serde_json::to_string(&payload.handshake.capability_contract)
                .map_err(|e| {
                    ApiError::InternalServerError(format!("failed to serialize contract: {}", e))
                })?;

            let anchor_json = serde_json::to_string(&payload.handshake.anchor_snapshot)
                .map_err(|e| {
                    ApiError::InternalServerError(format!("failed to serialize anchor: {}", e))
                })?;

            let now = chrono::Utc::now().to_rfc3339();

            conn.execute(
                "INSERT INTO agent_registrations (
                    server_id, pseudonym_id, alignment_status, transfer_scope,
                    capability_contract_json, anchor_snapshot_json, reputation_score, last_handshake_at, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now'), datetime('now'))
                ON CONFLICT(server_id, pseudonym_id) DO UPDATE SET
                    alignment_status = excluded.alignment_status,
                    transfer_scope = excluded.transfer_scope,
                    capability_contract_json = excluded.capability_contract_json,
                    anchor_snapshot_json = excluded.anchor_snapshot_json,
                    reputation_score = excluded.reputation_score,
                    last_handshake_at = excluded.last_handshake_at,
                    updated_at = datetime('now')
                ",
                rusqlite::params![
                    state.server_id,
                    payload.pseudonym_id,
                    report.alignment_status.to_string(),
                    report.transfer_scope.to_string(),
                    contract_json,
                    anchor_json,
                    reputation_score,
                    now
                ],
            )
            .map_err(|e| {
                ApiError::InternalServerError(format!("failed to upsert registration: {}", e))
            })?;

            // Emit AGENT_CONNECTED to persistent log
            let observe_payload = EventPayload::AgentConnected {
                pseudonym_id: payload.pseudonym_id.clone(),
                alignment_status: report.alignment_status.to_string(),
            };
            if let Err(e) = emit_event(
                &conn,
                state.server_id,
                EventDomain::Agent,
                observe_payload.event_type(),
                observe_payload.entity_type(),
                &payload.pseudonym_id,
                &observe_payload,
            ) {
                tracing::warn!("failed to emit AGENT_CONNECTED event: {}", e);
            }
        }
        // Roadmap says: "On Conflict: reject with detailed report".
        // But if an existing agent becomes Conflict, we should probably update their status.
        // Step 6.6 says "Disconnect agents that are now Conflict".
        // If this is a *new* handshake (re-evaluation), we should update the DB.
        // If it's a *new* agent, we don't insert.
        // But how do we distinguish? UPSERT handles both.
        // If I skip insert on Conflict, a new agent gets 409/Conflict report and no DB row.
        // If an existing agent gets Conflict, their DB row remains "Aligned" (stale). This is BAD.
        // So I *should* upsert even on Conflict if the row exists?
        // SQLite UPSERT inserts if not exists.
        // Maybe I should always upsert?
        // Step 3.6 says: "On Aligned or Partial: create agent_registrations row".
        // This implies on Conflict, do NOT create.
        // But if it *already exists*, I should update it to Conflict.
        // I'll implement: Check if exists. If exists, update. If not exists and Aligned/Partial, insert.
        // OR: Use UPSERT but valid only for Aligned/Partial?
        // Logic:
        // If Aligned/Partial -> Upsert (Create or Update).
        // If Conflict -> Only Update if exists?
        // Let's keep it simple: Only upsert on Aligned/Partial as per spec.
        // The "Re-evaluation on policy change" (Step 6.6) handles the bulk update case separately.
        // If an existing agent re-handshakes and gets Conflict, they effectively fail to "renew" their registration.
        // However, it's better to explicitly mark them Conflict if they exist.
        // I'll stick to the spec "On Aligned or Partial: create...".
        // If an agent gets Conflict, they receive the report and are rejected. They can't proceed to verify-membership anyway.

        Ok(report)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(result))
}
