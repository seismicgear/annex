use crate::{api::GetRootResponse, api_rtx::rtx_relay_signing_payload, AppState};
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
use annex_rtx::{enforce_transfer_scope, validate_bundle_structure};
use annex_types::NodeType;
use annex_vrp::{VrpFederationHandshake, VrpTransferScope, VrpValidationReport};
use axum::{
    extract::{Extension, Path},
    Json,
};
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey as EdVerifyingKey};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use std::sync::Arc;
use thiserror::Error;

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
            _ => (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                self.to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

#[derive(Deserialize)]
pub struct HandshakeRequest {
    /// Base URL of the requesting server (to identify the instance).
    pub base_url: String,
    /// The VRP handshake payload.
    #[serde(flatten)]
    pub handshake: VrpFederationHandshake,
}

#[derive(Deserialize, serde::Serialize)]
pub struct JoinFederatedChannelRequest {
    /// The base URL of the originating server.
    pub originating_server: String,
    /// The pseudonym ID of the participant joining.
    pub pseudonym_id: String,
    /// Signature of SHA256(channel_id + pseudonym_id).
    pub signature: String,
}

/// Relays a message to all active federation peers.
///
/// This function is intended to be spawned as a background task.
/// It queries all active federation agreements, constructs a signed envelope,
/// and sends it to each peer's `/api/federation/messages` endpoint.
pub async fn relay_message(
    state: Arc<AppState>,
    channel_id: String,
    message: annex_channels::Message,
) {
    let peers = tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let server_id = state.server_id;
        let sender = message.sender_pseudonym.clone();
        move || {
            let conn = pool.get().map_err(|e| e.to_string())?;

            // 1. Fetch Peers
            let mut stmt = conn
                .prepare(
                    "SELECT i.base_url, fa.transfer_scope
                 FROM federation_agreements fa
                 JOIN instances i ON fa.remote_instance_id = i.id
                 WHERE fa.local_server_id = ?1 AND fa.active = 1 AND i.status = 'ACTIVE'",
                )
                .map_err(|e| e.to_string())?;

            let rows = stmt
                .query_map(params![server_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?;

            let mut peers = Vec::new();
            for row in rows {
                peers.push(row.map_err(|e| e.to_string())?);
            }

            // 2. Find Commitment and Topic for Sender (Brute force lookup fallback)
            let mut attestation_ref = "annex:server:v1:unknown".to_string();

            if let Ok(Some((commitment, topic))) = find_commitment_for_pseudonym(&conn, &sender) {
                attestation_ref = format!("{}:{}", topic, commitment);
            }

            Ok::<_, String>((peers, attestation_ref))
        }
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));

    let (peers, attestation_ref) = match peers {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Failed to fetch federation peers: {}", e);
            return;
        }
    };

    if peers.is_empty() {
        return;
    }

    // Construct Envelope
    // Signature: SHA256(message_id + channel_id + content + sender + originating_server + attestation_ref + created_at)
    let signature_input = format!(
        "{}{}{}{}{}{}{}",
        message.message_id,
        channel_id,
        message.content,
        message.sender_pseudonym,
        state.public_url,
        attestation_ref,
        message.created_at
    );

    let signature = state.signing_key.sign(signature_input.as_bytes());
    let signature_hex = hex::encode(signature.to_bytes());

    let envelope = FederatedMessageEnvelope {
        message_id: message.message_id,
        channel_id: channel_id.clone(),
        content: message.content,
        sender_pseudonym: message.sender_pseudonym,
        originating_server: state.public_url.clone(),
        attestation_ref: attestation_ref.clone(),
        signature: signature_hex,
        created_at: message.created_at,
    };

    let client = reqwest::Client::new();

    for (base_url, _transfer_scope) in peers {
        // TODO: Check transfer scope if needed (e.g., filter content).
        // For now, we assume if federated channel, we relay.

        let url = format!("{}/api/federation/messages", base_url);
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
            if let Err(e) = client_clone.post(&url).json(&envelope_clone).send().await {
                tracing::warn!("Failed to relay message to {}: {}", url, e);
            }
        });
    }
}

