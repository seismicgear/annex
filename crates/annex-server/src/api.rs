//! API handlers for the Annex server.

use crate::AppState;
use annex_graph::{ensure_graph_node, role_code_to_node_type};
use annex_identity::{
    create_platform_identity, derive_nullifier_hex, derive_pseudonym_id, ensure_founder,
    get_all_roles, get_all_topics, get_path_for_commitment, get_platform_identity,
    insert_nullifier, register_identity,
    zk::{parse_fr_from_hex, parse_proof, parse_public_signals, verify_proof},
    Capabilities, PlatformIdentity, RoleCode, VrpRoleEntry, VrpTopic,
};
use annex_observe::EventPayload;
use annex_types::PresenceEvent;
use axum::{
    extract::{Extension, Json, Path},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

/// Request body for identity registration.
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    /// The identity commitment (64-char hex string).
    #[serde(rename = "commitmentHex")]
    pub commitment_hex: String,
    /// The role code of the participant (1..=5).
    #[serde(rename = "roleCode")]
    pub role_code: u8,
    /// The node ID used in the commitment derivation.
    #[serde(rename = "nodeId")]
    pub node_id: i64,
    /// Optional invite code for invite-only servers.
    #[serde(default, rename = "inviteCode")]
    pub invite_code: Option<String>,
    /// Optional server password for password-protected servers.
    #[serde(default, rename = "serverPassword")]
    pub server_password: Option<String>,
}

/// Response body for successful registration.
#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    /// The assigned database ID for the identity.
    #[serde(rename = "identityId")]
    pub identity_id: i64,
    /// The assigned Merkle tree leaf index.
    #[serde(rename = "leafIndex")]
    pub leaf_index: usize,
    /// The new Merkle root (hex string).
    #[serde(rename = "rootHex")]
    pub root_hex: String,
    /// The Merkle path elements (hex strings) for proof generation.
    #[serde(rename = "pathElements")]
    pub path_elements: Vec<String>,
    /// The Merkle path indices (0 or 1).
    #[serde(rename = "pathIndexBits")]
    pub path_indices: Vec<u8>,
}

/// Response body for Merkle path retrieval.
#[derive(Debug, Serialize, Deserialize)]
pub struct GetPathResponse {
    /// The Merkle tree leaf index.
    #[serde(rename = "leafIndex")]
    pub leaf_index: usize,
    /// The current Merkle root (hex string).
    #[serde(rename = "rootHex")]
    pub root_hex: String,
    /// The Merkle path elements (hex strings).
    #[serde(rename = "pathElements")]
    pub path_elements: Vec<String>,
    /// The Merkle path indices (0 or 1).
    #[serde(rename = "pathIndexBits")]
    pub path_indices: Vec<u8>,
}

/// Response body for current root retrieval.
#[derive(Debug, Serialize, Deserialize)]
pub struct GetRootResponse {
    /// The current Merkle root (hex string).
    #[serde(rename = "rootHex")]
    pub root_hex: String,
    /// The number of leaves currently in the tree.
    #[serde(rename = "leafCount")]
    pub leaf_count: usize,
    /// Timestamp when this root was created (if persisted).
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
}

/// Request body for ZK membership verification.
///
/// Note on privacy: This endpoint requires the public identity commitment to be
/// submitted alongside the proof. This allows the server to verify that the
/// proof corresponds to the claimed identity (via public signals) and to derive
/// the deterministic pseudonym. While the proof demonstrates membership in the
/// Merkle tree without revealing the private key or Merkle path to *observers*
/// of the proof alone, the server here acts as the verifier and issuer of the
/// topic-scoped pseudonym, and thus learns the mapping between commitment and
/// pseudonym for this interaction. This is consistent with the Phase 1 identity model.
#[derive(Debug, Deserialize)]
pub struct VerifyMembershipRequest {
    /// The Merkle root against which the proof was generated.
    pub root: String,
    /// The identity commitment.
    pub commitment: String,
    /// The topic for which the pseudonym is being derived.
    pub topic: String,
    /// The Groth16 proof (JSON object).
    pub proof: serde_json::Value,
    /// The public signals (array of strings).
    #[serde(rename = "publicSignals")]
    pub public_signals: Vec<String>,
}

/// Response body for successful membership verification.
#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyMembershipResponse {
    /// Whether verification succeeded.
    pub ok: bool,
    /// The derived pseudonym ID.
    #[serde(rename = "pseudonymId")]
    pub pseudonym_id: String,
    /// HMAC-signed session token for authenticated API calls.
    /// Clients must send this as `Authorization: Bearer <token>`.
    #[serde(rename = "sessionToken")]
    pub session_token: String,
}

