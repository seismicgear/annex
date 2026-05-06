//! Federation orchestration: handshake, attestation, channel join,
//! receive-message, RTX receive, and outbound relay.
//!
//! Each public method is the orchestration the matching `api_federation`
//! handler used to do inline:
//!
//!   * Acquire a DB connection from the pool inside a blocking task.
//!   * Resolve the remote `instances` row by `base_url` and reject
//!     unknown / non-`ACTIVE` peers.
//!   * Verify the active `federation_agreements` row exists (and, for
//!     RTX, that its `transfer_scope` clears `ReflectionSummariesOnly`).
//!   * Verify the Ed25519 signature on the wire payload (canonical
//!     newline-delimited message). The verifier is centralised in
//!     [`verify_ed25519`] so all three call sites share one
//!     implementation; signature checks are NEVER skipped on any path.
//!   * For attestation: fetch the remote VRP root and verify the
//!     Groth16 proof against `(root, commitment)` public inputs.
//!   * For received messages: stale-attestation check (compare the
//!     remote's current root against the one recorded at attestation
//!     time), channel federation_scope check, local membership check,
//!     idempotent message insert, in-process broadcast.
//!   * For RTX receive: circular-relay rejection, transfer-scope gate,
//!     bundle-structure validation, redacted-topic enforcement,
//!     transactional bundle insert + transfer log + per-subscriber
//!     deliveries.
//!
//! HTTP handlers in `api_federation.rs` are reduced to: extract
//! `Extension<Arc<AppState>>`, deserialize the request, call into here,
//! map the result. The error type [`FederationError`] is re-exported by
//! `api_federation.rs` so external imports are unaffected, and its
//! `IntoResponse` impl drives the same status-code / JSON-body shape
//! the previous inline handler used.

use std::sync::Arc;
use std::time::Duration;

use annex_channels::{
    add_member, create_message, list_federated_channels, Channel, CreateMessageParams,
};
use annex_federation::{
    process_incoming_handshake, AttestationRequest, FederatedMessageEnvelope, FederatedRtxEnvelope,
    HandshakeError,
};
use annex_graph::{ensure_graph_node, GraphError};
use annex_identity::{
    derive_nullifier_hex, derive_pseudonym_id,
    zk::{parse_fr_from_hex, parse_proof, verify_proof},
};
use annex_observe::EventPayload;
use annex_rtx::{check_redacted_topics, enforce_transfer_scope, validate_bundle_structure};
use annex_types::NodeType;
use annex_vrp::{VrpTransferScope, VrpValidationReport};
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey as EdVerifyingKey};
use thiserror::Error;

use crate::api::GetRootResponse;
use crate::api_rtx::rtx_relay_signing_payload;
use crate::api_ws::OutgoingMessage;
use crate::parse_transfer_scope;
use crate::services::federation_repository as repo;
use crate::AppState;

/// Timeout for outbound federation HTTP requests (connect + total).
pub(crate) const FEDERATION_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const FEDERATION_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Builds a reqwest client with timeouts to prevent resource exhaustion
/// from slow or malicious federation peers. Re-exported by
/// `api_federation::federation_http_client`.
pub fn federation_http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .connect_timeout(FEDERATION_HTTP_CONNECT_TIMEOUT)
        .timeout(FEDERATION_HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// Wire-format request body for `POST /api/federation/handshake`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct HandshakeRequest {
    /// Base URL of the requesting server (to identify the instance).
    pub base_url: String,
    /// Ed25519 signature (hex) over the canonical handshake JSON.
    /// Binds the handshake to the claimed server identity.
    pub signature: String,
    /// The VRP handshake payload.
    #[serde(flatten)]
    pub handshake: annex_vrp::VrpFederationHandshake,
}

/// Wire-format request body for `POST /api/federation/channels/:id/join`.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct JoinFederatedChannelRequest {
    /// The base URL of the originating server.
    pub originating_server: String,
    /// The pseudonym ID of the participant joining.
    pub pseudonym_id: String,
    /// Signature of `channel_id\npseudonym_id`.
    pub signature: String,
}

