//! VRP Handshake API handlers.

use crate::{api::ApiError, AppState};
use annex_graph::update_node_activity;
use annex_observe::EventPayload;
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
        let mut conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {}", e)))?;

        // 2. Get Server Policy (Read Lock)
        let policy = state.policy.read().map_err(|_| {
            ApiError::InternalServerError("server policy lock poisoned".to_string())
        })?;

        // 3. Construct Local Anchor from Policy
        let local_root = ServerPolicyRoot::from_policy(&policy);
        let local_anchor = local_root.to_anchor_snapshot().map_err(|e| {
            ApiError::InternalServerError(format!("failed to create anchor snapshot: {}", e))
        })?;

        // 4. Construct Local Capability Contract from Policy
        let mut offered_capabilities = Vec::new();
        if policy.voice_enabled {
            offered_capabilities.push("VOICE".to_string());
        }
        if policy.federation_enabled {
            offered_capabilities.push("FEDERATION".to_string());
        }
        offered_capabilities.push("TEXT".to_string());
        offered_capabilities.push("VRP".to_string());

        let local_contract = VrpCapabilitySharingContract {
            required_capabilities: policy.agent_required_capabilities.clone(),
            offered_capabilities,
            redacted_topics: vec![],
        };

        // 5. Construct Alignment Config
        let alignment_config = VrpAlignmentConfig {
            semantic_alignment_required: true,
            min_alignment_score: policy.agent_min_alignment_score,
        };

        // 6. Construct Transfer Acceptance Config
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

        // 8-10. Record outcome, check reputation, and upsert registration atomically.
        let tx = conn.transaction().map_err(|e| {
            ApiError::InternalServerError(format!("failed to begin transaction: {}", e))
        })?;

        // 8. Record Outcome
        record_vrp_outcome(
            &tx,
            state.server_id,
            &payload.pseudonym_id,
            "AI_AGENT",
            &report,
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to log vrp outcome: {}", e)))?;

        // 9. Check Longitudinal Reputation
        let reputation_score =
            check_reputation_score(&tx, state.server_id, &payload.pseudonym_id).map_err(
                |e| ApiError::InternalServerError(format!("failed to check reputation: {}", e)),
            )?;

        // 10. Upsert Agent Registration
        if report.alignment_status == VrpAlignmentStatus::Aligned
            || report.alignment_status == VrpAlignmentStatus::Partial
        {
            // Update graph node activity if it exists
            match update_node_activity(&tx, state.server_id, &payload.pseudonym_id) {
                Ok(true) => {
                    let _ = state.presence_tx.send(PresenceEvent::NodeUpdated {
                        pseudonym_id: payload.pseudonym_id.clone(),
                        active: true,
                    });

                    let observe_payload = EventPayload::NodeReactivated {
                        pseudonym_id: payload.pseudonym_id.clone(),
                    };
                    crate::emit_and_broadcast(
                        &tx,
                        state.server_id,
                        &payload.pseudonym_id,
                        &observe_payload,
                        &state.observe_tx,
                    );
                }
                Ok(false) => {
                    // Node does not exist or was already active; no action needed
                }
                Err(e) => {
                    tracing::warn!(
                        pseudonym_id = %payload.pseudonym_id,
                        "failed to update graph node activity during VRP handshake: {}", e
                    );
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

            tx.execute(
                "INSERT INTO agent_registrations (
                    server_id, pseudonym_id, alignment_status, transfer_scope,
                    capability_contract_json, anchor_snapshot_json, reputation_score, last_handshake_at, active, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, datetime('now'), datetime('now'))
                ON CONFLICT(server_id, pseudonym_id) DO UPDATE SET
                    alignment_status = excluded.alignment_status,
                    transfer_scope = excluded.transfer_scope,
                    capability_contract_json = excluded.capability_contract_json,
                    anchor_snapshot_json = excluded.anchor_snapshot_json,
                    reputation_score = excluded.reputation_score,
                    last_handshake_at = excluded.last_handshake_at,
                    active = 1,
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

            tx.commit().map_err(|e| {
                ApiError::InternalServerError(format!("failed to commit transaction: {}", e))
            })?;

            // Emit AGENT_CONNECTED to persistent log (after commit)
            let observe_payload = EventPayload::AgentConnected {
                pseudonym_id: payload.pseudonym_id.clone(),
                alignment_status: report.alignment_status.to_string(),
            };
            crate::emit_and_broadcast(
                &conn,
                state.server_id,
                &payload.pseudonym_id,
                &observe_payload,
                &state.observe_tx,
            );
        } else if report.alignment_status == VrpAlignmentStatus::Conflict {
            // If an existing agent re-handshakes and gets Conflict, update their
            // status in the DB and deactivate them. New agents with Conflict are
            // simply not inserted (they never had a row).
            let updated = tx
                .execute(
                    "UPDATE agent_registrations
                     SET alignment_status = 'Conflict',
                         transfer_scope = 'NO_TRANSFER',
                         active = 0,
                         updated_at = datetime('now')
                     WHERE server_id = ?1 AND pseudonym_id = ?2",
                    rusqlite::params![state.server_id, payload.pseudonym_id],
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!(
                        "failed to deactivate conflict agent: {}",
                        e
                    ))
                })?;

            tx.commit().map_err(|e| {
                ApiError::InternalServerError(format!("failed to commit transaction: {}", e))
            })?;

            if updated > 0 {
                let observe_payload = EventPayload::AgentDisconnected {
                    pseudonym_id: payload.pseudonym_id.clone(),
                    reason: "VRP handshake resulted in Conflict alignment".to_string(),
                };
                crate::emit_and_broadcast(
                    &conn,
                    state.server_id,
                    &payload.pseudonym_id,
                    &observe_payload,
                    &state.observe_tx,
                );
            }
        } else {
            tx.commit().map_err(|e| {
                ApiError::InternalServerError(format!("failed to commit transaction: {}", e))
            })?;
        }

        Ok(report)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(result))
}
