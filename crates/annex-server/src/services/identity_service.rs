//! Orchestration for the identity-plane HTTP surface (register / path /
//! current-root / verify-membership).
//!
//! Each public method is the entire orchestration that the matching
//! handler in `api.rs` used to do inline:
//!
//!   * Acquire DB connection from the pool.
//!   * Acquire (or wait on) the Merkle lock.
//!   * Run policy / access-mode checks.
//!   * Drive `annex_identity::register_identity`, the ZK verifier, and
//!     the platform-identity / graph-node writers, all under one
//!     transaction where the data model permits.
//!   * Emit observe/presence events on success.
//!
//! Handlers in `api.rs` are reduced to: extract `Extension<Arc<AppState>>`,
//! deserialize the request, call into here, map [`IdentityServiceError`]
//! to [`crate::api::ApiError`] (via the `From` impl in `services/mod.rs`),
//! and serialize the response.

// The orchestration docstrings below use deeply nested numbered/lettered
// lists so each transactional sub-step is named in place. Clippy's
// `doc_overindented_list_items` lint wants tighter indentation, but
// flattening the nesting would erase the (step / sub-step) hierarchy that
// matters when auditing the order of DB writes.
#![allow(clippy::doc_overindented_list_items)]

use std::sync::Arc;

use annex_graph::{ensure_graph_node, role_code_to_node_type};
use annex_identity::{
    create_platform_identity, derive_nullifier_hex, derive_pseudonym_id, get_path_for_commitment,
    insert_nullifier, register_identity,
    zk::{
        fr_to_canonical_hex, parse_fr_from_hex, parse_proof, parse_public_signals,
        topic_hash_for_v2, verify_proof,
    },
    RegistrationResult, RoleCode,
};
use annex_observe::EventPayload;
use annex_types::PresenceEvent;
use rusqlite::OptionalExtension;
use thiserror::Error;

use crate::api::{
    GetPathResponse, GetRootResponse, RegisterRequest, RegisterResponse, VerifyMembershipRequest,
    VerifyMembershipResponse,
};
use crate::AppState;

/// Errors returned by [`IdentityService`]. Translated to HTTP statuses by
/// the `From<IdentityServiceError> for ApiError` impl in `services/mod.rs`.
///
/// Variants intentionally mirror the HTTP status families the previous
/// inline handlers produced — keeping the wire shape unchanged across the
/// refactor.
#[derive(Debug, Error)]
pub enum IdentityServiceError {
    /// 400 — caller-induced format / value problem.
    #[error("{0}")]
    BadRequest(String),
    /// 403 — caller authenticated/identified, but access policy rejected
    /// the request (invite-only, password-required, max members reached).
    #[error("{0}")]
    Forbidden(String),
    /// 404 — referenced resource (commitment, identity) does not exist.
    #[error("{0}")]
    NotFound(String),
    /// 409 — duplicate-state collision: stale Merkle root, double-join
    /// nullifier, etc.
    #[error("{0}")]
    Conflict(String),
    /// 401 — proof verification failed.
    #[error("{0}")]
    Unauthorized(String),
    /// 500 — internal error. Always logged before being returned.
    #[error("{0}")]
    Internal(String),
}

/// Identity-plane orchestration. Holds an `Arc<AppState>` so it can be
/// constructed cheaply per-request from a handler's `Extension<Arc<AppState>>`.
pub struct IdentityService {
    state: Arc<AppState>,
}

