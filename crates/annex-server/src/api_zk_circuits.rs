//! Zero-knowledge circuit endpoints for capability, linkage, and federation
//! proofs (AUDIT P4-ID-1).
//!
//! These three endpoints back the privacy claims the README advertises but
//! that previously had no cryptographic implementation:
//!
//!   - `POST /api/zk/channel-eligibility` — prove the caller is a member whose
//!     committed role equals the role a channel admits, WITHOUT revealing which
//!     member. Replaces deanonymising plaintext role-flag reads.
//!   - `POST /api/zk/link-pseudonyms` — the holder voluntarily proves two
//!     topic-scoped pseudonyms are the same identity, WITHOUT revealing the
//!     secret key. This is the "opt-in cross-server linkage" the README
//!     promises is "never automatic".
//!   - `POST /api/zk/federation-attestation` — prove a hidden member of this
//!     server's tree is attesting in a federation context, verifiable against
//!     the server's published root, WITHOUT exposing the identity database.
//!
//! Every endpoint:
//!   - returns `503 Service Unavailable` when its circuit verification key is
//!     not configured (rather than silently accepting),
//!   - verifies the Groth16 proof against the version-matched vkey,
//!   - binds the proof's public topic/context signal to the caller's claimed
//!     topic/context via [`topic_hash_for_v2`] so a proof for context A cannot
//!     be replayed as a proof for context B,
//!   - and runs the CPU-bound verification on a blocking thread.
//!
//! The nullifier domain separators (eligibility=2, federation=3) differ from
//! membership v2 (=1), enforced in-circuit, so a nullifier minted by one
//! circuit can never be replayed as another's.

use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::{Deserialize, Serialize};

use annex_identity::zk::{
    fr_to_canonical_hex, parse_fr_from_hex, parse_proof, parse_public_signals, topic_hash_for_v2,
    verify_proof, Bn254, Fr, VerifyingKey,
};

use crate::api::ApiError;
use crate::state::AppState;

/// Parse + Groth16-verify a proof of `expected_len` public signals against
/// `vkey`, returning the parsed `Fr` public signals on success.
///
/// Pure CPU work; callers wrap this in `spawn_blocking`.
fn verify_circuit_proof(
    vkey: &VerifyingKey<Bn254>,
    proof_json: &serde_json::Value,
    public_signals: &[String],
    expected_len: usize,
) -> Result<Vec<Fr>, ApiError> {
    if public_signals.len() != expected_len {
        return Err(ApiError::BadRequest(format!(
            "invalid number of public signals: expected {expected_len}, got {}",
            public_signals.len()
        )));
    }
    let proof = parse_proof(&proof_json.to_string())
        .map_err(|e| ApiError::BadRequest(format!("invalid proof format: {e}")))?;
    let signals_json = serde_json::to_string(public_signals)
        .map_err(|e| ApiError::BadRequest(format!("failed to serialize public signals: {e}")))?;
    let signals = parse_public_signals(&signals_json)
        .map_err(|e| ApiError::BadRequest(format!("invalid public signals format: {e}")))?;
    let valid = verify_proof(vkey, &proof, &signals)
        .map_err(|e| ApiError::Unauthorized(format!("proof verification failed: {e}")))?;
    if !valid {
        return Err(ApiError::Unauthorized("invalid proof".to_string()));
    }
    Ok(signals)
}

// ───────────────────────── channel-eligibility ─────────────────────────

/// `POST /api/zk/channel-eligibility` request.
///
/// Public signals layout (verified): `[root, nullifier, requiredRoleCode,
/// channelTopicHash]`.
#[derive(Debug, Deserialize)]
pub struct ChannelEligibilityRequest {
    /// The Merkle root the proof was generated against (must be acceptable).
    pub root: String,
    /// The channel's VRP topic; the server binds `channelTopicHash =
    /// topic_hash_for_v2(channel_topic)`.
    #[serde(rename = "channelTopic")]
    pub channel_topic: String,
    /// The role the channel admits. The proof must show the hidden member's
    /// committed role equals this.
    #[serde(rename = "requiredRoleCode")]
    pub required_role_code: u8,
    /// The Groth16 proof object.
    pub proof: serde_json::Value,
    /// Public signals (length 4).
    #[serde(rename = "publicSignals")]
    pub public_signals: Vec<String>,
}

/// `POST /api/zk/channel-eligibility` response.
#[derive(Debug, Serialize)]
pub struct ChannelEligibilityResponse {
    pub ok: bool,
    /// Canonical hex of the channel-scoped nullifier (`publicSignals[1]`).
    /// The caller can use this to dedupe the (still-anonymous) member within
    /// this channel.
    #[serde(rename = "nullifierHex")]
    pub nullifier_hex: String,
}

