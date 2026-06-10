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
    process_incoming_handshake, AttestationRequest, FederatedMessageEnvelope,
    FederatedRedactionEnvelope, FederatedRtxEnvelope, HandshakeError,
};
use annex_graph::{ensure_graph_node, GraphError};
use annex_identity::{
    derive_nullifier_hex, derive_pseudonym_id,
    zk::{
        fr_to_canonical_hex, parse_fr_from_hex, parse_proof, parse_public_signals,
        topic_hash_for_v2, verify_proof, Bn254, VerifyingKey,
    },
};
use annex_observe::EventPayload;
use annex_rtx::{check_redacted_topics, enforce_transfer_scope, validate_bundle_structure};
use annex_types::NodeType;
use annex_vrp::{VrpTransferScope, VrpValidationReport};
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey as EdVerifyingKey};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior};
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

/// Maximum content length accepted from a federated message envelope.
///
/// Mirrors the local WebSocket ceiling
/// (`crate::ws::dispatch::MAX_WS_MESSAGE_CONTENT_LEN`, 64 KiB). A federated
/// peer that bypasses the local WS path could otherwise push messages up to
/// axum's 2 MiB request-body limit into the `messages` table — beyond what
/// any local client can produce. Bound it before persisting.
pub(crate) const FEDERATION_MAX_MESSAGE_CONTENT_LEN: usize = 65_536;

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
///
/// Version dispatch:
///   * `envelope.envelope_version == None | Some("v1")` — legacy
///     7-line input. Preserves wire-compatibility with peers that
///     have not adopted v2.
///   * `envelope.envelope_version == Some("v2")` — 8-line input,
///     prepended with the literal version string so v1 ↔ v2
///     downgrade / upgrade attacks change the signed bytes and
///     therefore break the signature.
///
/// Newline delimiters prevent field-boundary ambiguity (e.g.
/// `message_id="ab" + channel_id="c"` would collide with `"a" + "bc"`
/// without delimiters). Used by both the relay and receive paths.
pub fn message_signing_input(envelope: &FederatedMessageEnvelope) -> String {
    let version = envelope
        .envelope_version
        .as_deref()
        .unwrap_or(annex_federation::FEDERATED_MESSAGE_ENVELOPE_V1);
    if version == annex_federation::FEDERATED_MESSAGE_ENVELOPE_V2 {
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            annex_federation::FEDERATED_MESSAGE_ENVELOPE_V2,
            envelope.message_id,
            envelope.channel_id,
            envelope.content,
            envelope.sender_pseudonym,
            envelope.originating_server,
            envelope.attestation_ref,
            envelope.created_at
        )
    } else {
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
}

/// Canonical SHA-256 of the signing input. Used by the federation
/// receipt ledger to detect "same message_id, different signed body"
/// attacks (key compromise scenarios).
pub fn message_envelope_hash(envelope: &FederatedMessageEnvelope) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(message_signing_input(envelope).as_bytes());
    hex::encode(hasher.finalize())
}

/// Canonical signing input for a federated redaction envelope
/// (ADR-0011 tombstone protocol).
///
/// The first line is the [`annex_federation::REDACTION_SIGNING_DOMAIN_V1`]
/// literal — a domain-separation prefix distinct from both message
/// signing-input shapes, so a redaction signature can never verify as a
/// message envelope (or vice versa) regardless of field contents.
/// Newline delimiters prevent field-boundary ambiguity, exactly as in
/// [`message_signing_input`]. The `signature` field itself is not part
/// of the input.
pub fn redaction_signing_input(envelope: &FederatedRedactionEnvelope) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        annex_federation::REDACTION_SIGNING_DOMAIN_V1,
        envelope.message_id,
        envelope.channel_id,
        envelope.originating_server,
        envelope.redacted_by,
        envelope.redaction_reason,
        envelope.attestation_ref,
        envelope.created_at
    )
}

/// Canonical SHA-256 of the redaction signing input. Stored in the
/// receipt ledger (under the `redaction:`-prefixed message_id) to make
/// re-delivery idempotent and to detect a different signed body being
/// replayed under a captured redaction id.
pub fn redaction_envelope_hash(envelope: &FederatedRedactionEnvelope) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(redaction_signing_input(envelope).as_bytes());
    hex::encode(hasher.finalize())
}

/// Receipt-ledger key prefix for redaction envelopes.
///
/// Redactions share `federation_message_receipts` (and the federation
/// outbox) with message envelopes. Both tables key on
/// `(instance, message_id)`, and a redaction necessarily reuses the
/// original message's id — so redaction rows are namespaced with this
/// prefix to avoid colliding with the original message's receipt /
/// outbox row. The prefix contains a `:` and message ids are UUIDs, so
/// no legitimate message id can collide with a prefixed key.
pub const REDACTION_LEDGER_PREFIX: &str = "redaction:";

/// Errors specific to the freshness gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessRejection {
    /// `created_at` could not be parsed as ISO 8601 / RFC 3339.
    Unparseable,
    /// `created_at` is too far in the past for live delivery.
    TooOld,
    /// `created_at` is far enough in the future to indicate clock
    /// skew or deliberate forward-dating.
    TooFarInFuture,
}