fn find_commitment_for_pseudonym(
    conn: &rusqlite::Connection,
    pseudonym: &str,
) -> Result<Option<(String, String)>, rusqlite::Error> {
    // 1. Scan `zk_nullifiers` to find potential topic and nullifier_hex
    let mut stmt = conn.prepare("SELECT topic, nullifier_hex FROM zk_nullifiers")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut candidate_nullifiers = Vec::new();

    for row in rows {
        let (topic, nullifier_hex) = row?;
        if let Ok(p) = annex_identity::derive_pseudonym_id(&topic, &nullifier_hex) {
            if p == pseudonym {
                candidate_nullifiers.push((topic, nullifier_hex));
            }
        }
    }

    if candidate_nullifiers.is_empty() {
        return Ok(None);
    }

    // 2. Scan `vrp_identities` to find matching commitment
    let mut stmt = conn.prepare("SELECT commitment_hex FROM vrp_identities")?;
    let commitments: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .filter_map(Result::ok)
        .collect();

    for (topic, nullifier) in candidate_nullifiers {
        for commitment in &commitments {
            if let Ok(n) = annex_identity::derive_nullifier_hex(commitment, &topic) {
                if n == nullifier {
                    return Ok(Some((commitment.clone(), topic)));
                }
            }
        }
    }

    Ok(None)
}