#[derive(Debug, Error)]
pub enum FederationError {
    #[error("Handshake failed: {0}")]
    Handshake(#[from] HandshakeError),
    #[error("Database error: {0}")]
    DbError(#[from] rusqlite::Error),
    #[error("Unknown remote instance: {0}")]
    UnknownRemote(String),
    #[error("Server policy lock poisoned")]
    LockPoisoned,
    #[error("Invalid signature: {0}")]
    InvalidSignature(String),
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Remote server error: {0}")]
    RemoteServer(String),
    #[error("ZK Verification failed: {0}")]
    ZkVerification(String),
    #[error("Identity derivation failed: {0}")]
    IdentityDerivation(String),
    #[error("Channel error: {0}")]
    Channel(#[from] annex_channels::ChannelError),
    #[error("Forbidden: {0}")]
    Forbidden(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl axum::response::IntoResponse for FederationError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            FederationError::Handshake(HandshakeError::UnknownRemoteInstance) => {
                (axum::http::StatusCode::NOT_FOUND, self.to_string())
            }
            FederationError::UnknownRemote(_) => {
                (axum::http::StatusCode::NOT_FOUND, self.to_string())
            }
            FederationError::Forbidden(_) => (axum::http::StatusCode::FORBIDDEN, self.to_string()),
            FederationError::InvalidSignature(_) => {
                (axum::http::StatusCode::UNAUTHORIZED, self.to_string())
            }
            FederationError::ZkVerification(_) => {
                (axum::http::StatusCode::BAD_REQUEST, self.to_string())
            }
            FederationError::IdentityDerivation(_) => {
                (axum::http::StatusCode::BAD_REQUEST, self.to_string())
            }
            FederationError::Serialization(_) => {
                (axum::http::StatusCode::BAD_REQUEST, self.to_string())
            }
            FederationError::Handshake(HandshakeError::Vrp(_)) => {
                tracing::error!("federation internal error: {}", self);
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
            FederationError::Handshake(_) => {
                (axum::http::StatusCode::BAD_REQUEST, self.to_string())
            }
            FederationError::Channel(annex_channels::ChannelError::NotFound(_)) => {
                (axum::http::StatusCode::NOT_FOUND, self.to_string())
            }
            _ => {
                tracing::error!("federation internal error: {}", self);
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };
        (status, axum::Json(serde_json::json!({ "error": message }))).into_response()
    }
}

/// Wraps an `r2d2::Error` returned from `pool.get()` as a
/// `FederationError::DbError`. Matches the existing inline pattern,
/// where the pool error is round-tripped through
/// `rusqlite::Error::ToSqlConversionFailure`.
fn pool_err<E: std::error::Error + Send + Sync + 'static>(e: E) -> FederationError {
    FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

/// Centralised Ed25519 verification for federation payloads. All three
/// signature-bearing endpoints (handshake, attestation, message,
/// channel-join, RTX) go through this helper so the hex/length/verify
/// chain is implemented once.
pub(crate) fn verify_ed25519(
    public_key_hex: &str,
    signature_hex: &str,
    message: &[u8],
) -> Result<(), FederationError> {
    let public_key_bytes = hex::decode(public_key_hex)
        .map_err(|e| FederationError::InvalidSignature(format!("Invalid public key hex: {e}")))?;
    let signature_bytes = hex::decode(signature_hex)
        .map_err(|e| FederationError::InvalidSignature(format!("Invalid signature hex: {e}")))?;

    let public_key =
        EdVerifyingKey::from_bytes(&public_key_bytes.try_into().map_err(|_| {
            FederationError::InvalidSignature("Invalid public key length".to_string())
        })?)
        .map_err(|e| FederationError::InvalidSignature(e.to_string()))?;

    let signature =
        Signature::from_bytes(&signature_bytes.try_into().map_err(|_| {
            FederationError::InvalidSignature("Invalid signature length".to_string())
        })?);

    public_key
        .verify(message, &signature)
        .map_err(|e| FederationError::InvalidSignature(e.to_string()))
}

/// Canonical newline-delimited signing input for a federated message.
/// Newline delimiters prevent field-boundary ambiguity (e.g.
/// `message_id="ab" + channel_id="c"` would collide with `"a" + "bc"`
/// without delimiters). Used by both the relay and receive paths.
pub(crate) fn message_signing_input(envelope: &FederatedMessageEnvelope) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        envelope.message_id,
        envelope.channel_id,
        envelope.content,
        envelope.sender_pseudonym,
        envelope.originating_server,
        envelope.attestation_ref,
        envelope.created_at
    )
}

/// Parses an attestation reference string into `(commitment_hex, topic)`.
///
/// The expected format is `"topic:commitment_hex"` where both parts are non-empty.
/// Topics may contain colons (e.g., `"annex:server:v1:abc123"`), so the split
/// happens on the *last* colon.
pub(crate) fn parse_attestation_ref(
    attestation_ref: &str,
) -> Result<(&str, &str), FederationError> {
    match attestation_ref.rsplit_once(':') {
        Some((topic, commitment)) if !topic.is_empty() && !commitment.is_empty() => {
            Ok((commitment, topic))
        }
        _ => Err(FederationError::Forbidden(
            "Invalid attestation ref format".to_string(),
        )),
    }
}

/// Federation orchestration. Holds an `Arc<AppState>` so it can be
/// constructed cheaply per-request from a handler's
/// `Extension<Arc<AppState>>`.
pub struct FederationService {
    state: Arc<AppState>,
}

impl FederationService {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// `POST /api/federation/handshake` orchestration.
    pub async fn process_handshake(
        &self,
        payload: HandshakeRequest,
    ) -> Result<VrpValidationReport, FederationError> {
        let state = self.state.clone();

        tokio::task::spawn_blocking(move || {
            let mut conn = state.pool.get().map_err(pool_err)?;

            // 1. Resolve remote instance ID and public key from base_url
            tracing::debug!("Resolving instance for base_url: {}", payload.base_url);
            let (remote_instance_id, public_key_hex) =
                repo::find_instance_id_and_key(&conn, &payload.base_url)
                    .map_err(|e| {
                        tracing::error!("Instance resolution failed: {:?}", e);
                        FederationError::DbError(e)
                    })?
                    .ok_or_else(|| FederationError::UnknownRemote(payload.base_url.clone()))?;

            // 1b. Verify Ed25519 signature over the handshake payload.
            let handshake_json = serde_json::to_string(&payload.handshake)?;
            let signing_payload = format!("{}\n{}", payload.base_url, handshake_json);
            verify_ed25519(
                &public_key_hex,
                &payload.signature,
                signing_payload.as_bytes(),
            )?;

            // 2. Process handshake
            tracing::debug!(
                "Processing handshake for instance id: {}",
                remote_instance_id
            );
            let policy = state
                .policy
                .read()
                .map_err(|_| FederationError::LockPoisoned)?;

            let report = process_incoming_handshake(
                &mut conn,
                state.server_id,
                &policy,
                remote_instance_id,
                &payload.handshake,
            )
            .map_err(|e| {
                tracing::error!("Handshake failed: {:?}", e);
                FederationError::Handshake(e)
            })?;

            // Emit FEDERATION_ESTABLISHED to persistent log
            let observe_payload = EventPayload::FederationEstablished {
                remote_url: payload.base_url.clone(),
                alignment_status: report.alignment_status.to_string(),
            };
            crate::emit_and_broadcast(
                &conn,
                state.server_id,
                &payload.base_url,
                &observe_payload,
                &state.observe_tx,
            );

            Ok::<_, FederationError>(report)
        })
        .await
        .map_err(pool_err)?
    }

    /// `GET /api/federation/vrp-root` orchestration.
    pub async fn current_vrp_root(&self) -> Result<GetRootResponse, FederationError> {
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || {
            let (root_hex, leaf_count) = {
                let tree = state
                    .merkle_tree
                    .lock()
                    .map_err(|_| FederationError::LockPoisoned)?;
                (tree.root_hex(), tree.next_index)
            };

            let conn = state.pool.get().map_err(pool_err)?;
            let updated_at = repo::root_updated_at(&conn, &root_hex)?;

            Ok::<_, FederationError>(GetRootResponse {
                root_hex,
                leaf_count,
                updated_at,
            })
        })
        .await
        .map_err(pool_err)?
    }

    /// `POST /api/federation/attest-membership` orchestration.
    /// Returns the `pseudonym_id` derived locally for the attested identity.
    pub async fn attest_membership(
        &self,
        payload: AttestationRequest,
    ) -> Result<String, FederationError> {
        // 0. Reject HUMAN participant_type — only local identity proofs may
        //    attest HUMAN status. Federation peers must not be able to inject
        //    HUMAN-typed identities.
        if payload.participant_type == "HUMAN" {
            return Err(FederationError::ZkVerification(
                "HUMAN participant_type is not permitted via federation attestation".to_string(),
            ));
        }

        // 1. Verify Request Origin (Resolve Instance)
        let originating_server = payload.originating_server.clone();
        let state = self.state.clone();
        let (remote_instance_id, public_key_hex) = tokio::task::spawn_blocking({
            let state = state.clone();
            move || {
                let conn = state.pool.get().map_err(pool_err)?;
                repo::find_instance_id_and_key(&conn, &originating_server)
                    .map_err(FederationError::DbError)?
                    .ok_or_else(|| FederationError::UnknownRemote(originating_server.clone()))
            }
        })
        .await
        .map_err(pool_err)??;

        // Verify Signature (newline-delimited to prevent field-boundary ambiguity)
        let message = format!(
            "{}\n{}\n{}",
            payload.topic, payload.commitment, payload.participant_type
        );
        verify_ed25519(&public_key_hex, &payload.signature, message.as_bytes())?;

        // 2. Fetch Remote Root (with timeout and redirect protection against SSRF)
        let client = federation_http_client()?;
        let root_url = format!("{}/api/federation/vrp-root", payload.originating_server);
        let resp = client.get(&root_url).send().await?;

        if !resp.status().is_success() {
            return Err(FederationError::RemoteServer(format!(
                "Failed to fetch root: {}",
                resp.status()
            )));
        }

        let root_response: GetRootResponse = resp.json().await?;
        let remote_root_hex = root_response.root_hex;

        // 3. Verify ZK Proof
        let proof = parse_proof(&payload.proof.to_string())
            .map_err(|e| FederationError::ZkVerification(format!("Invalid proof format: {e}")))?;

        let remote_root_fr = parse_fr_from_hex(&remote_root_hex)
            .map_err(|e| FederationError::ZkVerification(format!("Invalid root hex: {e}")))?;
        let commitment_fr = parse_fr_from_hex(&payload.commitment)
            .map_err(|e| FederationError::ZkVerification(format!("Invalid commitment hex: {e}")))?;

        let public_inputs = vec![remote_root_fr, commitment_fr];

        let valid = verify_proof(&state.membership_vkey, &proof, &public_inputs).map_err(|e| {
            FederationError::ZkVerification(format!("Proof verification error: {e}"))
        })?;

        if !valid {
            return Err(FederationError::ZkVerification("Invalid proof".to_string()));
        }

        // 4. Persist Attestation (federated_identities + platform_identities + graph node)
        //    in a single transaction.
        tokio::task::spawn_blocking(move || {
            let mut conn = state.pool.get().map_err(pool_err)?;

            // Derive local identifiers
            let nullifier_hex =
                derive_nullifier_hex(&payload.commitment, &payload.topic).map_err(|e| {
                    FederationError::IdentityDerivation(format!("Failed to derive nullifier: {e}"))
                })?;
            let pseudonym_id =
                derive_pseudonym_id(&payload.topic, &nullifier_hex).map_err(|e| {
                    FederationError::IdentityDerivation(format!("Failed to derive pseudonym: {e}"))
                })?;

            let node_type = match payload.participant_type.as_str() {
                "HUMAN" => {
                    return Err(FederationError::ZkVerification(
                        "HUMAN participant_type is not permitted via federation attestation"
                            .to_string(),
                    ));
                }
                "AI_AGENT" => NodeType::AiAgent,
                "COLLECTIVE" => NodeType::Collective,
                "BRIDGE" => NodeType::Bridge,
                "SERVICE" => NodeType::Service,
                _ => {
                    return Err(FederationError::ZkVerification(format!(
                        "unknown participant type: {}",
                        payload.participant_type
                    )));
                }
            };

            let tx = conn.transaction().map_err(FederationError::DbError)?;

            repo::upsert_federated_identity(
                &tx,
                state.server_id,
                remote_instance_id,
                &payload.commitment,
                &pseudonym_id,
                &payload.topic,
                &remote_root_hex,
            )
            .map_err(FederationError::DbError)?;

            repo::upsert_platform_identity(
                &tx,
                state.server_id,
                &pseudonym_id,
                &payload.participant_type,
            )
            .map_err(FederationError::DbError)?;

            ensure_graph_node(
                &tx,
                state.server_id,
                &pseudonym_id,
                node_type,
                None, // metadata_json
            )
            .map_err(|e| match e {
                GraphError::DatabaseError(err) => FederationError::DbError(err),
                _ => FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e))),
            })?;

            tx.commit().map_err(FederationError::DbError)?;

            Ok::<_, FederationError>(pseudonym_id)
        })
        .await
        .map_err(pool_err)?
    }

    /// `GET /api/federation/channels` orchestration.
    pub async fn list_federated_channels(&self) -> Result<Vec<Channel>, FederationError> {
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || {
            let conn = state.pool.get().map_err(pool_err)?;
            list_federated_channels(&conn, state.server_id).map_err(FederationError::Channel)
        })
        .await
        .map_err(pool_err)?
    }

    /// `POST /api/federation/channels/:channelId/join` orchestration.
    pub async fn join_federated_channel(
        &self,
        channel_id: String,
        payload: JoinFederatedChannelRequest,
    ) -> Result<(), FederationError> {
        let state = self.state.clone();

        tokio::task::spawn_blocking(move || {
            let conn = state.pool.get().map_err(pool_err)?;

            // 1. Verify Originating Server is known + ACTIVE
            let instance = repo::find_instance_by_base_url(&conn, &payload.originating_server)
                .map_err(FederationError::DbError)?
                .ok_or_else(|| {
                    FederationError::UnknownRemote(payload.originating_server.clone())
                })?;

            if instance.status != "ACTIVE" {
                return Err(FederationError::Forbidden(format!(
                    "Instance {} is not active",
                    payload.originating_server
                )));
            }

            // 1.5. Verify Active Federation Agreement
            if !repo::has_active_agreement(&conn, instance.id).map_err(FederationError::DbError)? {
                return Err(FederationError::Forbidden(format!(
                    "No active federation agreement with {}",
                    payload.originating_server
                )));
            }

            // 2. Verify Signature (newline-delimited to prevent field-boundary ambiguity)
            let message = format!("{}\n{}", channel_id, payload.pseudonym_id);
            verify_ed25519(
                &instance.public_key_hex,
                &payload.signature,
                message.as_bytes(),
            )?;

            // 3. Verify Federated Identity Exists for this remote
            if !repo::federated_identity_exists(&conn, instance.id, &payload.pseudonym_id)
                .map_err(FederationError::DbError)?
            {
                return Err(FederationError::Forbidden(format!(
                    "Identity {} not attested for instance {}",
                    payload.pseudonym_id, payload.originating_server
                )));
            }

            // 4. Add Member
            add_member(&conn, state.server_id, &channel_id, &payload.pseudonym_id)
                .map_err(FederationError::Channel)?;

            Ok::<(), FederationError>(())
        })
        .await
        .map_err(pool_err)?
    }

    /// `POST /api/federation/messages` orchestration. Persists the
    /// envelope and broadcasts the resulting message to local
    /// subscribers. Idempotent: a duplicate message_id is accepted
    /// without re-broadcasting.
    pub async fn receive_federated_message(
        &self,
        envelope: FederatedMessageEnvelope,
    ) -> Result<(), FederationError> {
        let state = self.state.clone();
        let channel_id_for_broadcast = envelope.channel_id.clone();

        let inserted = tokio::task::spawn_blocking({
            let state = state.clone();
            move || {
                let conn = state.pool.get().map_err(pool_err)?;

                // 1. Resolve Remote Instance
                let instance = repo::find_instance_by_base_url(&conn, &envelope.originating_server)
                    .map_err(FederationError::DbError)?
                    .ok_or_else(|| {
                        FederationError::UnknownRemote(envelope.originating_server.clone())
                    })?;

                if instance.status != "ACTIVE" {
                    return Err(FederationError::Forbidden(format!(
                        "Instance {} is not active",
                        envelope.originating_server
                    )));
                }

                // 1.5. Verify Active Federation Agreement
                if !repo::has_active_agreement(&conn, instance.id)
                    .map_err(FederationError::DbError)?
                {
                    return Err(FederationError::Forbidden(format!(
                        "No active federation agreement with {}",
                        envelope.originating_server
                    )));
                }

                // 2. Verify Signature
                let signing_input = message_signing_input(&envelope);
                verify_ed25519(
                    &instance.public_key_hex,
                    &envelope.signature,
                    signing_input.as_bytes(),
                )?;

                // 3. Parse Attestation Ref to get Commitment and Topic
                let (commitment_hex, _topic) = parse_attestation_ref(&envelope.attestation_ref)?;

                // 4. Verify Sender in Federated Identities
                let identity =
                    repo::find_federated_identity_by_commitment(&conn, instance.id, commitment_hex)
                        .map_err(FederationError::DbError)?
                        .ok_or_else(|| {
                            FederationError::Forbidden(format!(
                                "Identity with commitment {commitment_hex} not attested"
                            ))
                        })?;

                // 4.5. Stale attestation check: if a root was recorded at
                // verification time, compare against the remote's current
                // root. A mismatch means the remote Merkle tree has changed
                // since attestation, so the proof may no longer be valid.
                if !identity.root_hex_at_verification.is_empty() {
                    let root_url =
                        format!("{}/api/federation/vrp-root", envelope.originating_server);
                    let remote_root_result: Result<String, String> = (|| {
                        let client = reqwest::blocking::Client::builder()
                            .connect_timeout(FEDERATION_HTTP_CONNECT_TIMEOUT)
                            .timeout(FEDERATION_HTTP_TIMEOUT)
                            .redirect(reqwest::redirect::Policy::none())
                            .build()
                            .map_err(|e| e.to_string())?;
                        let resp = client.get(&root_url).send().map_err(|e| e.to_string())?;
                        if !resp.status().is_success() {
                            return Err(format!("remote vrp-root returned {}", resp.status()));
                        }
                        let body: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
                        body.get("root_hex")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .ok_or_else(|| "missing root_hex in response".to_string())
                    })();

                    match remote_root_result {
                        Ok(current_remote_root)
                            if current_remote_root != identity.root_hex_at_verification =>
                        {
                            tracing::warn!(
                                sender = %envelope.sender_pseudonym,
                                originating_server = %envelope.originating_server,
                                stored_root = %identity.root_hex_at_verification,
                                current_root = %current_remote_root,
                                "federated identity attestation is stale: remote root has changed"
                            );
                            return Err(FederationError::Forbidden(
                                "attestation stale, re-verification required".to_string(),
                            ));
                        }
                        Ok(_) => { /* roots match — attestation is still valid */ }
                        Err(e) => {
                            // Log but do not block — network errors should
                            // not reject valid messages.
                            tracing::debug!(
                                originating_server = %envelope.originating_server,
                                "could not verify remote root for stale attestation check: {}", e
                            );
                        }
                    }
                }

                // 5. Verify Channel exists and is Federated
                let channel = annex_channels::get_channel(&conn, &envelope.channel_id)
                    .map_err(FederationError::Channel)?;
                let is_federated = matches!(
                    channel.federation_scope,
                    annex_types::FederationScope::Federated
                );
                if !is_federated {
                    return Err(FederationError::Forbidden(format!(
                        "Channel {} is not federated",
                        envelope.channel_id
                    )));
                }

                // 6. Verify Membership (Local Pseudonym)
                let is_member = annex_channels::is_member(
                    &conn,
                    state.server_id,
                    &envelope.channel_id,
                    &identity.pseudonym_id,
                )
                .map_err(FederationError::Channel)?;
                if !is_member {
                    return Err(FederationError::Forbidden(format!(
                        "User {} is not a member of channel {}",
                        identity.pseudonym_id, envelope.channel_id
                    )));
                }

                // 7. Insert Message (idempotent on UNIQUE message_id)
                let params = CreateMessageParams {
                    channel_id: envelope.channel_id.clone(),
                    message_id: envelope.message_id.clone(),
                    sender_pseudonym: identity.pseudonym_id.clone(),
                    content: envelope.content.clone(),
                    reply_to_message_id: None,
                };

                match create_message(&conn, &params) {
                    Ok(msg) => Ok(Some(msg)),
                    Err(annex_channels::ChannelError::Database(
                        rusqlite::Error::SqliteFailure(code, _),
                    )) if code.code == rusqlite::ErrorCode::ConstraintViolation => Ok(None),
                    Err(e) => Err(FederationError::Channel(e)),
                }
            }
        })
        .await
        .map_err(pool_err)??;

        // 8. Broadcast (idempotent skip when no row was inserted)
        if let Some(msg) = inserted {
            let out = OutgoingMessage::Message(msg.into());
            match serde_json::to_string(&out) {
                Ok(json) => {
                    state
                        .connection_manager
                        .broadcast(&channel_id_for_broadcast, json)
                        .await;
                }
                Err(e) => {
                    tracing::error!(
                        channel_id = %channel_id_for_broadcast,
                        "failed to serialize federated message for broadcast: {}", e
                    );
                }
            }
        }

        Ok(())
    }

    /// `POST /api/federation/rtx` orchestration. Returns
    /// `(bundle_id, delivered_count)`.
    #[allow(clippy::too_many_lines)]
    pub async fn receive_federated_rtx(
        &self,
        envelope: FederatedRtxEnvelope,
    ) -> Result<(String, usize), FederationError> {
        let state = self.state.clone();
        let bundle_id_for_response = envelope.bundle.bundle_id.clone();

        let (delivered_count, deliveries) = tokio::task::spawn_blocking({
            let state = state.clone();
            move || {
                let mut conn = state.pool.get().map_err(pool_err)?;

                // 1. Resolve relaying server instance
                let instance = repo::find_instance_by_base_url(&conn, &envelope.relaying_server)
                    .map_err(FederationError::DbError)?
                    .ok_or_else(|| {
                        FederationError::UnknownRemote(envelope.relaying_server.clone())
                    })?;

                if instance.status != "ACTIVE" {
                    return Err(FederationError::Forbidden(format!(
                        "Instance {} is not active",
                        envelope.relaying_server
                    )));
                }

                // 1.5. Circular relay prevention
                let local_url = state
                    .public_url
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                if !local_url.is_empty()
                    && envelope
                        .provenance
                        .relay_path
                        .iter()
                        .any(|hop| hop == &local_url)
                {
                    return Err(FederationError::Forbidden(
                        "circular relay detected: local server already in relay path".to_string(),
                    ));
                }

                // 1.6. Origin server validation
                if !repo::instance_known(&conn, &envelope.provenance.origin_server)
                    .map_err(FederationError::DbError)?
                {
                    return Err(FederationError::UnknownRemote(format!(
                        "origin server {} is not a known instance",
                        envelope.provenance.origin_server
                    )));
                }

                // 2. Verify active federation agreement and check transfer scope
                let transfer_scope_str =
                    repo::active_agreement_transfer_scope(&conn, instance.id)
                        .map_err(FederationError::DbError)?
                        .ok_or_else(|| {
                            FederationError::Forbidden(format!(
                                "No active federation agreement with {}",
                                envelope.relaying_server
                            ))
                        })?;

                let agreement_scope =
                    parse_transfer_scope(&transfer_scope_str).ok_or_else(|| {
                        FederationError::Forbidden(
                            "Federation agreement has invalid transfer scope".to_string(),
                        )
                    })?;

                if agreement_scope < VrpTransferScope::ReflectionSummariesOnly {
                    return Err(FederationError::Forbidden(
                        "Federation agreement does not permit RTX transfer".to_string(),
                    ));
                }

                // 3. Verify server signature on the envelope
                let signing_payload = rtx_relay_signing_payload(
                    &envelope.bundle.bundle_id,
                    &envelope.relaying_server,
                    &envelope.provenance.origin_server,
                    &envelope.provenance.relay_path,
                );
                verify_ed25519(
                    &instance.public_key_hex,
                    &envelope.signature,
                    signing_payload.as_bytes(),
                )?;

                // 4. Validate bundle structure
                validate_bundle_structure(&envelope.bundle).map_err(|e| {
                    FederationError::Forbidden(format!("Invalid bundle structure: {e}"))
                })?;

                // 4b. Enforce redacted topics from the federation agreement
                let redacted_topics = repo::active_agreement_redacted_topics(&conn, instance.id);
                if !redacted_topics.is_empty() {
                    check_redacted_topics(&envelope.bundle, &redacted_topics).map_err(|e| {
                        FederationError::Forbidden(format!("redacted topic violation: {e}"))
                    })?;
                }

                // 5. Enforce the local federation agreement's transfer scope
                //    on the bundle (may strip reasoning_chain when our
                //    agreement is ReflectionSummariesOnly).
                let scoped_bundle =
                    enforce_transfer_scope(&envelope.bundle, agreement_scope)
                        .map_err(|e| FederationError::Forbidden(e.to_string()))?;

                // 6. Begin transaction for all writes (bundle, transfer log, deliveries)
                let domain_tags_json = serde_json::to_string(&scoped_bundle.domain_tags)
                    .map_err(FederationError::Serialization)?;
                let caveats_json = serde_json::to_string(&scoped_bundle.caveats)
                    .map_err(FederationError::Serialization)?;
                let provenance_json = serde_json::to_string(&envelope.provenance)
                    .map_err(FederationError::Serialization)?;

                let tx = conn.transaction().map_err(FederationError::DbError)?;

                // Store bundle with provenance (idempotent on duplicate bundle_id).
                let inserted = repo::insert_rtx_bundle(
                    &tx,
                    state.server_id,
                    &scoped_bundle.bundle_id,
                    &scoped_bundle.source_pseudonym,
                    &scoped_bundle.source_server,
                    &domain_tags_json,
                    &scoped_bundle.summary,
                    scoped_bundle.reasoning_chain.as_deref(),
                    &caveats_json,
                    scoped_bundle.created_at as i64,
                    &scoped_bundle.signature,
                    &scoped_bundle.vrp_handshake_ref,
                    &provenance_json,
                )
                .map_err(FederationError::DbError)?;

                if !inserted {
                    // Duplicate bundle (idempotent) — already received.
                    // Transaction is dropped without commit (implicit
                    // rollback), which is correct because no writes
                    // succeeded.
                    return Ok((0_usize, Vec::<(String, String)>::new()));
                }

                // 7. Log the federated transfer (receive-side audit row).
                let redactions = if scoped_bundle.reasoning_chain.is_none()
                    && envelope.bundle.reasoning_chain.is_some()
                {
                    Some("reasoning_chain_stripped")
                } else {
                    None
                };

                repo::log_rtx_transfer(
                    &tx,
                    state.server_id,
                    &scoped_bundle.bundle_id,
                    &scoped_bundle.source_pseudonym,
                    None,
                    &agreement_scope.to_string(),
                    redactions,
                )
                .map_err(FederationError::DbError)?;

                // 8. Find matching local subscribers with accept_federated = 1.
                let mut deliveries: Vec<(String, String)> = Vec::new();
                let subscribers = repo::list_federated_rtx_subscribers(&tx, state.server_id)
                    .map_err(FederationError::DbError)?;

                for sub in subscribers {
                    // Parse domain filters. Corrupted JSON → skip this
                    // subscriber entirely (reject) rather than defaulting to
                    // accept-all, which could cause unauthorized knowledge
                    // transfer.
                    let domain_filters: Vec<String> =
                        match serde_json::from_str(&sub.domain_filters_json) {
                            Ok(f) => f,
                            Err(e) => {
                                tracing::error!(
                                    subscriber = %sub.pseudonym,
                                    bundle_id = %scoped_bundle.bundle_id,
                                    raw_json = %sub.domain_filters_json,
                                    "corrupted domain_filters_json in federated RTX delivery; skipping to protect against unauthorized transfer: {}",
                                    e
                                );
                                continue;
                            }
                        };

                    // Check domain tag match (empty filters = accept all)
                    let matches = domain_filters.is_empty()
                        || scoped_bundle
                            .domain_tags
                            .iter()
                            .any(|tag| domain_filters.contains(tag));
                    if !matches {
                        continue;
                    }

                    // Parse receiver's transfer scope
                    let receiver_scope = match parse_transfer_scope(&sub.transfer_scope_str) {
                        Some(s) if s >= VrpTransferScope::ReflectionSummariesOnly => s,
                        _ => {
                            tracing::warn!(
                                subscriber = %sub.pseudonym,
                                bundle_id = %scoped_bundle.bundle_id,
                                scope = %sub.transfer_scope_str,
                                "skipping federated RTX delivery: transfer scope is NoTransfer or unparseable"
                            );
                            continue;
                        }
                    };

                    // Apply receiver's transfer scope enforcement
                    let receiver_bundle = match enforce_transfer_scope(&scoped_bundle, receiver_scope) {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::error!(
                                subscriber = %sub.pseudonym,
                                bundle_id = %scoped_bundle.bundle_id,
                                scope = %receiver_scope.to_string(),
                                "federated transfer scope enforcement failed; skipping delivery: {}",
                                e
                            );
                            continue;
                        }
                    };

                    let payload = serde_json::json!({
                        "type": "rtx_bundle",
                        "bundle": receiver_bundle,
                        "federated": true,
                        "provenance": envelope.provenance,
                    });

                    match serde_json::to_string(&payload) {
                        Ok(json) => {
                            let delivery_redactions = if receiver_scope
                                == VrpTransferScope::ReflectionSummariesOnly
                                && scoped_bundle.reasoning_chain.is_some()
                            {
                                Some("reasoning_chain_stripped")
                            } else {
                                None
                            };

                            if let Err(e) = repo::log_rtx_transfer(
                                &tx,
                                state.server_id,
                                &scoped_bundle.bundle_id,
                                &scoped_bundle.source_pseudonym,
                                Some(&sub.pseudonym),
                                &receiver_scope.to_string(),
                                delivery_redactions,
                            ) {
                                tracing::warn!(
                                    bundle_id = %scoped_bundle.bundle_id,
                                    destination = %sub.pseudonym,
                                    "failed to write federated rtx transfer log: {}",
                                    e
                                );
                            }

                            deliveries.push((sub.pseudonym, json));
                        }
                        Err(e) => {
                            tracing::error!(
                                bundle_id = %scoped_bundle.bundle_id,
                                destination = %sub.pseudonym,
                                "failed to serialize federated rtx bundle for delivery: {}", e
                            );
                        }
                    }
                }

                let count = deliveries.len();
                tx.commit().map_err(FederationError::DbError)?;
                Ok::<(usize, Vec<(String, String)>), FederationError>((count, deliveries))
            }
        })
        .await
        .map_err(pool_err)??;

        // 9. Deliver via WebSocket (async, outside spawn_blocking)
        for (pseudonym, json) in &deliveries {
            state.connection_manager.send(pseudonym, json.clone()).await;
        }

        Ok((bundle_id_for_response, delivered_count))
    }
}