/// Response body for identity query.
#[derive(Debug, Serialize, Deserialize)]
pub struct GetIdentityResponse {
    /// The pseudonym ID.
    #[serde(rename = "pseudonymId")]
    pub pseudonym_id: String,
    /// The participant type (role).
    #[serde(rename = "participantType")]
    pub participant_type: RoleCode,
    /// Whether the identity is active.
    pub active: bool,
    /// Capability flags.
    pub capabilities: Capabilities,
}

/// Response body for identity capabilities query.
#[derive(Debug, Serialize, Deserialize)]
pub struct GetCapabilitiesResponse {
    /// Capability flags.
    pub capabilities: Capabilities,
}

/// API error type mapping to HTTP status codes.
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("invalid input: {0}")]
    BadRequest(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("internal server error: {0}")]
    InternalServerError(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            ApiError::InternalServerError(msg) => {
                // Log the real error server-side but return a generic message
                // to the client to prevent leaking internal implementation details
                // (DB errors, pool state, file paths, etc.)
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

/// Handler for `POST /api/registry/register`.
pub async fn register_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, ApiError> {
    // Validate role code
    let role = RoleCode::from_u8(payload.role_code)
        .ok_or_else(|| ApiError::BadRequest(format!("invalid role code: {}", payload.role_code)))?;

    // Enforce access_mode policy
    let access_mode = {
        let policy = state
            .policy
            .read()
            .map_err(|_| ApiError::InternalServerError("policy lock poisoned".to_string()))?;
        policy.access_mode.clone()
    };

    // Pre-validate invite code format before the blocking task.
    // The actual atomically-safe validation + use_count increment happens
    // inside the same transaction as registration (below).
    let invite_code_for_registration = if access_mode == "invite_only" {
        let invite_code = payload.invite_code.as_deref().unwrap_or("").trim();
        if invite_code.is_empty() {
            return Err(ApiError::Forbidden(
                "This server requires an invite code to register.".to_string(),
            ));
        }
        Some(invite_code.to_string())
    } else {
        None
    };

    // Enforce password access mode
    if access_mode == "password" {
        let expected_password = {
            let policy = state
                .policy
                .read()
                .map_err(|_| ApiError::InternalServerError("policy lock poisoned".to_string()))?;
            policy.access_password.clone()
        };
        let provided = payload.server_password.as_deref().unwrap_or("").trim();
        if provided.is_empty() || provided != expected_password {
            return Err(ApiError::Forbidden(
                "This server requires a password to register.".to_string(),
            ));
        }
    }

    let result =
        tokio::task::spawn_blocking(move || {
            // Get DB connection
            let mut conn = state.pool.get().map_err(|e| {
                ApiError::InternalServerError(format!("db connection failed: {e}"))
            })?;

            // Enforce max_members policy
            {
                let max_members = state
                    .policy
                    .read()
                    .map_err(|_| {
                        ApiError::InternalServerError("policy lock poisoned".to_string())
                    })?
                    .max_members;
                let current_count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM platform_identities WHERE server_id = ?1 AND active = 1",
                        rusqlite::params![state.server_id],
                        |row| row.get(0),
                    )
                    .map_err(|e| {
                        ApiError::InternalServerError(format!("member count query failed: {e}"))
                    })?;
                if current_count >= max_members as i64 {
                    return Err(ApiError::Forbidden(
                        "This server has reached its maximum member limit.".to_string(),
                    ));
                }
            }

            // Validate invite code (check expiry, max_uses) before
            // registration, but defer use_count increment until AFTER
            // registration succeeds. This prevents wasting invite uses
            // when registration fails (duplicate commitment, tree full).
            if let Some(ref code) = invite_code_for_registration {
                let row: Result<(Option<i64>, i64, Option<String>), _> = conn.query_row(
                    "SELECT max_uses, use_count, expires_at FROM invite_codes WHERE server_id = ?1 AND code = ?2",
                    rusqlite::params![state.server_id, code],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                );
                let (max_uses, use_count, expires_at) = row.map_err(|_| {
                    ApiError::Forbidden("Invalid or expired invite code.".to_string())
                })?;
                if let Some(ref exp) = expires_at {
                    if let Ok(exp_dt) =
                        chrono::NaiveDateTime::parse_from_str(exp, "%Y-%m-%d %H:%M:%S")
                    {
                        let now = chrono::Utc::now().naive_utc();
                        if exp_dt < now {
                            return Err(ApiError::Forbidden(
                                "Invalid or expired invite code.".to_string(),
                            ));
                        }
                    }
                }
                if let Some(max) = max_uses {
                    if use_count >= max {
                        return Err(ApiError::Forbidden(
                            "Invalid or expired invite code.".to_string(),
                        ));
                    }
                }
            }

            // Lock Merkle Tree
            let mut tree = state.merkle_tree.lock().map_err(|_| {
                ApiError::InternalServerError("merkle tree lock poisoned".to_string())
            })?;

            // Perform registration
            let result = register_identity(
                &mut tree,
                &mut conn,
                &payload.commitment_hex,
                role,
                payload.node_id,
            )
            .map_err(|e| match e {
                annex_identity::IdentityError::InvalidCommitmentFormat
                | annex_identity::IdentityError::InvalidRoleCode(_)
                | annex_identity::IdentityError::InvalidHex => ApiError::BadRequest(e.to_string()),
                annex_identity::IdentityError::DuplicateCommitment(_) => {
                    ApiError::Conflict(e.to_string())
                }
                annex_identity::IdentityError::TreeFull => {
                    ApiError::InternalServerError(e.to_string())
                }
                _ => ApiError::InternalServerError(e.to_string()),
            })?;

            // Registration succeeded — now atomically claim the invite.
            // The WHERE clause re-checks max_uses to prevent races.
            if let Some(ref code) = invite_code_for_registration {
                let updated = conn
                    .execute(
                        "UPDATE invite_codes SET use_count = use_count + 1 \
                         WHERE server_id = ?1 AND code = ?2 \
                         AND (max_uses IS NULL OR use_count < max_uses)",
                        rusqlite::params![state.server_id, code],
                    )
                    .map_err(|e| {
                        ApiError::InternalServerError(format!("invite update failed: {e}"))
                    })?;
                if updated == 0 {
                    // Another concurrent request used the last invite between
                    // our validation and this point. Registration already
                    // succeeded so we don't roll it back — the identity is
                    // valid. Log a warning for audit purposes.
                    tracing::warn!(
                        code = %code,
                        "invite code exhausted between validation and claim; \
                         registration succeeded but invite use not counted"
                    );
                }
            }

            // Emit IDENTITY_REGISTERED to the public event log
            let observe_payload = EventPayload::IdentityRegistered {
                commitment_hex: payload.commitment_hex.clone(),
                role_code: role.as_u8(),
            };
            crate::emit_and_broadcast(
                &conn,
                state.server_id,
                &payload.commitment_hex,
                &observe_payload,
                &state.observe_tx,
            );

            Ok(result)
        })
        .await
        .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(RegisterResponse {
        identity_id: result.identity_id,
        leaf_index: result.leaf_index,
        root_hex: result.root_hex,
        path_elements: result.path_elements,
        path_indices: result.path_indices,
    }))
}