impl IdentityService {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// `POST /api/registry/register` orchestration.
    ///
    /// Steps:
    ///   1. Validate the role code on the request body.
    ///   2. Read the access-mode policy and pre-validate the supplied
    ///      `invite_code` / `server_password` (light validation only — the
    ///      authoritative invite check happens inside the blocking task).
    ///   3. Spawn a blocking task that holds the connection + Merkle lock:
    ///        a. Enforce `max_members`.
    ///        b. Re-check the invite (max_uses, expires_at).
    ///        c. Call `annex_identity::register_identity`. On
    ///           `DuplicateCommitment`, fall back to `get_path_for_commitment`
    ///           and return the existing path — preserves idempotent
    ///           re-registration.
    ///        d. On success, attempt to bump the invite's `use_count`. If
    ///           a concurrent request beat us to the last seat, log a
    ///           warning (compensation path; documented).
    ///        e. Emit `IdentityRegistered` to the observe bus.
    ///
    /// Invite atomicity: `register_identity` opens its own transaction
    /// internally (over `vrp_identities` + `vrp_leaves` + `vrp_merkle_*`)
    /// so the invite_code update cannot be wrapped in the same write
    /// without a signature change in `annex-identity`. The compensation
    /// path is: a successful registration whose invite update returned
    /// `0 rows affected` is logged and accepted. The user already has an
    /// identity; rolling them back would be more destructive than the
    /// over-issue. This matches the pre-refactor behaviour.
    pub async fn register_identity(
        &self,
        payload: RegisterRequest,
    ) -> Result<RegisterResponse, IdentityServiceError> {
        // 1. Validate role code (cheap, do it before any I/O).
        let role = RoleCode::from_u8(payload.role_code).ok_or_else(|| {
            IdentityServiceError::BadRequest(format!("invalid role code: {}", payload.role_code))
        })?;

        // 2. Resolve access-mode policy + early-validate invite/password.
        let access_mode = self.read_access_mode()?;
        let invite_code_for_registration = if access_mode == "invite_only" {
            let invite_code = payload.invite_code.as_deref().unwrap_or("").trim();
            if invite_code.is_empty() {
                return Err(IdentityServiceError::Forbidden(
                    "This server requires an invite code to register.".to_string(),
                ));
            }
            Some(invite_code.to_string())
        } else {
            None
        };

        if access_mode == "password" {
            let expected_password = self.read_access_password()?;
            let provided = payload.server_password.as_deref().unwrap_or("").trim();
            // Constant-time compare: prevent rate-limited timing attacks from
            // recovering byte prefixes of the access password. `String::eq`
            // is short-circuiting; `subtle::ConstantTimeEq::ct_eq` runs in
            // time independent of where the bytes diverge. Differing lengths
            // also fail the check (returns false without comparing bytes).
            use subtle::ConstantTimeEq;
            let match_ok = !provided.is_empty()
                && provided.len() == expected_password.len()
                && bool::from(provided.as_bytes().ct_eq(expected_password.as_bytes()));
            if !match_ok {
                return Err(IdentityServiceError::Forbidden(
                    "This server requires a password to register.".to_string(),
                ));
            }
        }

        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<_, IdentityServiceError> {
            let mut conn = state
                .pool
                .get()
                .map_err(|e| IdentityServiceError::Internal(format!("db connection failed: {e}")))?;

            // 3a. Enforce max_members.
            {
                let max_members = state
                    .policy
                    .read()
                    .map_err(|_| IdentityServiceError::Internal("policy lock poisoned".to_string()))?
                    .max_members;
                let current_count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM platform_identities WHERE server_id = ?1 AND active = 1",
                        rusqlite::params![state.server_id],
                        |row| row.get(0),
                    )
                    .map_err(|e| IdentityServiceError::Internal(format!("member count query failed: {e}")))?;
                if current_count >= max_members as i64 {
                    return Err(IdentityServiceError::Forbidden(
                        "This server has reached its maximum member limit.".to_string(),
                    ));
                }
            }

            // 3b. Validate invite (max_uses + expires_at) before the
            // mutating registration. The use_count bump is deferred to
            // (3d) so a registration failure doesn't burn an invite seat.
            if let Some(ref code) = invite_code_for_registration {
                let row: Result<(Option<i64>, i64, Option<String>), _> = conn.query_row(
                    "SELECT max_uses, use_count, expires_at FROM invite_codes WHERE server_id = ?1 AND code = ?2",
                    rusqlite::params![state.server_id, code],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                );
                let (max_uses, use_count, expires_at) = row.map_err(|_| {
                    IdentityServiceError::Forbidden("Invalid or expired invite code.".to_string())
                })?;
                if let Some(ref exp) = expires_at {
                    let now = chrono::Utc::now().naive_utc();
                    if crate::api_invite::invite_expires_at_is_past(exp, now) {
                        return Err(IdentityServiceError::Forbidden(
                            "Invalid or expired invite code.".to_string(),
                        ));
                    }
                }
                if let Some(max) = max_uses {
                    if use_count >= max {
                        return Err(IdentityServiceError::Forbidden(
                            "Invalid or expired invite code.".to_string(),
                        ));
                    }
                }
            }

            // 3c. Lock the Merkle tree and run the actual registration.
            let mut tree = state.merkle_tree.lock().map_err(|_| {
                IdentityServiceError::Internal("merkle tree lock poisoned".to_string())
            })?;

            let registration_result = match register_identity(
                &mut tree,
                &mut conn,
                &payload.commitment_hex,
                role,
                payload.node_id,
            ) {
                Ok(result) => result,
                Err(annex_identity::IdentityError::DuplicateCommitment(_)) => {
                    // Idempotent re-registration: client retried (or app
                    // restarted) but the commitment is already in the
                    // tree. Surface the existing leaf + path so the
                    // caller can proceed with proof generation.
                    let commitment = payload.commitment_hex.to_ascii_lowercase();
                    let (leaf_index, root_hex, path_elements, path_indices) =
                        get_path_for_commitment(&tree, &conn, &commitment).map_err(|e| {
                            IdentityServiceError::Internal(format!(
                                "duplicate commitment lookup failed: {e}"
                            ))
                        })?;
                    let identity_id: i64 = conn
                        .query_row(
                            "SELECT rowid FROM vrp_identities WHERE commitment_hex = ?1",
                            rusqlite::params![commitment],
                            |row| row.get(0),
                        )
                        .map_err(|e| {
                            IdentityServiceError::Internal(format!(
                                "duplicate commitment id lookup failed: {e}"
                            ))
                        })?;
                    tracing::info!(
                        commitment = %commitment,
                        leaf_index,
                        "idempotent re-registration: commitment already exists, returning existing path"
                    );
                    RegistrationResult {
                        identity_id,
                        leaf_index,
                        root_hex,
                        path_elements,
                        path_indices,
                    }
                }
                Err(e) => {
                    return Err(match e {
                        annex_identity::IdentityError::InvalidCommitmentFormat
                        | annex_identity::IdentityError::InvalidRoleCode(_)
                        | annex_identity::IdentityError::InvalidHex => {
                            IdentityServiceError::BadRequest(e.to_string())
                        }
                        annex_identity::IdentityError::TreeFull => {
                            IdentityServiceError::Internal(e.to_string())
                        }
                        _ => IdentityServiceError::Internal(e.to_string()),
                    });
                }
            };

            // 3d. Atomically claim the invite (re-checks max_uses to avoid
            // races with a parallel registration). If `updated == 0`, the
            // invite was exhausted between our (3b) check and now; the
            // identity is already valid so we accept it but log a
            // warning. This is the documented compensation path; making
            // it a true single-transaction atomic claim would require a
            // signature change in annex_identity::register_identity.
            if let Some(ref code) = invite_code_for_registration {
                let updated = conn
                    .execute(
                        "UPDATE invite_codes SET use_count = use_count + 1 \
                         WHERE server_id = ?1 AND code = ?2 \
                         AND (max_uses IS NULL OR use_count < max_uses)",
                        rusqlite::params![state.server_id, code],
                    )
                    .map_err(|e| {
                        IdentityServiceError::Internal(format!("invite update failed: {e}"))
                    })?;
                if updated == 0 {
                    tracing::warn!(
                        code = %code,
                        "invite code exhausted between validation and claim; \
                         registration succeeded but invite use not counted"
                    );
                }
            }

            // 3e. Audit-log emission.
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
                &state.signing_key,
            );

