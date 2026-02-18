//! Policy management and re-evaluation logic.

use crate::{api::ApiError, AppState};
use annex_observe::EventPayload;
use annex_types::PresenceEvent;
use annex_vrp::{
    validate_federation_handshake, ServerPolicyRoot, VrpAlignmentConfig, VrpAlignmentStatus,
    VrpAnchorSnapshot, VrpCapabilitySharingContract, VrpFederationHandshake,
    VrpTransferAcceptanceConfig,
};
use std::sync::Arc;

/// Recalculates alignment for all active agents based on the current server policy.
///
/// This should be called whenever the server policy is updated.
pub async fn recalculate_agent_alignments(state: Arc<AppState>) -> Result<(), ApiError> {
    // 1. Get Server Policy (Read Lock)
    let policy = state
        .policy
        .read()
        .map_err(|_| ApiError::InternalServerError("policy lock poisoned".to_string()))?
        .clone();

    // 2. Prepare Validation Context
    let local_root = ServerPolicyRoot::from_policy(&policy);
    let local_anchor = local_root.to_anchor_snapshot();

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

    let alignment_config = VrpAlignmentConfig {
        semantic_alignment_required: false, // Deferred in Phase 3.3
        min_alignment_score: policy.agent_min_alignment_score,
    };

    let transfer_config = VrpTransferAcceptanceConfig {
        allow_reflection_summaries: true,
        allow_full_knowledge: false,
    };

    let state_clone = state.clone();

    // 3. Process Agents in Background
    let agents_to_disconnect = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {}", e)))?;

        let (agents_to_update, agents_to_disconnect) = {
            let mut stmt = conn
                .prepare(
                    "SELECT pseudonym_id, alignment_status, transfer_scope, capability_contract_json, anchor_snapshot_json
                     FROM agent_registrations
                     WHERE active = 1 AND server_id = ?1"
                )
                .map_err(|e| ApiError::InternalServerError(format!("prepare failed: {}", e)))?;

            let agent_iter = stmt
                .query_map([state_clone.server_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })
                .map_err(|e| ApiError::InternalServerError(format!("query failed: {}", e)))?;

            let mut updates = Vec::new();
            let mut disconnects = Vec::new();

            for agent in agent_iter {
                let (pseudonym, old_alignment_str, old_scope_str, contract_json, anchor_json) =
                    agent.map_err(|e| ApiError::InternalServerError(format!("row error: {}", e)))?;

                let anchor_json = match anchor_json {
                    Some(json) => json,
                    None => {
                        tracing::warn!("Agent {} has no anchor snapshot, skipping re-evaluation", pseudonym);
                        continue;
                    }
                };

                let anchor: VrpAnchorSnapshot = serde_json::from_str(&anchor_json).map_err(|_| {
                    ApiError::InternalServerError("failed to parse anchor".to_string())
                })?;

                let contract: VrpCapabilitySharingContract = serde_json::from_str(&contract_json)
                    .map_err(|_| {
                        ApiError::InternalServerError("failed to parse contract".to_string())
                    })?;

                let handshake = VrpFederationHandshake {
                    anchor_snapshot: anchor,
                    capability_contract: contract,
                };

                let report = validate_federation_handshake(
                    &local_anchor,
                    &local_contract,
                    &handshake,
                    &alignment_config,
                    &transfer_config,
                );

                let new_alignment_str = report.alignment_status.to_string();
                let new_scope_str = report.transfer_scope.to_string();

                if new_alignment_str != old_alignment_str || new_scope_str != old_scope_str {
                    if report.alignment_status == VrpAlignmentStatus::Conflict {
                        disconnects.push(pseudonym.clone());
                        updates.push((pseudonym, report, false)); // active = false
                    } else {
                        updates.push((pseudonym, report, true)); // active = true
                    }
                }
            }
            (updates, disconnects)
        };

        // Apply updates
        for (pseudonym, report, active) in agents_to_update {
             let active_int = if active { 1 } else { 0 };
             conn.execute(
                "UPDATE agent_registrations SET
                    alignment_status = ?1,
                    transfer_scope = ?2,
                    active = ?3,
                    updated_at = datetime('now')
                 WHERE server_id = ?4 AND pseudonym_id = ?5",
                rusqlite::params![
                    report.alignment_status.to_string(),
                    report.transfer_scope.to_string(),
                    active_int,
                    state_clone.server_id,
                    pseudonym
                ],
            ).map_err(|e| ApiError::InternalServerError(format!("update failed: {}", e)))?;

            // Emit presence event (SSE)
             let _ = state_clone.presence_tx.send(PresenceEvent::NodeUpdated {
                pseudonym_id: pseudonym.clone(),
                active,
            });

            // Emit to persistent event log
            if active {
                let observe_payload = EventPayload::AgentRealigned {
                    pseudonym_id: pseudonym.clone(),
                    alignment_status: report.alignment_status.to_string(),
                    previous_status: "changed".to_string(),
                };
                crate::emit_and_broadcast(
                    &conn,
                    state_clone.server_id,
                    &pseudonym,
                    &observe_payload,
                    &state_clone.observe_tx,
                );
            } else {
                let observe_payload = EventPayload::AgentDisconnected {
                    pseudonym_id: pseudonym.clone(),
                    reason: "policy_conflict".to_string(),
                };
                crate::emit_and_broadcast(
                    &conn,
                    state_clone.server_id,
                    &pseudonym,
                    &observe_payload,
                    &state_clone.observe_tx,
                );
            }
        }

        Ok(agents_to_disconnect)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    // Disconnect users (must be done in async context)
    for pseudonym in agents_to_disconnect {
        state.connection_manager.disconnect_user(&pseudonym).await;
    }

    Ok(())
}