/// Handler for `GET /api/registry/path/:commitmentHex`.
pub async fn get_path_handler(
    Extension(state): Extension<Arc<AppState>>,
    Path(commitment_hex): Path<String>,
) -> Result<Json<GetPathResponse>, ApiError> {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        let tree = state
            .merkle_tree
            .lock()
            .map_err(|_| ApiError::InternalServerError("merkle tree lock poisoned".to_string()))?;

        get_path_for_commitment(&tree, &conn, &commitment_hex).map_err(|e| match e {
            annex_identity::IdentityError::CommitmentNotFound(_) => {
                ApiError::NotFound(format!("commitment not found: {commitment_hex}"))
            }
            _ => ApiError::InternalServerError(e.to_string()),
        })
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(GetPathResponse {
        leaf_index: result.0,
        root_hex: result.1,
        path_elements: result.2,
        path_indices: result.3,
    }))
}

/// Handler for `GET /api/registry/current-root`.
pub async fn get_current_root_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<GetRootResponse>, ApiError> {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        let (root_hex, leaf_count) = {
            let tree = state.merkle_tree.lock().map_err(|_| {
                ApiError::InternalServerError("merkle tree lock poisoned".to_string())
            })?;
            (tree.root_hex(), tree.next_index)
        };

        let updated_at: Option<String> = conn
            .query_row(
                "SELECT created_at FROM vrp_roots WHERE root_hex = ?1",
                [&root_hex],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| ApiError::InternalServerError(format!("db query failed: {e}")))?;

        Ok((root_hex, leaf_count, updated_at))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(GetRootResponse {
        root_hex: result.0,
        leaf_count: result.1,
        updated_at: result.2,
    }))
}