/// Handler for `POST /api/federation/messages`.
pub async fn receive_federated_message_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(envelope): Json<FederatedMessageEnvelope>,
) -> Result<Json<serde_json::Value>, FederationError> {
    let state_clone = state.clone();
    let channel_id_clone = envelope.channel_id.clone();

    let inserted = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        // 1. Resolve Remote Instance
        let (remote_instance_id, public_key_hex, status): (i64, String, String) = conn
            .query_row(
                "SELECT id, public_key, status FROM instances WHERE base_url = ?1",
                params![envelope.originating_server],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| {
                if e == rusqlite::Error::QueryReturnedNoRows {
                    FederationError::UnknownRemote(envelope.originating_server.clone())
                } else {
                    FederationError::DbError(e)
                }
            })?;

        if status != "ACTIVE" {
            return Err(FederationError::Forbidden(format!(
                "Instance {} is not active",
                envelope.originating_server
            )));
        }

        // 1.5. Verify Active Federation Agreement
        let agreement_active: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM federation_agreements WHERE remote_instance_id = ?1 AND active = 1)",
                params![remote_instance_id],
                |row| row.get(0),
            )
            .map_err(FederationError::DbError)?;

        if !agreement_active {
            return Err(FederationError::Forbidden(format!(
                "No active federation agreement with {}",
                envelope.originating_server
            )));
        }

        // 2. Verify Signature
        let signature_input = format!(
            "{}{}{}{}{}{}{}",
            envelope.message_id,
            envelope.channel_id,
            envelope.content,
            envelope.sender_pseudonym,
            envelope.originating_server,
            envelope.attestation_ref,
            envelope.created_at
        );

        let public_key_bytes = hex::decode(&public_key_hex).map_err(|e| {
            FederationError::InvalidSignature(format!("Invalid public key hex: {}", e))
        })?;
        let signature_bytes = hex::decode(&envelope.signature).map_err(|e| {
            FederationError::InvalidSignature(format!("Invalid signature hex: {}", e))
        })?;

        let public_key =
            EdVerifyingKey::from_bytes(&public_key_bytes.try_into().map_err(|_| {
                FederationError::InvalidSignature("Invalid public key length".to_string())
            })?)
            .map_err(|e| FederationError::InvalidSignature(e.to_string()))?;

        let signature = Signature::from_bytes(&signature_bytes.try_into().map_err(|_| {
            FederationError::InvalidSignature("Invalid signature length".to_string())
        })?);

        public_key
            .verify(signature_input.as_bytes(), &signature)
            .map_err(|e| FederationError::InvalidSignature(e.to_string()))?;

        // 3. Parse Attestation Ref to get Commitment and Topic
        // Format: "topic:commitment_hex"
        let parts: Vec<&str> = envelope.attestation_ref.split(':').collect();
        if parts.len() < 2 {
            return Err(FederationError::Forbidden(
                "Invalid attestation ref format".to_string(),
            ));
        }
        let commitment_hex = parts.last().unwrap();
        // Topic is everything before the last colon
        let _topic = envelope
            .attestation_ref
            .strip_suffix(commitment_hex)
            .unwrap()
            .strip_suffix(':')
            .ok_or(FederationError::Forbidden(
                "Invalid attestation ref format".to_string(),
            ))?;

        // 4. Verify Sender in Federated Identities
        let local_pseudonym_id: String = conn
            .query_row(
                "SELECT pseudonym_id FROM federated_identities
             WHERE remote_instance_id = ?1 AND commitment_hex = ?2",
                params![remote_instance_id, commitment_hex],
                |row| row.get(0),
            )
            .map_err(|e| {
                if e == rusqlite::Error::QueryReturnedNoRows {
                    FederationError::Forbidden(format!(
                        "Identity with commitment {} not attested",
                        commitment_hex
                    ))
                } else {
                    FederationError::DbError(e)
                }
            })?;

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
        let is_member = annex_channels::is_member(&conn, &envelope.channel_id, &local_pseudonym_id)
            .map_err(FederationError::Channel)?;

        if !is_member {
            return Err(FederationError::Forbidden(format!(
                "User {} is not a member of channel {}",
                local_pseudonym_id, envelope.channel_id
            )));
        }

        // 7. Insert Message
        let params = CreateMessageParams {
            channel_id: envelope.channel_id.clone(),
            message_id: envelope.message_id.clone(),
            sender_pseudonym: local_pseudonym_id.clone(),
            content: envelope.content.clone(),
            reply_to_message_id: None,
        };

        match create_message(&conn, &params) {
            Ok(msg) => Ok(Some(msg)),
            Err(annex_channels::ChannelError::Database(rusqlite::Error::SqliteFailure(
                code,
                _,
            ))) if code.code == rusqlite::ffi::ErrorCode::ConstraintViolation => {
                // Duplicate message (idempotency)
                Ok(None)
            }
            Err(e) => Err(FederationError::Channel(e)),
        }
    })
    .await
    .map_err(|e| {
        FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })??;

    // 8. Broadcast
    if let Some(msg) = inserted {
        let out = crate::api_ws::OutgoingMessage::Message(msg);
        if let Ok(json) = serde_json::to_string(&out) {
            state
                .connection_manager
                .broadcast(&channel_id_clone, json)
                .await;
        }
    }

    Ok(Json(serde_json::json!({ "status": "received" })))
}

pub async fn federation_handshake_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<HandshakeRequest>,
) -> Result<Json<VrpValidationReport>, FederationError> {
    let state_clone = state.clone();

    // Perform database operations in blocking thread
    let result = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?; // Wrap pool error

        // 1. Resolve remote instance ID from base_url
        tracing::debug!("Resolving instance for base_url: {}", payload.base_url);
        let remote_instance_id: i64 = conn
            .query_row(
                "SELECT id FROM instances WHERE base_url = ?1",
                params![payload.base_url],
                |row| row.get(0),
            )
            .map_err(|e| {
                tracing::error!("Instance resolution failed: {:?}", e);
                if e == rusqlite::Error::QueryReturnedNoRows {
                    FederationError::UnknownRemote(payload.base_url.clone())
                } else {
                    FederationError::DbError(e)
                }
            })?;

        // 2. Process handshake
        tracing::debug!(
            "Processing handshake for instance id: {}",
            remote_instance_id
        );
        let policy = state_clone.policy.read().unwrap();

        let report = process_incoming_handshake(
            &conn,
            state_clone.server_id,
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
            state_clone.server_id,
            &payload.base_url,
            &observe_payload,
            &state_clone.observe_tx,
        );

        Ok::<_, FederationError>(report)
    })
    .await
    .map_err(|e| {
        FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })??;

    Ok(Json(result))
}