/// Handler for `POST /api/zk/channel-eligibility`.
pub async fn channel_eligibility_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<ChannelEligibilityRequest>,
) -> Result<Json<ChannelEligibilityResponse>, ApiError> {
    let vkey = state.channel_eligibility_vkey.clone().ok_or_else(|| {
        ApiError::ServiceUnavailable(
            "channel-eligibility circuit is not configured on this server".to_string(),
        )
    })?;

    let resp =
        tokio::task::spawn_blocking(move || -> Result<ChannelEligibilityResponse, ApiError> {
            let signals = verify_circuit_proof(&vkey, &payload.proof, &payload.public_signals, 4)?;

            // Bind root: must be a currently-acceptable root AND equal the
            // proof's root signal.
            let conn = state
                .pool
                .get()
                .map_err(|e| ApiError::InternalServerError(format!("db connection: {e}")))?;
            let root_ok = annex_identity::merkle::is_root_acceptable(&conn, &payload.root)
                .map_err(|e| ApiError::InternalServerError(format!("root check: {e}")))?;
            if !root_ok {
                return Err(ApiError::Conflict(format!(
                    "stale or invalid root: {}",
                    payload.root
                )));
            }
            let claimed_root = parse_fr_from_hex(&payload.root)
                .map_err(|e| ApiError::BadRequest(format!("invalid root hex: {e}")))?;
            if signals[0] != claimed_root {
                return Err(ApiError::BadRequest(
                    "proof root does not match claimed root".to_string(),
                ));
            }

            // Bind required role: publicSignals[2] must equal the claimed role.
            if signals[2] != Fr::from(payload.required_role_code as u64) {
                return Err(ApiError::Unauthorized(
                    "proof does not satisfy the required role for this channel".to_string(),
                ));
            }

            // Bind channel topic: publicSignals[3] must equal the canonical
            // hash of the claimed channel topic, so an eligibility proof for
            // channel A cannot be replayed against channel B.
            let expected_topic_hash = topic_hash_for_v2(&payload.channel_topic)
                .map_err(|e| ApiError::BadRequest(format!("invalid channel topic: {e}")))?;
            if signals[3] != expected_topic_hash {
                return Err(ApiError::BadRequest(
                    "proof's channelTopicHash does not match the claimed channel topic".to_string(),
                ));
            }

            Ok(ChannelEligibilityResponse {
                ok: true,
                nullifier_hex: fr_to_canonical_hex(signals[1]),
            })
        })
        .await
        .map_err(|e| ApiError::InternalServerError(format!("verification task failed: {e}")))??;

    Ok(Json(resp))
}

// ───────────────────────── link-pseudonyms ─────────────────────────

/// `POST /api/zk/link-pseudonyms` request.
///
/// Public signals layout (verified): `[nullifierA, nullifierB, topicHashA,
/// topicHashB]`.
#[derive(Debug, Deserialize)]
pub struct LinkPseudonymsRequest {
    /// First topic; server binds `topicHashA = topic_hash_for_v2(topic_a)`.
    #[serde(rename = "topicA")]
    pub topic_a: String,
    /// Second topic; server binds `topicHashB = topic_hash_for_v2(topic_b)`.
    #[serde(rename = "topicB")]
    pub topic_b: String,
    pub proof: serde_json::Value,
    #[serde(rename = "publicSignals")]
    pub public_signals: Vec<String>,
}

/// `POST /api/zk/link-pseudonyms` response.
#[derive(Debug, Serialize)]
pub struct LinkPseudonymsResponse {
    pub ok: bool,
    /// True: the proof cryptographically establishes both nullifiers share a
    /// secret key (same identity).
    pub linked: bool,
    #[serde(rename = "nullifierAHex")]
    pub nullifier_a_hex: String,
    #[serde(rename = "nullifierBHex")]
    pub nullifier_b_hex: String,
    /// Whether each nullifier is a pseudonym already registered on THIS server
    /// (`zk_nullifiers`). A `false` simply means the pseudonym lives on a peer
    /// or hasn't been registered here — the linkage proof itself is unaffected.
    #[serde(rename = "nullifierAKnownLocally")]
    pub nullifier_a_known_locally: bool,
    #[serde(rename = "nullifierBKnownLocally")]
    pub nullifier_b_known_locally: bool,
}