/// Handler for `POST /api/zk/verify-membership`.
pub async fn verify_membership_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<VerifyMembershipRequest>,
) -> Result<Json<VerifyMembershipResponse>, ApiError> {
    let ws_token_secret = state.ws_token_secret.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        // 1. Verify root exists and is active (not a stale historical root).
        // Only active roots are accepted to prevent revoked identities from
        // generating valid proofs against old Merkle tree states.
        let root_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM vrp_roots WHERE root_hex = ?1 AND active = 1",
                [&payload.root],
                |row| row.get(0),
            )
            .map_err(|e| ApiError::InternalServerError(format!("db query failed: {e}")))
            .map(|count: i64| count > 0)?;

        if !root_exists {
            return Err(ApiError::Conflict(format!(
                "stale or invalid root: {}",
                payload.root
            )));
        }

        // 2. Parse proof and public signals
        let proof = parse_proof(&payload.proof.to_string())
            .map_err(|e| ApiError::BadRequest(format!("invalid proof format: {e}")))?;

        let public_signals_json = serde_json::to_string(&payload.public_signals).map_err(|e| {
            ApiError::BadRequest(format!("failed to serialize public signals: {e}"))
        })?;
        let public_signals = parse_public_signals(&public_signals_json)
            .map_err(|e| ApiError::BadRequest(format!("invalid public signals format: {e}")))?;

        // 3. Verify proof
        let valid = verify_proof(&state.membership_vkey, &proof, &public_signals)
            .map_err(|e| ApiError::Unauthorized(format!("proof verification failed: {e}")))?;

        if !valid {
            return Err(ApiError::Unauthorized("invalid proof".to_string()));
        }

        // 4. Verify public signals match claimed root and commitment
        // membership.circom public output: [root, commitment]
        if public_signals.len() != 2 {
            return Err(ApiError::BadRequest(
                "invalid number of public signals".to_string(),
            ));
        }

        // Convert input hex strings to Fr for comparison
        let claimed_root = parse_fr_from_hex(&payload.root)
            .map_err(|e| ApiError::BadRequest(format!("invalid root hex: {e}")))?;
        let claimed_commitment = parse_fr_from_hex(&payload.commitment)
            .map_err(|e| ApiError::BadRequest(format!("invalid commitment hex: {e}")))?;

        if public_signals[0] != claimed_root {
            return Err(ApiError::BadRequest(
                "proof root does not match claimed root".to_string(),
            ));
        }
        if public_signals[1] != claimed_commitment {
            return Err(ApiError::BadRequest(
                "proof commitment does not match claimed commitment".to_string(),
            ));
        }

        // 4b. Emit IDENTITY_VERIFIED to the public event log.
        // This must happen AFTER all validation checks pass (proof verification +
        // public signal matching) to prevent false positive audit entries that
        // could never be corrected.
        let observe_payload = EventPayload::IdentityVerified {
            commitment_hex: payload.commitment.clone(),
            topic: payload.topic.clone(),
        };
        crate::emit_and_broadcast(
            &conn,
            state.server_id,
            &payload.commitment,
            &observe_payload,
            &state.observe_tx,
        );

        // 5. Derive nullifier
        let nullifier_hex = derive_nullifier_hex(&payload.commitment, &payload.topic)
            .map_err(|e| ApiError::BadRequest(format!("failed to derive nullifier: {e}")))?;

        // 6. Derive pseudonym (pure computation, no DB needed)
        let pseudonym_id = derive_pseudonym_id(&payload.topic, &nullifier_hex).map_err(|e| {
            ApiError::InternalServerError(format!("failed to derive pseudonym: {e}"))
        })?;

        // 7. Lookup role code from vrp_identities (read-only, before transaction)
        let role_code_int: u8 = conn
            .query_row(
                "SELECT role_code FROM vrp_identities WHERE commitment_hex = ?1",
                [&payload.commitment],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| ApiError::InternalServerError(format!("db query failed: {e}")))?
            .ok_or_else(|| ApiError::NotFound("identity not found in registry".to_string()))?;

        let role_code = RoleCode::from_u8(role_code_int).ok_or_else(|| {
            ApiError::InternalServerError(format!("invalid role code in db: {role_code_int}"))
        })?;

        // 8. Get Server ID (read-only, before transaction)
        let server_id: i64 = conn
            .query_row("SELECT id FROM servers LIMIT 1", [], |row| row.get(0))
            .optional()
            .map_err(|e| ApiError::InternalServerError(format!("db query failed: {e}")))?
            .ok_or_else(|| ApiError::InternalServerError("no server configured".to_string()))?;

        // 9. Pre-fetch agent metadata if applicable (read-only, before transaction)
        let node_type = role_code_to_node_type(role_code);
        let metadata_json = if role_code == RoleCode::AiAgent {
            let agent_data: Option<(String, String, String, f64)> = conn
                .query_row(
                    "SELECT alignment_status, transfer_scope, capability_contract_json, reputation_score
                     FROM agent_registrations
                     WHERE server_id = ?1 AND pseudonym_id = ?2",
                    rusqlite::params![server_id, pseudonym_id],
                    |row| Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                    )),
                )
                .optional()
                .map_err(|e| ApiError::InternalServerError(format!("db query failed: {e}")))?;

            if let Some((alignment, scope, contract, reputation)) = agent_data {
                let parsed_contract: serde_json::Value = serde_json::from_str(&contract)
                    .map_err(|e| {
                        tracing::error!(
                            pseudonym_id = %pseudonym_id,
                            raw_contract = %contract,
                            error = %e,
                            "corrupted capability_contract_json in agent_registrations; refusing to propagate"
                        );
                        ApiError::InternalServerError(
                            "corrupted agent capability contract in database".to_string()
                        )
                    })?;
                let metadata = serde_json::json!({
                    "alignment_status": alignment,
                    "transfer_scope": scope,
                    "capability_contract": parsed_contract,
                    "reputation_score": reputation
                });
                Some(metadata.to_string())
            } else {
                None
            }
        } else {
            None
        };

        // 10. Wrap all mutating operations in a single transaction to ensure
        // atomicity: nullifier insert, identity creation, graph node, and
        // audit log entries either all succeed or all roll back.
        //
        // The previous code had two bugs:
        // (a) A TOCTOU race between check_nullifier_exists and insert_nullifier
        //     (another request could insert between the check and the insert).
        //     Fixed by removing the redundant check and relying on insert_nullifier's
        //     UNIQUE constraint handling which returns DuplicateNullifier/DuplicateCommitment on conflict.
        // (b) create_platform_identity, ensure_graph_node, and emit_and_broadcast
        //     were not wrapped in a transaction, so a failure partway through
        //     could leave inconsistent state (e.g., identity exists but no graph node).
        let mut conn = conn;
        let tx = conn.transaction().map_err(|e| {
            ApiError::InternalServerError(format!("failed to start transaction: {e}"))
        })?;

        // Insert nullifier with denormalized lookup columns for O(1) pseudonym resolution
        insert_nullifier(
            &tx,
            &payload.topic,
            &nullifier_hex,
            Some(&pseudonym_id),
            Some(&payload.commitment),
        )
        .map_err(|e| match e {
            annex_identity::IdentityError::DuplicateNullifier(_) => {
                ApiError::Conflict(e.to_string())
            }
            _ => ApiError::InternalServerError(format!("failed to insert nullifier: {e}")),
        })?;

        // Emit PSEUDONYM_DERIVED to the public event log (inside transaction)
        let observe_payload = EventPayload::PseudonymDerived {
            pseudonym_id: pseudonym_id.clone(),
            topic: payload.topic.clone(),
        };
        crate::emit_and_broadcast(
            &tx,
            state.server_id,
            &pseudonym_id,
            &observe_payload,
            &state.observe_tx,
        );

        // Create Platform Identity
        create_platform_identity(&tx, server_id, &pseudonym_id, role_code).map_err(|e| {
            ApiError::InternalServerError(format!("failed to create platform identity: {e}"))
        })?;

        // Create/Update Graph Node
        ensure_graph_node(&tx, server_id, &pseudonym_id, node_type, metadata_json).map_err(|e| {
            ApiError::InternalServerError(format!("failed to ensure graph node: {e}"))
        })?;

        // Emit NodeAdded to the public event log (inside transaction)
        let observe_payload = EventPayload::NodeAdded {
            pseudonym_id: pseudonym_id.clone(),
            node_type: format!("{node_type:?}"),
        };
        crate::emit_and_broadcast(
            &tx,
            server_id,
            &pseudonym_id,
            &observe_payload,
            &state.observe_tx,
        );

        tx.commit().map_err(|e| {
            ApiError::InternalServerError(format!("failed to commit transaction: {e}"))
        })?;

        // 11. Emit Presence Event (SSE broadcast only, no DB write needed after commit)
        let event = PresenceEvent::NodeUpdated {
            pseudonym_id: pseudonym_id.clone(),
            active: true,
        };
        let _ = state.presence_tx.send(event);

        Ok(pseudonym_id)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    let session_token = crate::api_ws::generate_session_token(
        &result,
        &ws_token_secret,
        crate::api_ws::SESSION_TOKEN_TTL_SECS,
    );

    Ok(Json(VerifyMembershipResponse {
        ok: true,
        pseudonym_id: result,
        session_token,
    }))
}