/// Background relay: post a freshly persisted local message to every
/// active federation peer's `/api/federation/messages` endpoint. Spawned
/// from `ws::commands::message::handle` for federated channels.
///
/// Behaviour preserved verbatim from the previous inline implementation:
///   * Skip peers whose `transfer_scope == "NO_TRANSFER"`.
///   * Use the canonical newline-delimited signing input from
///     [`message_signing_input`].
///   * Sign with the local server's Ed25519 signing key.
///   * Build envelopes with `originating_server = state.get_public_url()`
///     and `attestation_ref = "<topic>:<commitment>"` resolved via
///     `find_commitment_for_pseudonym`. Pseudonyms with no commitment
///     fall back to `"annex:server:v1:unknown"` (preserved string).
///   * Per-peer POST is fire-and-forget under `tokio::spawn`; non-success
///     responses and network errors are logged but not retried.
pub async fn relay_message(
    state: Arc<AppState>,
    channel_id: String,
    message: annex_channels::Message,
) {
    let peers_result = tokio::task::spawn_blocking({
        let state = state.clone();
        let sender = message.sender_pseudonym.clone();
        move || {
            let conn = state.pool.get().map_err(|e| e.to_string())?;

            // 1. Fetch active peers
            let peers = repo::list_active_peers(&conn, state.server_id)
                .map_err(|e| e.to_string())?;

            // 2. Resolve commitment + topic for sender (with legacy fallback);
            //    on failure fall through to the "unknown" attestation ref.
            let mut attestation_ref = "annex:server:v1:unknown".to_string();
            match repo::find_commitment_for_pseudonym(&conn, &sender) {
                Ok(Some((commitment, topic))) => {
                    attestation_ref = format!("{topic}:{commitment}");
                }
                Ok(None) => {
                    tracing::debug!(sender = %sender, "no commitment found for pseudonym, using unknown attestation ref");
                }
                Err(e) => {
                    tracing::warn!(sender = %sender, "failed to look up commitment for pseudonym: {}", e);
                }
            }

            Ok::<_, String>((peers, attestation_ref))
        }
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));

    let (peers, attestation_ref) = match peers_result {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Failed to fetch federation peers: {}", e);
            return;
        }
    };

    if peers.is_empty() {
        return;
    }

    // Construct envelope with the canonical signing input.
    let pub_url = state.get_public_url();
    let envelope_for_signing = FederatedMessageEnvelope {
        message_id: message.message_id.clone(),
        channel_id: channel_id.clone(),
        content: message.content.clone(),
        sender_pseudonym: message.sender_pseudonym.clone(),
        originating_server: pub_url.clone(),
        attestation_ref: attestation_ref.clone(),
        signature: String::new(), // placeholder for signing input only
        created_at: message.created_at.clone(),
    };
    let signature = state
        .signing_key
        .sign(message_signing_input(&envelope_for_signing).as_bytes());
    let signature_hex = hex::encode(signature.to_bytes());

    let envelope = FederatedMessageEnvelope {
        signature: signature_hex,
        ..envelope_for_signing
    };

    let client = match federation_http_client() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to build federation HTTP client: {}", e);
            return;
        }
    };

    for peer in peers {
        // Skip peers whose transfer scope does not permit message relay.
        if peer.transfer_scope == "NO_TRANSFER" {
            tracing::debug!(
                peer = %peer.base_url,
                "skipping message relay: transfer scope is NO_TRANSFER"
            );
            continue;
        }

        let url = format!("{}/api/federation/messages", peer.base_url);
        let envelope_clone = FederatedMessageEnvelope {
            message_id: envelope.message_id.clone(),
            channel_id: envelope.channel_id.clone(),
            content: envelope.content.clone(),
            sender_pseudonym: envelope.sender_pseudonym.clone(),
            originating_server: envelope.originating_server.clone(),
            attestation_ref: envelope.attestation_ref.clone(),
            signature: envelope.signature.clone(),
            created_at: envelope.created_at.clone(),
        };

        let client_clone = client.clone();
        tokio::spawn(async move {
            match client_clone.post(&url).json(&envelope_clone).send().await {
                Ok(resp) if !resp.status().is_success() => {
                    tracing::warn!(
                        peer = %url,
                        status = %resp.status(),
                        "federation message relay received non-success response"
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(peer = %url, "failed to relay message: {}", e);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::parse_attestation_ref;

    #[test]
    fn parse_attestation_ref_simple() {
        let (commitment, topic) =
            parse_attestation_ref("general:abc123").expect("should parse successfully");
        assert_eq!(commitment, "abc123");
        assert_eq!(topic, "general");
    }

    #[test]
    fn parse_attestation_ref_multi_colon_topic() {
        let (commitment, topic) =
            parse_attestation_ref("annex:server:v1:deadbeef").expect("should parse successfully");
        assert_eq!(commitment, "deadbeef");
        assert_eq!(topic, "annex:server:v1");
    }

    #[test]
    fn parse_attestation_ref_empty_string() {
        let result = parse_attestation_ref("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_attestation_ref_no_colon() {
        let result = parse_attestation_ref("nodelimiter");
        assert!(result.is_err());
    }

    #[test]
    fn parse_attestation_ref_empty_topic() {
        let result = parse_attestation_ref(":abc123");
        assert!(result.is_err());
    }

    #[test]
    fn parse_attestation_ref_empty_commitment() {
        let result = parse_attestation_ref("topic:");
        assert!(result.is_err());
    }

    #[test]
    fn parse_attestation_ref_only_colon() {
        let result = parse_attestation_ref(":");
        assert!(result.is_err());
    }

    #[test]
    fn parse_attestation_ref_fallback_format() {
        // The relay_message function generates "annex:server:v1:unknown" as fallback
        let (commitment, topic) =
            parse_attestation_ref("annex:server:v1:unknown").expect("fallback format should parse");
        assert_eq!(commitment, "unknown");
        assert_eq!(topic, "annex:server:v1");
    }

    #[test]
    fn verify_ed25519_rejects_garbage_signature() {
        // Smoke test: invalid hex must not pass the gate.
        let result = super::verify_ed25519("deadbeef", "not-hex-at-all-zzz", b"any payload");
        assert!(matches!(
            result,
            Err(super::FederationError::InvalidSignature(_))
        ));
    }

    #[test]
    fn verify_ed25519_rejects_signature_for_different_payload() {
        // Generate a signing key, sign one payload, ask the verifier
        // about a different payload. Must reject.
        use ed25519_dalek::{Signer, SigningKey};
        use rand::rngs::OsRng;

        let signing_key = SigningKey::generate(&mut OsRng);
        let public_key_hex = hex::encode(signing_key.verifying_key().to_bytes());
        let signature = signing_key.sign(b"signed payload");
        let signature_hex = hex::encode(signature.to_bytes());

        // Sanity: correct payload verifies.
        let ok = super::verify_ed25519(&public_key_hex, &signature_hex, b"signed payload");
        assert!(ok.is_ok());

        // Tampered payload must fail.
        let bad = super::verify_ed25519(&public_key_hex, &signature_hex, b"different payload");
        assert!(matches!(
            bad,
            Err(super::FederationError::InvalidSignature(_))
        ));
    }
}