/// Handler for `GET /api/federation/vrp-root`.
pub async fn get_vrp_root_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<GetRootResponse>, FederationError> {
    // Reusing the same logic as /api/registry/current-root, but exposed under federation
    // This allows us to potentially filter or transform for federation peers in the future.
    let result = tokio::task::spawn_blocking(move || {
        let (root_hex, leaf_count) = {
            let tree = state
                .merkle_tree
                .lock()
                .map_err(|_| FederationError::LockPoisoned)?;
            (tree.root_hex(), tree.next_index)
        };

        let conn = state.pool.get().map_err(|e| {
            FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        })?;

        let updated_at: Option<String> = conn
            .query_row(
                "SELECT created_at FROM vrp_roots WHERE root_hex = ?1",
                [&root_hex],
                |row| row.get(0),
            )
            .optional()
            .map_err(FederationError::DbError)?;

        Ok::<_, FederationError>((root_hex, leaf_count, updated_at))
    })
    .await
    .map_err(|e| {
        FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })??;

    Ok(Json(GetRootResponse {
        root_hex: result.0,
        leaf_count: result.1,
        updated_at: result.2,
    }))
}

/// Handler for `POST /api/federation/attest-membership`.
pub async fn attest_membership_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<AttestationRequest>,
) -> Result<Json<serde_json::Value>, FederationError> {
    let state_clone = state.clone();

    // 1. Verify Request Origin (Resolve Instance & Check Signature)
    let originating_server = payload.originating_server.clone();
    let (remote_instance_id, public_key_hex) = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        conn.query_row(
            "SELECT id, public_key FROM instances WHERE base_url = ?1",
            params![originating_server],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|e| {
            if e == rusqlite::Error::QueryReturnedNoRows {
                FederationError::UnknownRemote(originating_server.clone())
            } else {
                FederationError::DbError(e)
            }
        })
    })
    .await
    .map_err(|e| {
        FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })??;

    // Verify Signature
    // Message: SHA256(topic || commitment || participant_type)
    let message = format!(
        "{}{}{}",
        payload.topic, payload.commitment, payload.participant_type
    );
    let public_key_bytes = hex::decode(&public_key_hex)
        .map_err(|e| FederationError::InvalidSignature(format!("Invalid public key hex: {}", e)))?;
    let signature_bytes = hex::decode(&payload.signature)
        .map_err(|e| FederationError::InvalidSignature(format!("Invalid signature hex: {}", e)))?;

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
        .verify(message.as_bytes(), &signature)
        .map_err(|e| FederationError::InvalidSignature(e.to_string()))?;

    // 2. Fetch Remote Root
    let client = reqwest::Client::new();
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
        .map_err(|e| FederationError::ZkVerification(format!("Invalid proof format: {}", e)))?;

    // Construct public inputs: [root, commitment]
    // Note: Verify input order in membership.circom.
    // In api.rs, it checks: public_signals[0] == root, public_signals[1] == commitment.
    // So we assume the circuit public outputs are [root, commitment].
    let remote_root_fr = parse_fr_from_hex(&remote_root_hex)
        .map_err(|e| FederationError::ZkVerification(format!("Invalid root hex: {}", e)))?;
    let commitment_fr = parse_fr_from_hex(&payload.commitment)
        .map_err(|e| FederationError::ZkVerification(format!("Invalid commitment hex: {}", e)))?;

    let public_inputs = vec![remote_root_fr, commitment_fr];

    let valid = verify_proof(&state_clone.membership_vkey, &proof, &public_inputs)
        .map_err(|e| FederationError::ZkVerification(format!("Proof verification error: {}", e)))?;

    if !valid {
        return Err(FederationError::ZkVerification("Invalid proof".to_string()));
    }

    // 4. Persist Attestation
    let pseudonym_id = tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
        })?;

        // Derive local identifiers
        let nullifier_hex =
            derive_nullifier_hex(&payload.commitment, &payload.topic).map_err(|e| {
                FederationError::IdentityDerivation(format!("Failed to derive nullifier: {}", e))
            })?;
        let pseudonym_id = derive_pseudonym_id(&payload.topic, &nullifier_hex).map_err(|e| {
            FederationError::IdentityDerivation(format!("Failed to derive pseudonym: {}", e))
        })?;

        // Insert into federated_identities
        conn.execute(
            "INSERT INTO federated_identities (
                server_id, remote_instance_id, commitment_hex, pseudonym_id, vrp_topic, attested_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
            ON CONFLICT(server_id, remote_instance_id, pseudonym_id) DO UPDATE SET
                attested_at = datetime('now'),
                commitment_hex = excluded.commitment_hex,
                vrp_topic = excluded.vrp_topic
            ",
            params![
                state_clone.server_id,
                remote_instance_id,
                payload.commitment,
                pseudonym_id,
                payload.topic
            ],
        )
        .map_err(FederationError::DbError)?;

        // Ensure Platform Identity
        // We set default capabilities to 0 (false) for federated users initially.
        // They can be upgraded later based on VRP negotiation if needed.
        conn.execute(
            "INSERT INTO platform_identities (
                server_id, pseudonym_id, participant_type, active
            ) VALUES (?1, ?2, ?3, 1)
            ON CONFLICT(server_id, pseudonym_id) DO UPDATE SET
                active = 1,
                participant_type = excluded.participant_type
            ",
            params![
                state_clone.server_id,
                pseudonym_id,
                payload.participant_type
            ],
        )
        .map_err(FederationError::DbError)?;

        // Ensure Graph Node
        let node_type = match payload.participant_type.as_str() {
            "HUMAN" => NodeType::Human,
            "AI_AGENT" => NodeType::AiAgent,
            "COLLECTIVE" => NodeType::Collective,
            "BRIDGE" => NodeType::Bridge,
            "SERVICE" => NodeType::Service,
            _ => NodeType::Human, // Fallback
        };

        ensure_graph_node(
            &conn,
            state_clone.server_id,
            &pseudonym_id,
            node_type,
            None, // metadata_json
        )
        .map_err(|e| match e {
            GraphError::DatabaseError(err) => FederationError::DbError(err),
            _ => FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e))),
        })?;

        Ok::<_, FederationError>(pseudonym_id)
    })
    .await
    .map_err(|e| {
        FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })??;

    Ok(Json(serde_json::json!({
        "ok": true,
        "pseudonymId": pseudonym_id
    })))
}