/// Handler for `POST /api/zk/link-pseudonyms`.
pub async fn link_pseudonyms_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<LinkPseudonymsRequest>,
) -> Result<Json<LinkPseudonymsResponse>, ApiError> {
    let vkey = state.link_pseudonyms_vkey.clone().ok_or_else(|| {
        ApiError::ServiceUnavailable(
            "link-pseudonyms circuit is not configured on this server".to_string(),
        )
    })?;

    if payload.topic_a == payload.topic_b {
        return Err(ApiError::BadRequest(
            "topicA and topicB must differ — linking a topic to itself is meaningless".to_string(),
        ));
    }

    let resp = tokio::task::spawn_blocking(move || -> Result<LinkPseudonymsResponse, ApiError> {
        let signals = verify_circuit_proof(&vkey, &payload.proof, &payload.public_signals, 4)?;

        // Bind both topic hashes to the claimed topics.
        let expected_a = topic_hash_for_v2(&payload.topic_a)
            .map_err(|e| ApiError::BadRequest(format!("invalid topicA: {e}")))?;
        let expected_b = topic_hash_for_v2(&payload.topic_b)
            .map_err(|e| ApiError::BadRequest(format!("invalid topicB: {e}")))?;
        if signals[2] != expected_a {
            return Err(ApiError::BadRequest(
                "proof's topicHashA does not match claimed topicA".to_string(),
            ));
        }
        if signals[3] != expected_b {
            return Err(ApiError::BadRequest(
                "proof's topicHashB does not match claimed topicB".to_string(),
            ));
        }

        let nullifier_a_hex = fr_to_canonical_hex(signals[0]);
        let nullifier_b_hex = fr_to_canonical_hex(signals[1]);

        // Best-effort: report which nullifiers are pseudonyms registered here.
        // These use the SAME domain (1) as membership v2, so a registered v2
        // pseudonym's nullifier matches exactly.
        let conn = state
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection: {e}")))?;
        let known = |n: &str| -> bool {
            conn.query_row(
                "SELECT 1 FROM zk_nullifiers WHERE nullifier_hex = ?1 LIMIT 1",
                [n],
                |_| Ok(()),
            )
            .is_ok()
        };
        let a_known = known(&nullifier_a_hex);
        let b_known = known(&nullifier_b_hex);

        Ok(LinkPseudonymsResponse {
            ok: true,
            linked: true,
            nullifier_a_hex,
            nullifier_b_hex,
            nullifier_a_known_locally: a_known,
            nullifier_b_known_locally: b_known,
        })
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("verification task failed: {e}")))??;

    Ok(Json(resp))
}

// ───────────────────────── federation-attestation ─────────────────────────

/// `POST /api/zk/federation-attestation` request.
///
/// Public signals layout (verified): `[root, nullifier, federationContextHash]`.
#[derive(Debug, Deserialize)]
pub struct FederationAttestationRequest {
    /// The Merkle root the proof attests membership under (must be acceptable
    /// on this server).
    pub root: String,
    /// The federation context string; server binds `federationContextHash =
    /// topic_hash_for_v2(federation_context)`.
    #[serde(rename = "federationContext")]
    pub federation_context: String,
    pub proof: serde_json::Value,
    #[serde(rename = "publicSignals")]
    pub public_signals: Vec<String>,
}

/// `POST /api/zk/federation-attestation` response.
#[derive(Debug, Serialize)]
pub struct FederationAttestationResponse {
    pub ok: bool,
    /// Canonical hex of the federation-context-scoped nullifier.
    #[serde(rename = "nullifierHex")]
    pub nullifier_hex: String,
    /// Echo of the root the attestation was verified against.
    pub root: String,
}

/// Handler for `POST /api/zk/federation-attestation`.
pub async fn federation_attestation_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<FederationAttestationRequest>,
) -> Result<Json<FederationAttestationResponse>, ApiError> {
    let vkey = state.federation_attestation_vkey.clone().ok_or_else(|| {
        ApiError::ServiceUnavailable(
            "federation-attestation circuit is not configured on this server".to_string(),
        )
    })?;

    let resp =
        tokio::task::spawn_blocking(move || -> Result<FederationAttestationResponse, ApiError> {
            let signals = verify_circuit_proof(&vkey, &payload.proof, &payload.public_signals, 3)?;

            let conn = state
                .pool
                .get()
                .map_err(|e| ApiError::InternalServerError(format!("db connection: {e}")))?;
            let root_ok = annex_identity::merkle::is_root_acceptable(&conn, &payload.root)
                .map_err(|e| ApiError::InternalServerError(format!("root check: {e}")))?;
            if !root_ok {
                return Err(ApiError::Conflict(format!(
                    "stale or invalid root: {}",
                    payload.root
                )));
            }
            let claimed_root = parse_fr_from_hex(&payload.root)
                .map_err(|e| ApiError::BadRequest(format!("invalid root hex: {e}")))?;
            if signals[0] != claimed_root {
                return Err(ApiError::BadRequest(
                    "proof root does not match claimed root".to_string(),
                ));
            }

            let expected_ctx = topic_hash_for_v2(&payload.federation_context)
                .map_err(|e| ApiError::BadRequest(format!("invalid federation context: {e}")))?;
            if signals[2] != expected_ctx {
                return Err(ApiError::BadRequest(
                    "proof's federationContextHash does not match the claimed context".to_string(),
                ));
            }

            Ok(FederationAttestationResponse {
                ok: true,
                nullifier_hex: fr_to_canonical_hex(signals[1]),
                root: payload.root,
            })
        })
        .await
        .map_err(|e| ApiError::InternalServerError(format!("verification task failed: {e}")))??;

    Ok(Json(resp))
}