/// Handler for `GET /api/registry/topics`.
pub async fn get_topics_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<VrpTopic>>, ApiError> {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        get_all_topics(&conn).map_err(|e| ApiError::InternalServerError(e.to_string()))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(result))
}

/// Helper to fetch platform identity. Blocking.
///
/// When the fetched identity lacks moderator capabilities, this also runs
/// [`ensure_founder`] to self-heal servers that have no moderator (e.g. due
/// to stale identities preventing the normal founder bootstrap). If a
/// promotion occurs the identity is re-fetched so the caller sees the
/// updated capabilities.
fn fetch_platform_identity(
    state: &AppState,
    pseudonym_id: &str,
) -> Result<PlatformIdentity, ApiError> {
    let conn = state
        .pool
        .get()
        .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

    let identity =
        get_platform_identity(&conn, state.server_id, pseudonym_id).map_err(|e| match e {
            annex_identity::IdentityError::DatabaseError(rusqlite::Error::QueryReturnedNoRows) => {
                ApiError::NotFound(format!("identity not found: {pseudonym_id}"))
            }
            _ => ApiError::InternalServerError(e.to_string()),
        })?;

    // Self-heal: if the identity has no moderator flag, check whether the
    // server has *any* moderator. If not, promote the earliest active identity
    // and re-fetch in case this identity was the one promoted.
    //
    // SECURITY: Only the `ensure_founder` path (which promotes the EARLIEST
    // active identity, not the requester) is used here. The previous
    // stale-moderator auto-promotion was removed because it allowed any
    // identity to escalate to admin by waiting for moderators to go offline.
    if !identity.can_moderate {
        let promoted = ensure_founder(&conn, state.server_id)
            .map_err(|e| ApiError::InternalServerError(e.to_string()))?;
        if promoted {
            return get_platform_identity(&conn, state.server_id, pseudonym_id)
                .map_err(|e| ApiError::InternalServerError(e.to_string()));
        }
    }

    Ok(identity)
}