/// Handler for `GET /api/federation/channels`.
pub async fn get_federated_channels_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<Channel>>, FederationError> {
    let channels = tokio::task::spawn_blocking(move || {
        let conn = state
            .pool
            .get()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        list_federated_channels(&conn, state.server_id).map_err(FederationError::Channel)
    })
    .await
    .map_err(|e| {
        FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })??;

    Ok(Json(channels))
}

/// Handler for `POST /api/federation/channels/:channelId/join`.
pub async fn join_federated_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Path(channel_id): Path<String>,
    Json(payload): Json<JoinFederatedChannelRequest>,
) -> Result<Json<serde_json::Value>, FederationError> {
    let state_clone = state.clone();
    let channel_id_clone = channel_id.clone();

    tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        // 1. Verify Originating Server
        let (remote_instance_id, public_key_hex, status): (i64, String, String) = conn
            .query_row(
                "SELECT id, public_key, status FROM instances WHERE base_url = ?1",
                params![payload.originating_server],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| {
                if e == rusqlite::Error::QueryReturnedNoRows {
                    FederationError::UnknownRemote(payload.originating_server.clone())
                } else {
                    FederationError::DbError(e)
                }
            })?;

        if status != "ACTIVE" {
            return Err(FederationError::Forbidden(format!(
                "Instance {} is not active",
                payload.originating_server
            )));
        }

        // 1.5. Verify Active Federation Agreement
        let agreement_active: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM federation_agreements WHERE remote_instance_id = ?1 AND active = 1)",
                params![remote_instance_id],
                |row| row.get(0),
            )
            .map_err(FederationError::DbError)?;

        if !agreement_active {
            return Err(FederationError::Forbidden(format!(
                "No active federation agreement with {}",
                payload.originating_server
            )));
        }

        // 2. Verify Signature
        // Message: SHA256(channel_id + pseudonym_id)
        let message = format!("{}{}", channel_id_clone, payload.pseudonym_id);
        let public_key_bytes = hex::decode(&public_key_hex).map_err(|e| {
            FederationError::InvalidSignature(format!("Invalid public key hex: {}", e))
        })?;
        let signature_bytes = hex::decode(&payload.signature).map_err(|e| {
            FederationError::InvalidSignature(format!("Invalid signature hex: {}", e))
        })?;

        let public_key = EdVerifyingKey::from_bytes(&public_key_bytes.try_into().map_err(|_| {
            FederationError::InvalidSignature("Invalid public key length".to_string())
        })?)
        .map_err(|e| FederationError::InvalidSignature(e.to_string()))?;

        let signature = Signature::from_bytes(&signature_bytes.try_into().map_err(|_| {
            FederationError::InvalidSignature("Invalid signature length".to_string())
        })?);

        public_key
            .verify(message.as_bytes(), &signature)
            .map_err(|e| FederationError::InvalidSignature(e.to_string()))?;

        // 3. Verify Federated Identity Exists
        // Must match remote_instance_id AND pseudonym_id
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM federated_identities WHERE remote_instance_id = ?1 AND pseudonym_id = ?2)",
                params![remote_instance_id, payload.pseudonym_id],
                |row| row.get(0),
            )
            .map_err(FederationError::DbError)?;

        if !exists {
            return Err(FederationError::Forbidden(format!(
                "Identity {} not attested for instance {}",
                payload.pseudonym_id, payload.originating_server
            )));
        }

        // 4. Add Member
        add_member(
            &conn,
            state_clone.server_id,
            &channel_id_clone,
            &payload.pseudonym_id,
        )
        .map_err(FederationError::Channel)?;

        Ok::<(), FederationError>(())
    })
    .await
    .map_err(|e| FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))??;

    Ok(Json(serde_json::json!({ "status": "joined" })))
}