/// Federation envelope delivery mode. Live envelopes go through the
/// strict freshness gate; catch-up envelopes are allowed to be older
/// than the live window but still must be signature- and replay-valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    Live,
    Catchup,
}

impl DeliveryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            DeliveryMode::Live => "live",
            DeliveryMode::Catchup => "catchup",
        }
    }
}

/// Decides whether an envelope's `created_at` is acceptable for the
/// given delivery mode. The `now` parameter is injectable so tests
/// can pin the clock; the production caller uses `chrono::Utc::now()`.
pub fn check_freshness(
    created_at: &str,
    now: chrono::DateTime<chrono::Utc>,
    freshness_window_seconds: i64,
    future_skew_seconds: i64,
    mode: DeliveryMode,
) -> Result<(), FreshnessRejection> {
    let parsed = chrono::DateTime::parse_from_rfc3339(created_at)
        .map_err(|_| FreshnessRejection::Unparseable)?
        .with_timezone(&chrono::Utc);
    let age = now.signed_duration_since(parsed).num_seconds();
    if age < -future_skew_seconds {
        return Err(FreshnessRejection::TooFarInFuture);
    }
    if mode == DeliveryMode::Live && age > freshness_window_seconds {
        return Err(FreshnessRejection::TooOld);
    }
    Ok(())
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
                &state.signing_key,
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
    ///
    /// Dispatches to the v1 or v2 verifier based on the peer's
    /// `protocol_version`. v1 (legacy / default) verifies a 2-signal proof
    /// against `state.membership_vkey` and derives the nullifier as
    /// `Poseidon(commitment, topic)`. v2 verifies a 4-signal proof against
    /// `state.membership_vkey_v2`, cross-checks `publicSignals[3]` against
    /// the server-recomputed `topic_hash_for_v2(payload.topic)`, and uses
    /// the secret-derived nullifier from `publicSignals[2]`. v2 attestations
    /// are rejected with `409 Conflict` when the receiving server has not
    /// loaded the v2 vkey (i.e., `"v2"` is not in
    /// `Config::security.enabled_zk_versions`).
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

        // Resolve protocol version up-front so the wire-format check fires
        // before any DB or network I/O.
        let protocol_version = payload.protocol_version.as_deref().unwrap_or("v1");
        let vkey_for_proof: Arc<VerifyingKey<Bn254>> = match protocol_version {
            "v1" => self.state.membership_vkey.clone(),
            "v2" => self.state.membership_vkey_v2.clone().ok_or_else(|| {
                FederationError::Forbidden(
                    "membership v2 is not enabled on this server (security.enabled_zk_versions \
                     does not include \"v2\")"
                        .to_string(),
                )
            })?,
            other => {
                return Err(FederationError::ZkVerification(format!(
                    "unsupported protocol_version '{other}' (expected \"v1\" or \"v2\")"
                )));
            }
        };

        // v2 input-shape validation runs BEFORE the network round-trip so
        // a malformed v2 envelope is rejected deterministically without
        // probing the peer. The cross-check against `remote_root` happens
        // later, after we fetch the peer's current root.
        struct V2Inputs {
            public_signals: Vec<annex_identity::zk::Fr>,
            canonical_nullifier_hex: String,
            expected_topic_hash: annex_identity::zk::Fr,
        }
        let v2_inputs: Option<V2Inputs> = if protocol_version == "v2" {
            let raw_public_signals = payload.public_signals.as_ref().ok_or_else(|| {
                FederationError::ZkVerification(
                    "v2 attestation must include publicSignals".to_string(),
                )
            })?;
            if raw_public_signals.len() != 4 {
                return Err(FederationError::ZkVerification(format!(
                    "v2 attestation publicSignals must have length 4, got {}",
                    raw_public_signals.len()
                )));
            }
            let public_signals_json = serde_json::to_string(raw_public_signals)?;
            let public_signals = parse_public_signals(&public_signals_json).map_err(|e| {
                FederationError::ZkVerification(format!("invalid publicSignals format: {e}"))
            })?;

            // Topic-binding: same rule as `verify-membership` — the proof
            // is bound to whatever the prover put in `topicHash`, so the
            // server MUST require it to equal the canonical hash of
            // `payload.topic`. Without this a malicious prover could
            // reuse a v2 proof for topic A as a v2 attestation for
            // topic B.
            let expected_topic_hash = topic_hash_for_v2(&payload.topic).map_err(|e| {
                FederationError::ZkVerification(format!(
                    "failed to derive topicHash for v2 attestation: {e}"
                ))
            })?;
            if public_signals[3] != expected_topic_hash {
                return Err(FederationError::ZkVerification(
                    "v2 publicSignals[3] (topicHash) does not match the canonical hash of \
                     payload.topic — the proof is bound to a different topic"
                        .to_string(),
                ));
            }

            // Cross-check claimed scalars against the proof's public
            // signals so a single field mismatch surfaces as a 400 instead
            // of being routed into the verifier as a tampered input.
            let claimed_nullifier_hex = payload.nullifier_hex.as_deref().ok_or_else(|| {
                FederationError::ZkVerification(
                    "v2 attestation must include nullifierHex".to_string(),
                )
            })?;
            let canonical_nullifier_hex = fr_to_canonical_hex(public_signals[2]);
            if claimed_nullifier_hex.to_ascii_lowercase() != canonical_nullifier_hex {
                return Err(FederationError::ZkVerification(
                    "v2 nullifierHex does not match publicSignals[2]".to_string(),
                ));
            }

            if let Some(claimed_topic_hash_hex) = payload.topic_hash_hex.as_deref() {
                let claimed_topic_hash_fr =
                    parse_fr_from_hex(claimed_topic_hash_hex).map_err(|e| {
                        FederationError::ZkVerification(format!("invalid topicHashHex: {e}"))
                    })?;
                if claimed_topic_hash_fr != expected_topic_hash {
                    return Err(FederationError::ZkVerification(
                        "v2 topicHashHex does not match canonical hash of payload.topic"
                            .to_string(),
                    ));
                }
            } else {
                return Err(FederationError::ZkVerification(
                    "v2 attestation must include topicHashHex".to_string(),
                ));
            }

            Some(V2Inputs {
                public_signals,
                canonical_nullifier_hex,
                expected_topic_hash,
            })
        } else {
            None
        };

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

        // Verify Signature. The signing input includes the protocol version
        // and the v2-specific scalars when v2 is declared, so a peer cannot
        // tamper with the version field on the wire — flipping v2 → v1 (or
        // stripping the field entirely) breaks the signature.
        let signing_message = if protocol_version == "v2" {
            let nullifier_hex_for_signing = payload.nullifier_hex.as_deref().ok_or_else(|| {
                FederationError::ZkVerification(
                    "v2 attestation must include nullifierHex".to_string(),
                )
            })?;
            let topic_hash_hex_for_signing =
                payload.topic_hash_hex.as_deref().ok_or_else(|| {
                    FederationError::ZkVerification(
                        "v2 attestation must include topicHashHex".to_string(),
                    )
                })?;
            format!(
                "{}\n{}\n{}\n{}\n{}\n{}",
                payload.topic,
                payload.commitment,
                payload.participant_type,
                "v2",
                nullifier_hex_for_signing,
                topic_hash_hex_for_signing,
            )
        } else {
            format!(
                "{}\n{}\n{}",
                payload.topic, payload.commitment, payload.participant_type
            )
        };
        verify_ed25519(
            &public_key_hex,
            &payload.signature,
            signing_message.as_bytes(),
        )?;

        // 2. Fetch Remote Root (with timeout and redirect protection against SSRF)
        // Defence-in-depth: even though the originating_server matches a
        // known instance row (administrator-controlled), block private /
        // loopback / link-local hosts so a misconfigured peer entry cannot
        // turn this endpoint into an SSRF probe of internal services.
        if crate::api_link_preview::is_url_private_or_reserved(&payload.originating_server) {
            return Err(FederationError::Forbidden(format!(
                "originating_server {} resolves to a private or reserved address",
                payload.originating_server
            )));
        }
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

        // Finalise the public-inputs vector and pick the canonical
        // nullifier we will store in `federated_identities`. For v1 the
        // server recomputes the nullifier from `Poseidon(commitment,
        // topic)`. For v2 the nullifier is secret-derived inside the
        // circuit and carried in `public_signals[2]`; the server only
        // cross-checks it against the peer's claim. The
        // `publicSignals[0]` (root) check requires the network round-trip
        // result, so it fires here.
        let (public_inputs, nullifier_hex) = if let Some(v2) = v2_inputs {
            if v2.public_signals[0] != remote_root_fr {
                return Err(FederationError::ZkVerification(
                    "v2 publicSignals[0] does not match remote root_hex".to_string(),
                ));
            }
            if v2.public_signals[1] != commitment_fr {
                return Err(FederationError::ZkVerification(
                    "v2 publicSignals[1] does not match payload.commitment".to_string(),
                ));
            }
            // expected_topic_hash already validated against publicSignals[3] above.
            let _ = v2.expected_topic_hash;
            (v2.public_signals, v2.canonical_nullifier_hex)
        } else {
            let v1_inputs = vec![remote_root_fr, commitment_fr];
            let v1_nullifier =
                derive_nullifier_hex(&payload.commitment, &payload.topic).map_err(|e| {
                    FederationError::IdentityDerivation(format!("Failed to derive nullifier: {e}"))
                })?;
            (v1_inputs, v1_nullifier)
        };

        let valid = verify_proof(&vkey_for_proof, &proof, &public_inputs).map_err(|e| {
            FederationError::ZkVerification(format!("Proof verification error: {e}"))
        })?;

        if !valid {
            return Err(FederationError::ZkVerification("Invalid proof".to_string()));
        }

        // 4. Persist Attestation (federated_identities + platform_identities + graph node)
        //    in a single transaction.
        tokio::task::spawn_blocking(move || {
            let mut conn = state.pool.get().map_err(pool_err)?;

            // Derive local identifiers from the version-correct nullifier.
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
            if !repo::has_active_agreement(&conn, state.server_id, instance.id)
                .map_err(FederationError::DbError)?
            {
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
        // Enforce the same per-message content cap that local WS messages
        // honour. Without this, a federated peer (signed envelopes still
        // pass signature verification) could push messages up to axum's
        // 2 MiB body cap into our `messages` table, well beyond what local
        // clients are allowed to send. Bound it before any DB I/O.
        if envelope.content.len() > FEDERATION_MAX_MESSAGE_CONTENT_LEN {
            return Err(FederationError::Forbidden(format!(
                "Federated message content exceeds maximum length of {FEDERATION_MAX_MESSAGE_CONTENT_LEN} bytes"
            )));
        }

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

                // 1.4. Freshness gate (v2 envelopes only — v1 peers
                //      never advertised support for this and would be
                //      surprised by sudden rejection). v2 envelopes
                //      delivered live must be within the configured
                //      freshness window and not far-future-skewed.
                let envelope_version = envelope
                    .envelope_version
                    .as_deref()
                    .unwrap_or(annex_federation::FEDERATED_MESSAGE_ENVELOPE_V1);
                if envelope_version == annex_federation::FEDERATED_MESSAGE_ENVELOPE_V2 {
                    let fed_cfg = &state.federation_config;
                    match check_freshness(
                        &envelope.created_at,
                        chrono::Utc::now(),
                        fed_cfg.freshness_window_seconds,
                        fed_cfg.future_skew_seconds,
                        DeliveryMode::Live,
                    ) {
                        Ok(()) => {}
                        Err(FreshnessRejection::Unparseable) => {
                            return Err(FederationError::Forbidden(
                                "envelope.created_at is not RFC 3339 / ISO 8601".to_string(),
                            ));
                        }
                        Err(FreshnessRejection::TooOld) => {
                            return Err(FederationError::Forbidden(format!(
                                "envelope created_at {} is older than the live freshness window ({}s) — use the catch-up endpoint",
                                envelope.created_at, fed_cfg.freshness_window_seconds
                            )));
                        }
                        Err(FreshnessRejection::TooFarInFuture) => {
                            return Err(FederationError::Forbidden(format!(
                                "envelope created_at {} is more than {}s in the future",
                                envelope.created_at, fed_cfg.future_skew_seconds
                            )));
                        }
                    }
                }

                // 1.5. Verify Active Federation Agreement
                if !repo::has_active_agreement(&conn, state.server_id, instance.id)
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
                //
                // SSRF defence-in-depth: skip the freshness callback if the
                // peer's base_url resolves to a private/loopback/link-local
                // host. Peers are administratively trusted, but a misconfigured
                // peer entry (e.g. `http://localhost:9090`) would otherwise
                // turn this code path into an outbound probe of internal
                // services on every received message. We log the skip rather
                // than rejecting the message, because the freshness check is
                // a soft "log on mismatch / continue on network error" gate,
                // not a hard authorization step.
                if !identity.root_hex_at_verification.is_empty()
                    && !crate::api_link_preview::is_url_private_or_reserved(
                        &envelope.originating_server,
                    )
                {
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

                // 6.5. Federation receipt ledger + message insert.
                //
                //   * If we have NOT seen (remote_instance_id, message_id)
                //     before, insert a receipt row and let the message
                //     INSERT proceed.
                //   * If we HAVE seen it and the envelope hash matches,
                //     this is a benign replay (e.g. outbox retry against
                //     a peer that ack'd late). Skip the insert+broadcast
                //     so the receiver-side flow is idempotent.
                //   * If we HAVE seen it and the envelope hash DIFFERS,
                //     someone is presenting a forged or mutated
                //     envelope under a captured message_id. Reject.
                //
                // Receipt insert + message insert MUST commit atomically.
                // Prior to this transaction, the receipt INSERT ran under
                // autocommit, then `create_message` ran as a separate
                // implicit transaction. If `create_message` failed for a
                // transient reason (SQLITE_BUSY, disk full, FK violation
                // because the channel was deleted between membership check
                // and message insert), the receipt was still committed —
                // the next outbox retry from the peer would find the
                // receipt with a matching hash, return Ok(None), and the
                // message would be SILENTLY DROPPED forever. Wrapping
                // both inserts in IMMEDIATE forces the SQLite writer lock
                // up front and ensures either both rows land or neither
                // does, so the peer's retry has a chance to succeed.
                let env_hash = message_envelope_hash(&envelope);
                let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)
                    .map_err(FederationError::DbError)?;

                let receipt_existing: Option<String> = tx
                    .query_row(
                        "SELECT envelope_hash FROM federation_message_receipts \
                         WHERE remote_instance_id = ?1 AND message_id = ?2",
                        rusqlite::params![instance.id, &envelope.message_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(FederationError::DbError)?;

                if let Some(prior_hash) = receipt_existing {
                    if prior_hash != env_hash {
                        tracing::warn!(
                            remote_instance_id = instance.id,
                            message_id = %envelope.message_id,
                            prior_hash = %prior_hash,
                            new_hash = %env_hash,
                            "federation receipt mismatch: same message_id, different envelope hash"
                        );
                        return Err(FederationError::Forbidden(
                            "envelope hash does not match prior receipt for this message_id"
                                .to_string(),
                        ));
                    }
                    // Benign duplicate — no insert, no broadcast. The tx
                    // drops without commit, releasing the writer lock.
                    return Ok(None);
                }

                tx.execute(
                    "INSERT INTO federation_message_receipts \
                     (remote_instance_id, message_id, envelope_hash, envelope_created_at, delivery_mode) \
                     VALUES (?1, ?2, ?3, ?4, 'live')",
                    rusqlite::params![
                        instance.id,
                        &envelope.message_id,
                        &env_hash,
                        &envelope.created_at,
                    ],
                )
                .map_err(FederationError::DbError)?;

                // 7. Insert Message (idempotent on UNIQUE message_id).
                // The UNIQUE constraint on messages.message_id is the
                // legacy idempotency path; under the receipt-ledger
                // changes above it should be unreachable in practice
                // (the receipt check rejects duplicates earlier), but
                // we keep the constraint-violation arm for defence in
                // depth against direct DB writes or schema oddities.
                let params = CreateMessageParams {
                    channel_id: envelope.channel_id.clone(),
                    message_id: envelope.message_id.clone(),
                    sender_pseudonym: identity.pseudonym_id.clone(),
                    content: envelope.content.clone(),
                    reply_to_message_id: None,
                };

                let inserted = match create_message(&tx, &params) {
                    Ok(msg) => Some(msg),
                    Err(annex_channels::ChannelError::Database(
                        rusqlite::Error::SqliteFailure(code, _),
                    )) if code.code == rusqlite::ErrorCode::ConstraintViolation => None,
                    Err(e) => return Err(FederationError::Channel(e)),
                };

                tx.commit().map_err(FederationError::DbError)?;
                Ok(inserted)
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

    /// `POST /api/federation/redactions` orchestration (ADR-0011
    /// tombstone protocol).
    ///
    /// Verification chain, mirroring [`Self::receive_federated_message`]
    /// where the concerns overlap:
    ///
    /// 1. Envelope shape: kind/version/reason validated against the
    ///    constants in `annex_federation`.
    /// 2. Originating instance is known + ACTIVE, with an active
    ///    federation agreement.
    /// 3. Freshness gate — always enforced (the redaction protocol is
    ///    new, so there are no legacy peers to grandfather).
    /// 4. Ed25519 signature over [`redaction_signing_input`] verifies
    ///    against the originating server's published key.
    /// 5. **Origin authority**: a receipt for the ORIGINAL message must
    ///    exist from the SAME peer instance. Only the server that
    ///    delivered a message may redact it — a peer can never redact
    ///    locally-authored messages or messages delivered by a
    ///    different peer.
    /// 6. **Redactor authority**: for `reason != "moderation"`,
    ///    `redacted_by` must equal the stored row's `sender_pseudonym`.
    ///    `moderation` redactions are accepted on the originating
    ///    server's signature alone — the channel lives on that server
    ///    and its moderators govern it (sovereignty model).
    /// 7. Idempotency: a redaction receipt (keyed
    ///    `redaction:<message_id>`) with a matching envelope hash is a
    ///    benign replay; a hash mismatch is rejected.
    ///
    /// Effect: `content` is blanked and `deleted_at` set on the local
    /// row; `message_id`, `created_at`, and `sender_pseudonym` are kept
    /// for audit (same shape as a local soft delete). The receipt and
    /// the UPDATE commit atomically (IMMEDIATE transaction — same
    /// rationale as the [F32] fix on the message path).
    ///
    /// Returns the channel id when the redaction was applied, `None`
    /// for benign replays and for messages already hard-deleted by
    /// retention.
    #[allow(clippy::too_many_lines)]
    pub async fn receive_federated_redaction(
        &self,
        envelope: FederatedRedactionEnvelope,
    ) -> Result<Option<String>, FederationError> {
        // 1. Envelope shape — cheap rejects before any DB I/O.
        if envelope.envelope_kind != annex_federation::FEDERATED_ENVELOPE_KIND_REDACTION {
            return Err(FederationError::Forbidden(format!(
                "unexpected envelopeKind '{}' (expected '{}')",
                envelope.envelope_kind,
                annex_federation::FEDERATED_ENVELOPE_KIND_REDACTION
            )));
        }
        if envelope.envelope_version != annex_federation::FEDERATED_REDACTION_ENVELOPE_V1 {
            return Err(FederationError::Forbidden(format!(
                "unsupported redaction envelope version '{}'",
                envelope.envelope_version
            )));
        }
        if !annex_federation::REDACTION_REASONS.contains(&envelope.redaction_reason.as_str()) {
            return Err(FederationError::Forbidden(format!(
                "invalid redaction_reason '{}' (expected one of: {})",
                envelope.redaction_reason,
                annex_federation::REDACTION_REASONS.join(", ")
            )));
        }

        let state = self.state.clone();
        let applied_channel = tokio::task::spawn_blocking({
            let state = state.clone();
            move || {
                let conn = state.pool.get().map_err(pool_err)?;

                // 2. Resolve + gate the originating instance.
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
                if !repo::has_active_agreement(&conn, state.server_id, instance.id)
                    .map_err(FederationError::DbError)?
                {
                    return Err(FederationError::Forbidden(format!(
                        "No active federation agreement with {}",
                        envelope.originating_server
                    )));
                }

                // 3. Freshness gate.
                let fed_cfg = &state.federation_config;
                match check_freshness(
                    &envelope.created_at,
                    chrono::Utc::now(),
                    fed_cfg.freshness_window_seconds,
                    fed_cfg.future_skew_seconds,
                    DeliveryMode::Live,
                ) {
                    Ok(()) => {}
                    Err(FreshnessRejection::Unparseable) => {
                        return Err(FederationError::Forbidden(
                            "envelope.created_at is not RFC 3339 / ISO 8601".to_string(),
                        ));
                    }
                    Err(FreshnessRejection::TooOld) => {
                        return Err(FederationError::Forbidden(format!(
                            "redaction created_at {} is older than the live freshness window ({}s)",
                            envelope.created_at, fed_cfg.freshness_window_seconds
                        )));
                    }
                    Err(FreshnessRejection::TooFarInFuture) => {
                        return Err(FederationError::Forbidden(format!(
                            "redaction created_at {} is more than {}s in the future",
                            envelope.created_at, fed_cfg.future_skew_seconds
                        )));
                    }
                }

                // 4. Signature.
                verify_ed25519(
                    &instance.public_key_hex,
                    &envelope.signature,
                    redaction_signing_input(&envelope).as_bytes(),
                )?;

                // 5. Origin authority: we must hold a receipt for the
                //    ORIGINAL message from this same peer.
                let original_receipt: Option<String> = conn
                    .query_row(
                        "SELECT envelope_hash FROM federation_message_receipts \
                         WHERE remote_instance_id = ?1 AND message_id = ?2",
                        rusqlite::params![instance.id, &envelope.message_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(FederationError::DbError)?;
                if original_receipt.is_none() {
                    return Err(FederationError::Forbidden(format!(
                        "no receipt for message {} from {} — only the delivering peer may redact",
                        envelope.message_id, envelope.originating_server
                    )));
                }

                // 6. Local row + redactor authority. A missing row means
                //    retention already hard-deleted it — record the
                //    receipt (idempotency) and report "nothing to do".
                let local_row: Option<(String, String, Option<String>)> = conn
                    .query_row(
                        "SELECT channel_id, sender_pseudonym, deleted_at \
                         FROM messages WHERE message_id = ?1",
                        rusqlite::params![&envelope.message_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()
                    .map_err(FederationError::DbError)?;

                if let Some((_, ref sender, _)) = local_row {
                    if envelope.redaction_reason != "moderation" && &envelope.redacted_by != sender
                    {
                        return Err(FederationError::Forbidden(format!(
                            "redactor {} is not the sender of message {}",
                            envelope.redacted_by, envelope.message_id
                        )));
                    }
                }

                // 7. Receipt + UPDATE, atomically.
                let env_hash = redaction_envelope_hash(&envelope);
                let ledger_key = format!("{REDACTION_LEDGER_PREFIX}{}", envelope.message_id);
                let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)
                    .map_err(FederationError::DbError)?;

                let prior: Option<String> = tx
                    .query_row(
                        "SELECT envelope_hash FROM federation_message_receipts \
                         WHERE remote_instance_id = ?1 AND message_id = ?2",
                        rusqlite::params![instance.id, &ledger_key],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(FederationError::DbError)?;
                if let Some(prior_hash) = prior {
                    if prior_hash != env_hash {
                        tracing::warn!(
                            remote_instance_id = instance.id,
                            message_id = %envelope.message_id,
                            "redaction receipt mismatch: same id, different signed body"
                        );
                        return Err(FederationError::Forbidden(
                            "envelope hash does not match prior receipt for this redaction"
                                .to_string(),
                        ));
                    }
                    // Benign replay. Drop the tx without commit.
                    return Ok(None);
                }

                tx.execute(
                    "INSERT INTO federation_message_receipts \
                     (remote_instance_id, message_id, envelope_hash, envelope_created_at, delivery_mode) \
                     VALUES (?1, ?2, ?3, ?4, 'live')",
                    rusqlite::params![instance.id, &ledger_key, &env_hash, &envelope.created_at],
                )
                .map_err(FederationError::DbError)?;

                let applied = if let Some((channel_id, _, deleted_at)) = local_row {
                    if deleted_at.is_none() {
                        tx.execute(
                            "UPDATE messages SET content = '', deleted_at = datetime('now') \
                             WHERE message_id = ?1",
                            rusqlite::params![&envelope.message_id],
                        )
                        .map_err(FederationError::DbError)?;
                    }
                    // Re-read the (now blanked) row so the broadcast
                    // payload matches what the local-delete flow sends.
                    let updated = annex_channels::get_message(&tx, &envelope.message_id)
                        .map_err(FederationError::Channel)?;
                    Some((channel_id, updated))
                } else {
                    tracing::debug!(
                        message_id = %envelope.message_id,
                        "federated redaction for a message already removed by retention"
                    );
                    None
                };

                tx.commit().map_err(FederationError::DbError)?;
                Ok(applied)
            }
        })
        .await
        .map_err(pool_err)??;

        // Broadcast the deletion to local subscribers so open clients
        // blank the message in place, mirroring the local-delete flow
        // (`OutgoingMessage::MessageDeleted`).
        if let Some((channel_id, updated)) = applied_channel {
            let out = OutgoingMessage::MessageDeleted(updated.into());
            match serde_json::to_string(&out) {
                Ok(json) => {
                    state.connection_manager.broadcast(&channel_id, json).await;
                }
                Err(e) => {
                    tracing::error!(
                        channel_id = %channel_id,
                        "failed to serialize federated redaction broadcast: {}", e
                    );
                }
            }
            return Ok(Some(channel_id));
        }

        Ok(None)
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
                    repo::active_agreement_transfer_scope(&conn, state.server_id, instance.id)
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
                let redacted_topics = repo::active_agreement_redacted_topics(&conn, state.server_id, instance.id);
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

/// Enqueue a freshly persisted local message into the federation
/// outbox for every active peer. The actual HTTP POST happens later
/// in `crate::background::start_federation_outbox_task`, which retries
/// with bounded exponential backoff against the receiver-side replay
/// ledger introduced in migration 036.
///
/// Per-peer envelope shape is unchanged from the pre-hardening relay:
///   * Skip peers whose `transfer_scope == "NO_TRANSFER"`.
///   * Use the canonical newline-delimited signing input from
///     [`message_signing_input`].
///   * Sign with the local server's Ed25519 signing key.
///   * Build envelopes with `originating_server = state.get_public_url()`
///     and `attestation_ref = "<topic>:<commitment>"` resolved via
///     `find_commitment_for_pseudonym`. Pseudonyms with no commitment
///     fall back to `"annex:server:v1:unknown"` (preserved string).
///
/// Pre-outbox behaviour was to `tokio::spawn` a fire-and-forget HTTP
/// POST per peer with no retry. The new path:
///
///   1. Build the canonical signed envelope exactly as before so the
///      signature is committed to the durable envelope JSON.
///   2. Insert one `federation_outbox` row per active peer with
///      `status='pending'`. The unique `(peer_instance_id, message_id)`
///      constraint makes duplicate enqueues a no-op.
///   3. Return — the outbox worker handles delivery.
///
/// This preserves "best-effort" semantics for callers (they don't
/// block on peer reachability) while making delivery itself durable
/// across server restarts.
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

    // Construct envelope with the canonical signing input. The
    // outbound envelope version is read from config so an operator
    // can stay on v1 while peers catch up. Defaults to v1 for one
    // release after the v2 verifier ships; flipping the default to
    // v2 once a quorum of peers verify v2 is a one-line config
    // change.
    let pub_url = state.get_public_url();
    let envelope_version = Some(
        state
            .federation_config
            .default_outbound_envelope_version
            .clone(),
    );
    let envelope_for_signing = FederatedMessageEnvelope {
        envelope_version: envelope_version.clone(),
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

    // Serialise the envelope exactly once. Each outbox row gets the
    // same bytes; the receiver verifies signature over those bytes.
    let envelope_json = match serde_json::to_string(&envelope) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to serialise outbound federation envelope: {}", e);
            return;
        }
    };
    let message_id_for_outbox = message.message_id.clone();

    // Collect peer ids that should receive this envelope, applying
    // the existing transfer-scope and SSRF filters (the outbox worker
    // does NOT re-evaluate those — they are policy that lives on the
    // sender side at enqueue time).
    let peer_ids: Vec<i64> = peers
        .into_iter()
        .filter(|p| {
            if p.transfer_scope == "NO_TRANSFER" {
                tracing::debug!(peer = %p.base_url, "skipping outbox enqueue: NO_TRANSFER");
                return false;
            }
            if crate::api_link_preview::is_url_private_or_reserved(&p.base_url) {
                tracing::warn!(
                    peer = %p.base_url,
                    "skipping outbox enqueue: peer base_url resolves to a private or reserved host"
                );
                return false;
            }
            true
        })
        .map(|p| p.id)
        .collect();

    if peer_ids.is_empty() {
        return;
    }

    // Enqueue one row per peer. UNIQUE(peer_instance_id, message_id)
    // makes a duplicate enqueue idempotent. We trip the storage gate
    // on disk-full / I/O failure so the next request fails fast
    // rather than retrying into the same error.
    let pool = state.pool.clone();
    let health = state.storage_health.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let conn = match pool.get() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("federation outbox enqueue: pool error: {}", e);
                return;
            }
        };
        for peer_id in peer_ids {
            match conn.execute(
                "INSERT OR IGNORE INTO federation_outbox \
                 (peer_instance_id, message_id, envelope_json, status, attempts, next_retry_at) \
                 VALUES (?1, ?2, ?3, 'pending', 0, datetime('now'))",
                rusqlite::params![peer_id, &message_id_for_outbox, &envelope_json],
            ) {
                Ok(_) => {}
                Err(e) => {
                    if crate::storage_health::interpret_sqlite_error(&health, &e) {
                        tracing::error!(
                            peer_instance_id = peer_id,
                            "outbox enqueue tripped storage gate: {}",
                            e
                        );
                        break;
                    }
                    tracing::warn!(peer_instance_id = peer_id, "outbox enqueue failed: {}", e);
                }
            }
        }
    })
    .await;
}

/// Enqueue a signed redaction tombstone (ADR-0011) into the federation
/// outbox for every active peer, after a local soft delete on a
/// federated channel succeeded.
///
/// Mirrors [`relay_message`]'s structure: peer listing, attestation-ref
/// resolution for the redactor, enqueue-time transfer-scope + SSRF
/// filters, one durable outbox row per peer, storage-gate trip on
/// enqueue failure. The outbox worker routes redaction rows to
/// `POST /api/federation/redactions` by the `envelopeKind`
/// discriminator in the serialized JSON; the row's `message_id` is
/// namespaced with [`REDACTION_LEDGER_PREFIX`] so it cannot collide
/// with the original message's outbox row under
/// `UNIQUE(peer_instance_id, message_id)`.
pub async fn relay_redaction(
    state: Arc<AppState>,
    channel_id: String,
    message_id: String,
    redacted_by: String,
    redaction_reason: &'static str,
) {
    let peers_result = tokio::task::spawn_blocking({
        let state = state.clone();
        let redactor = redacted_by.clone();
        move || {
            let conn = state.pool.get().map_err(|e| e.to_string())?;
            let peers =
                repo::list_active_peers(&conn, state.server_id).map_err(|e| e.to_string())?;

            let mut attestation_ref = "annex:server:v1:unknown".to_string();
            match repo::find_commitment_for_pseudonym(&conn, &redactor) {
                Ok(Some((commitment, topic))) => {
                    attestation_ref = format!("{topic}:{commitment}");
                }
                Ok(None) => {
                    tracing::debug!(redactor = %redactor, "no commitment found for redactor, using unknown attestation ref");
                }
                Err(e) => {
                    tracing::warn!(redactor = %redactor, "failed to look up commitment for redactor: {}", e);
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
            tracing::error!("Failed to fetch federation peers for redaction: {}", e);
            return;
        }
    };
    if peers.is_empty() {
        return;
    }

    let envelope_for_signing = FederatedRedactionEnvelope {
        envelope_kind: annex_federation::FEDERATED_ENVELOPE_KIND_REDACTION.to_string(),
        envelope_version: annex_federation::FEDERATED_REDACTION_ENVELOPE_V1.to_string(),
        message_id: message_id.clone(),
        channel_id,
        originating_server: state.get_public_url(),
        redacted_by,
        redaction_reason: redaction_reason.to_string(),
        attestation_ref,
        signature: String::new(), // placeholder for signing input only
        created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    };
    let signature = state
        .signing_key
        .sign(redaction_signing_input(&envelope_for_signing).as_bytes());
    let envelope = FederatedRedactionEnvelope {
        signature: hex::encode(signature.to_bytes()),
        ..envelope_for_signing
    };

    let envelope_json = match serde_json::to_string(&envelope) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to serialise outbound redaction envelope: {}", e);
            return;
        }
    };
    let outbox_key = format!("{REDACTION_LEDGER_PREFIX}{message_id}");

    let peer_ids: Vec<i64> = peers
        .into_iter()
        .filter(|p| {
            if p.transfer_scope == "NO_TRANSFER" {
                return false;
            }
            if crate::api_link_preview::is_url_private_or_reserved(&p.base_url) {
                tracing::warn!(
                    peer = %p.base_url,
                    "skipping redaction enqueue: peer base_url resolves to a private or reserved host"
                );
                return false;
            }
            true
        })
        .map(|p| p.id)
        .collect();
    if peer_ids.is_empty() {
        return;
    }

    let pool = state.pool.clone();
    let health = state.storage_health.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let conn = match pool.get() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("redaction outbox enqueue: pool error: {}", e);
                return;
            }
        };
        for peer_id in peer_ids {
            match conn.execute(
                "INSERT OR IGNORE INTO federation_outbox \
                 (peer_instance_id, message_id, envelope_json, status, attempts, next_retry_at) \
                 VALUES (?1, ?2, ?3, 'pending', 0, datetime('now'))",
                rusqlite::params![peer_id, &outbox_key, &envelope_json],
            ) {
                Ok(_) => {}
                Err(e) => {
                    if crate::storage_health::interpret_sqlite_error(&health, &e) {
                        tracing::error!(
                            peer_instance_id = peer_id,
                            "redaction outbox enqueue tripped storage gate: {}",
                            e
                        );
                        break;
                    }
                    tracing::warn!(
                        peer_instance_id = peer_id,
                        "redaction outbox enqueue failed: {}",
                        e
                    );
                }
            }
        }
    })
    .await;
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