            Ok(registration_result)
        })
        .await
        .map_err(|e| IdentityServiceError::Internal(format!("task join error: {e}")))??;

        Ok(RegisterResponse {
            identity_id: result.identity_id,
            leaf_index: result.leaf_index,
            root_hex: result.root_hex,
            path_elements: result.path_elements,
            path_indices: result.path_indices,
        })
    }

    /// `GET /api/registry/path/:commitmentHex` orchestration.
    pub async fn get_merkle_path(
        &self,
        commitment_hex: String,
    ) -> Result<GetPathResponse, IdentityServiceError> {
        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<_, IdentityServiceError> {
            let conn = state.pool.get().map_err(|e| {
                IdentityServiceError::Internal(format!("db connection failed: {e}"))
            })?;

            let tree = state.merkle_tree.lock().map_err(|_| {
                IdentityServiceError::Internal("merkle tree lock poisoned".to_string())
            })?;

            get_path_for_commitment(&tree, &conn, &commitment_hex).map_err(|e| match e {
                annex_identity::IdentityError::CommitmentNotFound(_) => {
                    IdentityServiceError::NotFound(format!(
                        "commitment not found: {commitment_hex}"
                    ))
                }
                _ => IdentityServiceError::Internal(e.to_string()),
            })
        })
        .await
        .map_err(|e| IdentityServiceError::Internal(format!("task join error: {e}")))??;

        Ok(GetPathResponse {
            leaf_index: result.0,
            root_hex: result.1,
            path_elements: result.2,
            path_indices: result.3,
        })
    }

    /// `GET /api/registry/current-root` orchestration.
    pub async fn get_current_root(&self) -> Result<GetRootResponse, IdentityServiceError> {
        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<_, IdentityServiceError> {
            let conn = state.pool.get().map_err(|e| {
                IdentityServiceError::Internal(format!("db connection failed: {e}"))
            })?;

            let (root_hex, leaf_count) = {
                let tree = state.merkle_tree.lock().map_err(|_| {
                    IdentityServiceError::Internal("merkle tree lock poisoned".to_string())
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
                .map_err(|e| IdentityServiceError::Internal(format!("db query failed: {e}")))?;

            Ok((root_hex, leaf_count, updated_at))
        })
        .await
        .map_err(|e| IdentityServiceError::Internal(format!("task join error: {e}")))??;

        Ok(GetRootResponse {
            root_hex: result.0,
            leaf_count: result.1,
            updated_at: result.2,
        })
    }

    /// `POST /api/zk/verify-membership` orchestration.
    ///
    /// Steps:
    ///   1. Resolve `protocolVersion` (`v1` default, `v2` opt-in) BEFORE
    ///      any DB or proof work so an unknown version surfaces as 400
    ///      regardless of state.
    ///   2. Confirm the claimed root is currently active (legacy
    ///      `vrp_roots` table — kept compatible with prior behaviour).
    ///   3. Parse + verify the Groth16 proof against the version-matched
    ///      vkey. Cross-check `publicSignals[0..2]` against the claimed
    ///      root + commitment; for v2, also cross-check the prover-bound
    ///      nullifier and topicHash.
    ///   4. Resolve the canonical `nullifier_hex` (v1 = commitment-derived;
    ///      v2 = secret-derived from the proof).
    ///   5. Derive the topic-scoped pseudonym id.
    ///   6. Open a transaction and atomically:
    ///        - insert the nullifier (rejects double-joins),
    ///        - emit `PseudonymDerived`,
    ///        - upsert the platform identity,
    ///        - upsert the graph node,
    ///        - emit `NodeAdded`.
    ///   7. Broadcast a `PresenceEvent::NodeUpdated` after commit.
    ///   8. Issue an HMAC-signed session token.
    pub async fn verify_membership(
        &self,
        payload: VerifyMembershipRequest,
    ) -> Result<VerifyMembershipResponse, IdentityServiceError> {
        let state = self.state.clone();
        let ws_token_secret = state.ws_token_secret.clone();
        let pseudonym_id = tokio::task::spawn_blocking(move || -> Result<String, IdentityServiceError> {
            let protocol_version = payload.protocol_version.as_deref().unwrap_or("v1");
            let (vkey_for_proof, expected_signals_len) = match protocol_version {
                "v1" => (state.membership_vkey.clone(), 2usize),
                "v2" => {
                    let v2_key = state.membership_vkey_v2.clone().ok_or_else(|| {
                        IdentityServiceError::Conflict(
                            "membership v2 is not enabled on this server (security.enabled_zk_versions \
                             does not include \"v2\")".to_string(),
                        )
                    })?;
                    (v2_key, 4usize)
                }
                other => {
                    return Err(IdentityServiceError::BadRequest(format!(
                        "unsupported protocol_version '{other}' (expected \"v1\" or \"v2\")"
                    )));
                }
            };

            let mut conn = state
                .pool
                .get()
                .map_err(|e| IdentityServiceError::Internal(format!("db connection failed: {e}")))?;

            // 2. Reject roots that are neither the current active root nor a
            //    recently retired root inside the grace window.
            //
            // The legacy `vrp_roots WHERE active = 1` check rejected EVERY
            // proof not built against the latest root. With registrations
            // bumping the root, that race-conditioned any in-flight prover —
            // a client whose proof was generated against root_N gets a 409
            // the moment a different client registers and produces root_N+1.
            // `is_root_acceptable` consults `vrp_root_epochs` and accepts
            // the active root plus the grace window (`accepted_until`)
            // recorded at rotation time. See
            // `annex_identity::merkle::ROOT_EPOCH_GRACE_SECONDS`.
            let root_acceptable =
                annex_identity::merkle::is_root_acceptable(&conn, &payload.root)
                    .map_err(|e| IdentityServiceError::Internal(format!("root check failed: {e}")))?;
            if !root_acceptable {
                return Err(IdentityServiceError::Conflict(format!(
                    "stale or invalid root: {}",
                    payload.root
                )));
            }

            // 3. Parse + verify Groth16 proof.
            let proof = parse_proof(&payload.proof.to_string())
                .map_err(|e| IdentityServiceError::BadRequest(format!("invalid proof format: {e}")))?;

            let public_signals_json = serde_json::to_string(&payload.public_signals).map_err(|e| {
                IdentityServiceError::BadRequest(format!("failed to serialize public signals: {e}"))
            })?;
            let public_signals = parse_public_signals(&public_signals_json).map_err(|e| {
                IdentityServiceError::BadRequest(format!("invalid public signals format: {e}"))
            })?;

            if public_signals.len() != expected_signals_len {
                return Err(IdentityServiceError::BadRequest(format!(
                    "invalid number of public signals: expected {} for protocol_version '{}', got {}",
                    expected_signals_len,
                    protocol_version,
                    public_signals.len()
                )));
            }

            let valid = verify_proof(&vkey_for_proof, &proof, &public_signals).map_err(|e| {
                IdentityServiceError::Unauthorized(format!("proof verification failed: {e}"))
            })?;
            if !valid {
                return Err(IdentityServiceError::Unauthorized("invalid proof".to_string()));
            }

            // Cross-check public signals against the claimed root + commitment.
            let claimed_root = parse_fr_from_hex(&payload.root)
                .map_err(|e| IdentityServiceError::BadRequest(format!("invalid root hex: {e}")))?;
            let claimed_commitment = parse_fr_from_hex(&payload.commitment).map_err(|e| {
                IdentityServiceError::BadRequest(format!("invalid commitment hex: {e}"))
            })?;
            if public_signals[0] != claimed_root {
                return Err(IdentityServiceError::BadRequest(
                    "proof root does not match claimed root".to_string(),
                ));
            }
            if public_signals[1] != claimed_commitment {
                return Err(IdentityServiceError::BadRequest(
                    "proof commitment does not match claimed commitment".to_string(),
                ));
            }

            // v2-specific: extract the secret-derived nullifier and topicHash
            // from publicSignals[2]/[3], and bind them to `payload.topic`.
            //
            // The server MUST recompute the expected topicHash from the
            // caller's claimed `payload.topic` and reject the request if it
            // does not match `publicSignals[3]`. Without this binding a
            // malicious prover could produce a v2 proof for topic A and
            // submit it as a v2 proof for topic B — getting a
            // nullifier-bound pseudonym in topic B without ever proving
            // membership for topic B. See
            // `annex_identity::zk::topic_hash_for_v2` for the canonical
            // mapping.
            let nullifier_v2_hex = if protocol_version == "v2" {
                let nh = fr_to_canonical_hex(public_signals[2]);
                if let Some(claimed) = payload.nullifier_hex.as_deref() {
                    if claimed.to_ascii_lowercase() != nh {
                        return Err(IdentityServiceError::BadRequest(
                            "claimed nullifierHex does not match proof's public signal".to_string(),
                        ));
                    }
                }
                let expected_topic_hash = topic_hash_for_v2(&payload.topic).map_err(|e| {
                    IdentityServiceError::BadRequest(format!(
                        "failed to derive topicHash for v2 proof: {e}"
                    ))
                })?;
                if public_signals[3] != expected_topic_hash {
                    return Err(IdentityServiceError::BadRequest(
                        "v2 proof's topicHash public signal does not match the canonical \
                         hash of payload.topic — the proof is bound to a different topic"
                            .to_string(),
                    ));
                }
                if let Some(claimed_topic_hash) = payload.topic_hash_hex.as_deref() {
                    let claimed_th = parse_fr_from_hex(claimed_topic_hash).map_err(|e| {
                        IdentityServiceError::BadRequest(format!("invalid topicHashHex: {e}"))
                    })?;
                    if claimed_th != expected_topic_hash {
                        return Err(IdentityServiceError::BadRequest(
                            "claimed topicHashHex does not match the canonical hash of \
                             payload.topic"
                                .to_string(),
                        ));
                    }
                }
                Some(nh)
            } else {
                None
            };

            // Audit-log: identity_verified (post-validation, pre-mutation).
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
                &state.signing_key,
            );

            // 4. Resolve canonical nullifier_hex per protocol version.
            let nullifier_hex = match protocol_version {
                "v1" => derive_nullifier_hex(&payload.commitment, &payload.topic).map_err(|e| {
                    IdentityServiceError::BadRequest(format!("failed to derive nullifier: {e}"))
                })?,
                "v2" => nullifier_v2_hex
                    .clone()
                    .expect("v2 path always populates nullifier_v2_hex"),
                other => {
                    return Err(IdentityServiceError::BadRequest(format!(
                        "unsupported protocol_version '{other}'"
                    )));
                }
            };

            // 5. Derive pseudonym id.
            let pseudonym_id = derive_pseudonym_id(&payload.topic, &nullifier_hex).map_err(|e| {
                IdentityServiceError::Internal(format!("failed to derive pseudonym: {e}"))
            })?;

            // Pre-fetch role (read-only, cheap, before transaction).
            let role_code_int: u8 = conn
                .query_row(
                    "SELECT role_code FROM vrp_identities WHERE commitment_hex = ?1",
                    [&payload.commitment],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| IdentityServiceError::Internal(format!("db query failed: {e}")))?
                .ok_or_else(|| {
                    IdentityServiceError::NotFound("identity not found in registry".to_string())
                })?;
            let role_code = RoleCode::from_u8(role_code_int).ok_or_else(|| {
                IdentityServiceError::Internal(format!("invalid role code in db: {role_code_int}"))
            })?;

            let server_id: i64 = conn
                .query_row("SELECT id FROM servers LIMIT 1", [], |row| row.get(0))
                .optional()
                .map_err(|e| IdentityServiceError::Internal(format!("db query failed: {e}")))?
                .ok_or_else(|| {
                    IdentityServiceError::Internal("no server configured".to_string())
                })?;

            let node_type = role_code_to_node_type(role_code);
            let metadata_json = if role_code == RoleCode::AiAgent {
                let agent_data: Option<(String, String, String, f64)> = conn
                    .query_row(
                        "SELECT alignment_status, transfer_scope, capability_contract_json, reputation_score
                         FROM agent_registrations
                         WHERE server_id = ?1 AND pseudonym_id = ?2",
                        rusqlite::params![server_id, pseudonym_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .optional()
                    .map_err(|e| IdentityServiceError::Internal(format!("db query failed: {e}")))?;
                if let Some((alignment, scope, contract, reputation)) = agent_data {
                    let parsed_contract: serde_json::Value =
                        serde_json::from_str(&contract).map_err(|e| {
                            tracing::error!(
                                pseudonym_id = %pseudonym_id,
                                raw_contract = %contract,
                                error = %e,
                                "corrupted capability_contract_json in agent_registrations; refusing to propagate"
                            );
                            IdentityServiceError::Internal(
                                "corrupted agent capability contract in database".to_string(),
                            )
                        })?;
                    let metadata = serde_json::json!({
                        "alignment_status": alignment,
                        "transfer_scope": scope,
                        "capability_contract": parsed_contract,
                        "reputation_score": reputation,
                    });
                    Some(metadata.to_string())
                } else {
                    None
                }
            } else {
                None
            };

            // 6. Atomic mutation block.
            // IMMEDIATE — this block reads the nullifier and the tree before
            // writing both, which is the snapshot-conflict shape.
            let tx = conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|e| {
                IdentityServiceError::Internal(format!("failed to start transaction: {e}"))
            })?;

            // The nullifier is single-use per topic — that is what stops one
            // identity claiming two pseudonyms in the same topic. But it is
            // ALSO presented every time an existing member re-authenticates,
            // and treating that as an error locked members out permanently:
            //
            //   1. Alice joins; her cached proof binds to Merkle root R1.
            //   2. Bob joins; the root becomes R2.
            //   3. Alice returns. Her cached proof no longer matches the
            //      current root, so the client re-proves against R2 and
            //      re-submits here.
            //   4. Her nullifier is already consumed -> 409 -> she can never
            //      get back in, on a server she is a legitimate member of.
            //
            // On any multi-user server that locked out every member except
            // the most recent joiner.
            //
            // Re-presenting the nullifier grants no new capability: the
            // caller has already produced a valid Groth16 proof against an
            // acceptable root, with `publicSignals` cross-checked against the
            // claimed commitment, so they have demonstrated knowledge of the
            // preimage. The pseudonym derives deterministically from
            // (topic, nullifier), so a re-verification necessarily resolves
            // to the SAME pseudonym — the double-join property is preserved.
            //
            // A different commitment arriving on an already-consumed
            // nullifier is a real conflict and still rejected.
            let reauthenticating = match insert_nullifier(
                &tx,
                &payload.topic,
                &nullifier_hex,
                Some(&pseudonym_id),
                Some(&payload.commitment),
            ) {
                Ok(()) => false,
                Err(annex_identity::IdentityError::DuplicateNullifier(_)) => {
                    let owner = annex_identity::existing_nullifier_owner(
                        &tx,
                        &payload.topic,
                        &nullifier_hex,
                    )
                    .map_err(|e| {
                        IdentityServiceError::Internal(format!(
                            "failed to resolve existing nullifier owner: {e}"
                        ))
                    })?
                    .ok_or_else(|| {
                        IdentityServiceError::Internal(
                            "nullifier reported as duplicate but no row found".to_string(),
                        )
                    })?;

                    match owner.commitment_hex.as_deref() {
                        Some(existing)
                            if !existing.eq_ignore_ascii_case(&payload.commitment) =>
                        {
                            tracing::warn!(
                                topic = %payload.topic,
                                "nullifier presented by a different commitment than the one that \
                                 consumed it; rejecting"
                            );
                            return Err(IdentityServiceError::Conflict(
                                "nullifier already bound to a different identity".to_string(),
                            ));
                        }
                        Some(_) => {}
                        None => {
                            // Pre-migration-024 row: no denormalised binding
                            // to compare against. The proof already
                            // established ownership, so record it now.
                            annex_identity::backfill_nullifier_owner(
                                &tx,
                                &payload.topic,
                                &nullifier_hex,
                                &pseudonym_id,
                                &payload.commitment,
                            )
                            .map_err(|e| {
                                IdentityServiceError::Internal(format!(
                                    "failed to backfill nullifier owner: {e}"
                                ))
                            })?;
                        }
                    }
                    true
                }
                Err(e) => {
                    return Err(IdentityServiceError::Internal(format!(
                        "failed to insert nullifier: {e}"
                    )));
                }
            };

            // `PseudonymDerived` records the moment a pseudonym came into
            // existence. A returning member's pseudonym was derived on their
            // first visit, so re-emitting it on every re-authentication would
            // put a false "derived" entry in the signed, hash-chained audit
            // log once per login.
            if !reauthenticating {
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
                    &state.signing_key,
                );
            }

            // `create_platform_identity` is a plain INSERT (its founder
            // election is a TOCTOU-free sub-SELECT that only makes sense on
            // first insert), so it must not run again for a returning member.
            // `ensure_graph_node` below IS an upsert and should still run —
            // it refreshes `last_seen_at`, which is exactly what a returning
            // member's presence needs.
            if !reauthenticating {
                create_platform_identity(&tx, server_id, &pseudonym_id, role_code).map_err(|e| {
                    IdentityServiceError::Internal(format!(
                        "failed to create platform identity: {e}"
                    ))
                })?;
            }

            ensure_graph_node(&tx, server_id, &pseudonym_id, node_type, metadata_json).map_err(
                |e| IdentityServiceError::Internal(format!("failed to ensure graph node: {e}")),
            )?;

            // Likewise `NodeAdded` — the node is added once. Re-authentication
            // refreshes it (via the `ensure_graph_node` upsert above) and is
            // reported by the post-commit `NodeUpdated` presence broadcast.
            if !reauthenticating {
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
                    &state.signing_key,
                );
            }

            tx.commit().map_err(|e| {
                IdentityServiceError::Internal(format!("failed to commit transaction: {e}"))
            })?;

            // 7. Presence broadcast (post-commit, no DB write).
            let event = PresenceEvent::NodeUpdated {
                pseudonym_id: pseudonym_id.clone(),
                active: true,
            };
            let _ = state.presence_tx.send(event);

            Ok(pseudonym_id)
        })
        .await
        .map_err(|e| IdentityServiceError::Internal(format!("task join error: {e}")))??;

        // 8. Mint session token. Pure computation; lives in api_ws so we
        //    keep the constants and HMAC keying in one place.
        let session_token = crate::api_ws::generate_session_token(
            &pseudonym_id,
            &ws_token_secret,
            crate::api_ws::SESSION_TOKEN_TTL_SECS,
        );

        Ok(VerifyMembershipResponse {
            ok: true,
            pseudonym_id,
            session_token,
        })
    }

    // ── helpers ──────────────────────────────────────────────────────────

    fn read_access_mode(&self) -> Result<String, IdentityServiceError> {
        let policy = self
            .state
            .policy
            .read()
            .map_err(|_| IdentityServiceError::Internal("policy lock poisoned".to_string()))?;
        Ok(policy.access_mode.clone())
    }

    fn read_access_password(&self) -> Result<String, IdentityServiceError> {
        let policy = self
            .state
            .policy
            .read()
            .map_err(|_| IdentityServiceError::Internal("policy lock poisoned".to_string()))?;
        Ok(policy.access_password.clone())
    }
}