/// Parses a transfer scope string from the database.
fn parse_transfer_scope(s: &str) -> Option<VrpTransferScope> {
    match s {
        "FULL_KNOWLEDGE_BUNDLE" => Some(VrpTransferScope::FullKnowledgeBundle),
        "REFLECTION_SUMMARIES_ONLY" => Some(VrpTransferScope::ReflectionSummariesOnly),
        "NO_TRANSFER" => Some(VrpTransferScope::NoTransfer),
        _ => None,
    }
}

/// Handler for `POST /api/federation/rtx`.
///
/// Receives an RTX bundle relayed from a federation peer. Validates:
/// 1. The relaying server is a known, active instance
/// 2. An active federation agreement exists with sufficient transfer scope
/// 3. The server's Ed25519 signature on the envelope is valid
/// 4. The bundle structure is well-formed
/// 5. The bundle has not already been stored (idempotency via bundle_id uniqueness)
///
/// On success, stores the bundle with provenance, logs the transfer, and delivers
/// to local subscribers with `accept_federated = true`.
pub async fn receive_federated_rtx_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(envelope): Json<FederatedRtxEnvelope>,
) -> Result<Json<serde_json::Value>, FederationError> {
    let state_clone = state.clone();
    let bundle = envelope.bundle.clone();

    let (delivered_count, deliveries) = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        // 1. Resolve relaying server instance
        let (remote_instance_id, public_key_hex, status): (i64, String, String) = conn
            .query_row(
                "SELECT id, public_key, status FROM instances WHERE base_url = ?1",
                params![envelope.relaying_server],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| {
                if e == rusqlite::Error::QueryReturnedNoRows {
                    FederationError::UnknownRemote(envelope.relaying_server.clone())
                } else {
                    FederationError::DbError(e)
                }
            })?;

        if status != "ACTIVE" {
            return Err(FederationError::Forbidden(format!(
                "Instance {} is not active",
                envelope.relaying_server
            )));
        }

        // 2. Verify active federation agreement and check transfer scope
        let transfer_scope_str: String = conn
            .query_row(
                "SELECT transfer_scope FROM federation_agreements
                 WHERE remote_instance_id = ?1 AND active = 1",
                params![remote_instance_id],
                |row| row.get(0),
            )
            .map_err(|e| {
                if e == rusqlite::Error::QueryReturnedNoRows {
                    FederationError::Forbidden(format!(
                        "No active federation agreement with {}",
                        envelope.relaying_server
                    ))
                } else {
                    FederationError::DbError(e)
                }
            })?;

        let agreement_scope = parse_transfer_scope(&transfer_scope_str).ok_or_else(|| {
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

        let public_key_bytes = hex::decode(&public_key_hex).map_err(|e| {
            FederationError::InvalidSignature(format!("Invalid public key hex: {}", e))
        })?;
        let signature_bytes = hex::decode(&envelope.signature).map_err(|e| {
            FederationError::InvalidSignature(format!("Invalid signature hex: {}", e))
        })?;

        let public_key =
            EdVerifyingKey::from_bytes(&public_key_bytes.try_into().map_err(|_| {
                FederationError::InvalidSignature("Invalid public key length".to_string())
            })?)
            .map_err(|e| FederationError::InvalidSignature(e.to_string()))?;

        let signature = Signature::from_bytes(&signature_bytes.try_into().map_err(|_| {
            FederationError::InvalidSignature("Invalid signature length".to_string())
        })?);

        public_key
            .verify(signing_payload.as_bytes(), &signature)
            .map_err(|e| FederationError::InvalidSignature(e.to_string()))?;

        // 4. Validate bundle structure
        validate_bundle_structure(&envelope.bundle)
            .map_err(|e| FederationError::Forbidden(format!("Invalid bundle structure: {}", e)))?;

        // 5. Enforce the local federation agreement's transfer scope on the bundle
        //    (may strip reasoning_chain if our agreement is ReflectionSummariesOnly)
        let scoped_bundle = enforce_transfer_scope(&envelope.bundle, agreement_scope)
            .map_err(|e| FederationError::Forbidden(e.to_string()))?;

        // 6. Store bundle with provenance (idempotent on duplicate bundle_id)
        let domain_tags_json = serde_json::to_string(&scoped_bundle.domain_tags)
            .map_err(FederationError::Serialization)?;
        let caveats_json = serde_json::to_string(&scoped_bundle.caveats)
            .map_err(FederationError::Serialization)?;
        let provenance_json =
            serde_json::to_string(&envelope.provenance).map_err(FederationError::Serialization)?;

        let insert_result = conn.execute(
            "INSERT INTO rtx_bundles (
                server_id, bundle_id, source_pseudonym, source_server,
                domain_tags_json, summary, reasoning_chain, caveats_json,
                created_at_ms, signature, vrp_handshake_ref, provenance_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                state_clone.server_id,
                scoped_bundle.bundle_id,
                scoped_bundle.source_pseudonym,
                scoped_bundle.source_server,
                domain_tags_json,
                scoped_bundle.summary,
                scoped_bundle.reasoning_chain,
                caveats_json,
                scoped_bundle.created_at as i64,
                scoped_bundle.signature,
                scoped_bundle.vrp_handshake_ref,
                provenance_json,
            ],
        );

        match insert_result {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(ref err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                // Duplicate bundle (idempotent) — already received
                return Ok((0, Vec::new()));
            }
            Err(e) => return Err(FederationError::DbError(e)),
        }

        // 7. Log the federated transfer
        let redactions = if scoped_bundle.reasoning_chain.is_none()
            && envelope.bundle.reasoning_chain.is_some()
        {
            Some("reasoning_chain_stripped")
        } else {
            None
        };

        conn.execute(
            "INSERT INTO rtx_transfer_log (
                server_id, bundle_id, source_pseudonym, destination_pseudonym,
                transfer_scope_applied, redactions_applied
            ) VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
            params![
                state_clone.server_id,
                scoped_bundle.bundle_id,
                scoped_bundle.source_pseudonym,
                agreement_scope.to_string(),
                redactions,
            ],
        )
        .map_err(FederationError::DbError)?;

        // 8. Find matching local subscribers with accept_federated = true
        let mut deliveries: Vec<(String, String)> = Vec::new();

        let mut stmt = conn
            .prepare(
                "SELECT s.subscriber_pseudonym, s.domain_filters_json, a.transfer_scope
                 FROM rtx_subscriptions s
                 JOIN agent_registrations a
                   ON a.server_id = s.server_id AND a.pseudonym_id = s.subscriber_pseudonym
                 WHERE s.server_id = ?1 AND s.accept_federated = 1 AND a.active = 1",
            )
            .map_err(FederationError::DbError)?;

        let rows = stmt
            .query_map(params![state_clone.server_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(FederationError::DbError)?;

        for row in rows {
            let (sub_pseudonym, domain_filters_json, scope_str) =
                row.map_err(FederationError::DbError)?;

            // Parse domain filters
            let domain_filters: Vec<String> =
                serde_json::from_str(&domain_filters_json).unwrap_or_default();

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
            let receiver_scope = match parse_transfer_scope(&scope_str) {
                Some(s) if s >= VrpTransferScope::ReflectionSummariesOnly => s,
                _ => continue,
            };

            // Apply receiver's transfer scope enforcement
            let receiver_bundle = match enforce_transfer_scope(&scoped_bundle, receiver_scope) {
                Ok(b) => b,
                Err(_) => continue,
            };

            let payload = serde_json::json!({
                "type": "rtx_bundle",
                "bundle": receiver_bundle,
                "federated": true,
                "provenance": envelope.provenance,
            });

            if let Ok(json) = serde_json::to_string(&payload) {
                // Log delivery
                let delivery_redactions = if receiver_scope
                    == VrpTransferScope::ReflectionSummariesOnly
                    && scoped_bundle.reasoning_chain.is_some()
                {
                    Some("reasoning_chain_stripped")
                } else {
                    None
                };

                let _ = conn.execute(
                    "INSERT INTO rtx_transfer_log (
                        server_id, bundle_id, source_pseudonym, destination_pseudonym,
                        transfer_scope_applied, redactions_applied
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        state_clone.server_id,
                        scoped_bundle.bundle_id,
                        scoped_bundle.source_pseudonym,
                        sub_pseudonym,
                        receiver_scope.to_string(),
                        delivery_redactions,
                    ],
                );

                deliveries.push((sub_pseudonym, json));
            }
        }

        let count = deliveries.len();
        Ok::<_, FederationError>((count, deliveries))
    })
    .await
    .map_err(|e| {
        FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })??;

    // 9. Deliver via WebSocket (async, outside spawn_blocking)
    for (pseudonym, json) in &deliveries {
        state.connection_manager.send(pseudonym, json.clone()).await;
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "bundleId": bundle.bundle_id,
        "delivered_to": delivered_count,
    })))
}