/// Handler for `GET /api/identity/:pseudonymId`.
pub async fn get_identity_handler(
    Extension(state): Extension<Arc<AppState>>,
    Path(pseudonym_id): Path<String>,
) -> Result<Json<GetIdentityResponse>, ApiError> {
    let result =
        tokio::task::spawn_blocking(move || fetch_platform_identity(&state, &pseudonym_id))
            .await
            .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(GetIdentityResponse {
        pseudonym_id: result.pseudonym_id,
        participant_type: result.participant_type,
        active: result.active,
        capabilities: Capabilities {
            can_voice: result.can_voice,
            can_moderate: result.can_moderate,
            can_invite: result.can_invite,
            can_federate: result.can_federate,
            can_bridge: result.can_bridge,
        },
    }))
}

/// Handler for `GET /api/identity/:pseudonymId/capabilities`.
pub async fn get_identity_capabilities_handler(
    Extension(state): Extension<Arc<AppState>>,
    Path(pseudonym_id): Path<String>,
) -> Result<Json<GetCapabilitiesResponse>, ApiError> {
    let result =
        tokio::task::spawn_blocking(move || fetch_platform_identity(&state, &pseudonym_id))
            .await
            .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(GetCapabilitiesResponse {
        capabilities: Capabilities {
            can_voice: result.can_voice,
            can_moderate: result.can_moderate,
            can_invite: result.can_invite,
            can_federate: result.can_federate,
            can_bridge: result.can_bridge,
        },
    }))
}

/// Handler for `GET /api/registry/roles`.
pub async fn get_roles_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<VrpRoleEntry>>, ApiError> {
    let result = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        get_all_roles(&conn).map_err(|e| ApiError::InternalServerError(e.to_string()))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(Json(result))
}
