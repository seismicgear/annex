//! VRP Handshake API handlers.

use crate::{api::ApiError, api_ws::verify_ws_token_for_auth, AppState};
use annex_graph::update_node_activity;
use annex_observe::EventPayload;
use annex_types::PresenceEvent;
use annex_vrp::{
    apply_reputation_gate, check_reputation_score, record_vrp_outcome,
    validate_federation_handshake, ServerPolicyRoot, VrpAlignmentConfig, VrpAlignmentStatus,
    VrpCapabilitySharingContract, VrpFederationHandshake, VrpTransferAcceptanceConfig,
    VrpValidationReport,
};
use axum::{
    extract::Extension,
    http::{HeaderMap, StatusCode},
    Json,
};
use rusqlite::OptionalExtension;
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

/// Verifies a `Authorization: Bearer <session-token>` header and returns the
/// pseudonym bound by the token. Used to gate re-handshakes against
/// hijacking from unauthenticated callers.
fn pseudonym_from_authorization_header(
    headers: &HeaderMap,
    secret: &[u8; 32],
) -> Result<Option<String>, ApiError> {
    let Some(val) = headers.get("Authorization") else {
        return Ok(None);
    };
    let val_str = val
        .to_str()
        .map_err(|_| ApiError::Forbidden("invalid Authorization header".to_string()))?;
    let Some(token) = val_str.strip_prefix("Bearer ") else {
        return Ok(None);
    };
    match verify_ws_token_for_auth(token, secret) {
        Ok(pseudonym) => Ok(Some(pseudonym)),
        Err(StatusCode::UNAUTHORIZED) => Err(ApiError::Forbidden(
            "agent handshake rejected: invalid or expired session token".to_string(),
        )),
        Err(_) => Err(ApiError::InternalServerError(
            "session token verification failed".to_string(),
        )),
    }
}