/// Recalculates alignment for all active federation agreements based on the current server policy.
pub async fn recalculate_federation_agreements(state: Arc<AppState>) -> Result<(), ApiError> {
    // 1. Get Server Policy (Read Lock)
    let policy = state
        .policy
        .read()
        .map_err(|_| ApiError::InternalServerError("policy lock poisoned".to_string()))?
        .clone();

    // 2. Prepare Validation Context
    let local_root = ServerPolicyRoot::from_policy(&policy);
    let local_anchor = local_root.to_anchor_snapshot();

    let mut offered_capabilities = Vec::new();
    if policy.voice_enabled {
        offered_capabilities.push("voice".to_string());
    }
    if policy.federation_enabled {
        offered_capabilities.push("federation".to_string());
    }

    let local_contract = VrpCapabilitySharingContract {
        required_capabilities: policy.agent_required_capabilities.clone(),
        offered_capabilities,
        redacted_topics: vec![],
    };

    let alignment_config = VrpAlignmentConfig {
        semantic_alignment_required: false,
        min_alignment_score: policy.agent_min_alignment_score,
    };

    let transfer_config = VrpTransferAcceptanceConfig {
        allow_reflection_summaries: policy.federation_enabled,
        allow_full_knowledge: false,
    };

    let state_clone = state.clone();

    // 3. Process in Background
    tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {}", e)))?;

        let updates = {
            let mut stmt = conn
                .prepare(
                    "SELECT fa.id, i.base_url, fa.alignment_status, fa.transfer_scope, fa.remote_handshake_json
                     FROM federation_agreements fa
                     JOIN instances i ON fa.remote_instance_id = i.id
                     WHERE fa.active = 1",
                )
                .map_err(|e| ApiError::InternalServerError(format!("prepare failed: {}", e)))?;

            let iter = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })
                .map_err(|e| ApiError::InternalServerError(format!("query failed: {}", e)))?;

            let mut updates = Vec::new();

            for row in iter {
                let (id, base_url, old_alignment_str, old_scope_str, handshake_json) =
                    row.map_err(|e| ApiError::InternalServerError(format!("row error: {}", e)))?;

                let handshake_json = match handshake_json {
                    Some(json) => json,
                    None => {
                        tracing::warn!(
                            "Federation agreement {} ({}) has no handshake data, skipping re-evaluation",
                            id,
                            base_url
                        );
                        continue;
                    }
                };

                let handshake: VrpFederationHandshake = serde_json::from_str(&handshake_json)
                    .map_err(|_| {
                        ApiError::InternalServerError("failed to parse handshake".to_string())
                    })?;

                let report = validate_federation_handshake(
                    &local_anchor,
                    &local_contract,
                    &handshake,
                    &alignment_config,
                    &transfer_config,
                );

                let new_alignment_str = report.alignment_status.to_string();
                let new_scope_str = report.transfer_scope.to_string();

                if new_alignment_str != old_alignment_str || new_scope_str != old_scope_str {
                    updates.push((id, base_url, report));
                }
            }
            updates
        };

        // Apply updates
        for (id, base_url, report) in updates {
            let active_int = if report.alignment_status == VrpAlignmentStatus::Conflict {
                0
            } else {
                1
            };

            // Serialize updated report
            let report_json = serde_json::to_string(&report).map_err(|e| {
                ApiError::InternalServerError(format!("failed to serialize report: {}", e))
            })?;

            conn.execute(
                "UPDATE federation_agreements SET
                    alignment_status = ?1,
                    transfer_scope = ?2,
                    agreement_json = ?3,
                    active = ?4,
                    updated_at = datetime('now')
                 WHERE id = ?5",
                rusqlite::params![
                    report.alignment_status.to_string(),
                    report.transfer_scope.to_string(),
                    report_json,
                    active_int,
                    id
                ],
            )
            .map_err(|e| ApiError::InternalServerError(format!("update failed: {}", e)))?;

            // Emit Event (SSE broadcast + persistent log)
            if report.alignment_status == VrpAlignmentStatus::Conflict {
                tracing::info!(
                    "Federation severed with {} due to policy conflict",
                    base_url
                );
                let _ = state_clone.presence_tx.send(PresenceEvent::FederationSevered {
                    remote_base_url: base_url.clone(),
                });

                let observe_payload = EventPayload::FederationSevered {
                    remote_url: base_url.clone(),
                    reason: "policy_conflict".to_string(),
                };
                crate::emit_and_broadcast(
                    &conn,
                    state_clone.server_id,
                    &base_url,
                    &observe_payload,
                    &state_clone.observe_tx,
                );
            } else {
                tracing::info!(
                    "Federation realigned with {}: {}",
                    base_url,
                    report.alignment_status
                );
                // Map VrpAlignmentStatus to annex_types::AlignmentStatus
                let status = match report.alignment_status {
                    VrpAlignmentStatus::Aligned => annex_types::AlignmentStatus::Aligned,
                    VrpAlignmentStatus::Partial => annex_types::AlignmentStatus::Partial,
                    VrpAlignmentStatus::Conflict => annex_types::AlignmentStatus::Conflict,
                };

                let _ = state_clone
                    .presence_tx
                    .send(PresenceEvent::FederationRealigned {
                        remote_base_url: base_url.clone(),
                        alignment_status: status,
                    });

                let observe_payload = EventPayload::FederationRealigned {
                    remote_url: base_url.clone(),
                    alignment_status: report.alignment_status.to_string(),
                    previous_status: "changed".to_string(),
                };
                crate::emit_and_broadcast(
                    &conn,
                    state_clone.server_id,
                    &base_url,
                    &observe_payload,
                    &state_clone.observe_tx,
                );
            }
        }

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(())
}

/// Recalculates alignments for both agents and federation agreements.
pub async fn recalculate_all_alignments(state: Arc<AppState>) -> Result<(), ApiError> {
    recalculate_agent_alignments(state.clone()).await?;
    recalculate_federation_agreements(state).await?;
    Ok(())
}