/// Handler for `POST /api/vrp/agent-handshake`.
pub async fn agent_handshake_handler(
    Extension(state): Extension<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<AgentHandshakeRequest>,
) -> Result<Json<VrpValidationReport>, ApiError> {
    // Validate Authorization header up-front (before any DB I/O) so a
    // malformed token never produces a partial state change. The token is
    // optional here — pre-registration handshakes do not have one yet —
    // but if present it must be valid.
    let token_pseudonym = pseudonym_from_authorization_header(&headers, &state.ws_token_secret)?;

    let pseudonym_id_for_disconnect = payload.pseudonym_id.clone();
    let state_for_disconnect = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        // 1. Get DB connection
        let mut conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        // 1b. Validate that the pseudonym belongs to an AI agent identity.
        // Without this check, human identities could register as agents and
        // gain agent-specific capabilities (RTX, voice profiles).
        //
        // This endpoint is unauthenticated by design so newly-spun-up agents
        // can establish their first handshake before any identity row
        // exists. But once a platform_identities row DOES exist for the
        // pseudonym, allowing unauthenticated re-handshakes is identity
        // hijacking: any caller who knows a public agent pseudonym (visible
        // in `/api/public/agents`, channel listings, the events stream)
        // could submit a fresh anchor + contract and silently rewrite the
        // agent's `agent_registrations` row, including capability
        // contracts, alignment status, and transfer scope. They could also
        // force the agent into Conflict alignment, which deactivates the
        // row and cuts the agent's WebSocket session.
        //
        // Mitigation: when the pseudonym is already registered as an
        // AI_AGENT, REQUIRE a valid `Authorization: Bearer <token>` whose
        // bound pseudonym matches `payload.pseudonym_id`. Pre-registration
        // (no platform_identities row yet) keeps the existing
        // unauthenticated path.
        //
        // Note on the column type: `platform_identities.participant_type`
        // is TEXT — populated by `create_platform_identity` with the
        // label-form (`"HUMAN"`, `"AI_AGENT"`, …), not the role-code
        // integer. Reading it as `u8` (as the previous version did) would
        // silently turn a successful row read into a rusqlite type-coercion
        // error, masking the lookup as a 500. We compare strings instead.
        {
            let participant_type: Option<String> = conn
                .query_row(
                    "SELECT pi.participant_type FROM platform_identities pi
                     WHERE pi.server_id = ?1 AND pi.pseudonym_id = ?2 AND pi.active = 1",
                    rusqlite::params![state.server_id, &payload.pseudonym_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| ApiError::InternalServerError(format!("db query failed: {e}")))?;

            match participant_type.as_deref() {
                Some(label) if label == annex_types::RoleCode::AiAgent.label() => {
                    // Re-handshake path: require a session token bound to
                    // this exact pseudonym.
                    match token_pseudonym.as_deref() {
                        Some(p) if p == payload.pseudonym_id => { /* OK */ }
                        Some(_) => {
                            return Err(ApiError::Forbidden(
                                "agent handshake rejected: session token does not match pseudonymId"
                                    .to_string(),
                            ));
                        }
                        None => {
                            return Err(ApiError::Forbidden(
                                "agent handshake rejected: registered agent must present a valid \
                                 session token for re-handshake".to_string(),
                            ));
                        }
                    }
                }
                Some(_) => {
                    return Err(ApiError::Forbidden(
                        "agent handshake rejected: identity is not registered as AI_AGENT".to_string(),
                    ));
                }
                None => {
                    // Allow handshake from unregistered pseudonyms (pre-registration agents)
                    // but log a warning for monitoring
                    tracing::debug!(
                        pseudonym_id = %payload.pseudonym_id,
                        "agent handshake from unregistered pseudonym (pre-registration)"
                    );
                }
            }
        }

        // 2. Get Server Policy (Read Lock)
        let policy = state.policy.read().map_err(|_| {
            ApiError::InternalServerError("server policy lock poisoned".to_string())
        })?;

        // 3. Construct Local Anchor from Policy
        let local_root = ServerPolicyRoot::from_policy(&policy);
        let local_anchor = local_root.to_anchor_snapshot().map_err(|e| {
            ApiError::InternalServerError(format!("failed to create anchor snapshot: {e}"))
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
            ApiError::InternalServerError(format!("failed to begin transaction: {e}"))
        })?;

        // 9. Check longitudinal reputation FROM PRIOR HISTORY — before the
        // current outcome is recorded — so a sustained history of
        // Partial/Conflict outcomes can gate this handshake's verdict.
        let reputation_score =
            check_reputation_score(&tx, state.server_id, &payload.pseudonym_id).map_err(
                |e| ApiError::InternalServerError(format!("failed to check reputation: {e}")),
            )?;

        // Gate the freshly-computed alignment by reputation: a poor track
        // record downgrades the verdict one step (Aligned->Partial->Conflict).
        // This is what makes reputation actually affect the outcome.
        let report = apply_reputation_gate(report, reputation_score, &transfer_config);

        // 8. Record the FINAL (gated) outcome to the handshake log.
        record_vrp_outcome(
            &tx,
            state.server_id,
            &payload.pseudonym_id,
            "AI_AGENT",
            &report,
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to log vrp outcome: {e}")))?;

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
                        &state.signing_key,
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
                    ApiError::InternalServerError(format!("failed to serialize contract: {e}"))
                })?;

            let anchor_json = serde_json::to_string(&payload.handshake.anchor_snapshot)
                .map_err(|e| {
                    ApiError::InternalServerError(format!("failed to serialize anchor: {e}"))
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
                ApiError::InternalServerError(format!("failed to upsert registration: {e}"))
            })?;

            tx.commit().map_err(|e| {
                ApiError::InternalServerError(format!("failed to commit transaction: {e}"))
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
                &state.signing_key,
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
                        "failed to deactivate conflict agent: {e}"
                    ))
                })?;

            tx.commit().map_err(|e| {
                ApiError::InternalServerError(format!("failed to commit transaction: {e}"))
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
                    &state.signing_key,
                );
            }
        } else {
            tx.commit().map_err(|e| {
                ApiError::InternalServerError(format!("failed to commit transaction: {e}"))
            })?;
        }

        Ok(report)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    // If the agent was deactivated due to Conflict alignment, disconnect their
    // WebSocket session so they cannot continue sending/receiving messages.
    if result.alignment_status == VrpAlignmentStatus::Conflict {
        state_for_disconnect
            .connection_manager
            .disconnect_user(&pseudonym_id_for_disconnect)
            .await;
    }

    Ok(Json(result))
}
