use crate::{api::GetRootResponse, api_ws::OutgoingMessage, AppState};
use annex_channels::{
    add_member, create_message, get_channel, is_member, list_federated_channels, Channel,
    CreateMessageParams, Message,
};
use annex_federation::{
    process_incoming_handshake, AttestationRequest, FederatedMessageEnvelope, HandshakeError,
};
use annex_graph::{ensure_graph_node, GraphError};
use annex_identity::{
    derive_nullifier_hex, derive_pseudonym_id,
    zk::{parse_fr_from_hex, parse_proof, verify_proof},
};
use annex_types::{FederationScope, NodeType};
use annex_vrp::{VrpFederationHandshake, VrpValidationReport};
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

        process_incoming_handshake(
            &conn,
            state_clone.server_id,
            &policy,
            remote_instance_id,
            &payload.handshake,
        )
        .map_err(|e| {
            tracing::error!("Handshake failed: {:?}", e);
            FederationError::Handshake(e)
        })
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

/// Relays a message to all active federation peers.
pub async fn relay_message(
    state: Arc<AppState>,
    message: annex_channels::Message,
) -> Result<(), FederationError> {
    let state_clone = state.clone();

    // 1. Identify Peers (Active Federation Agreements)
    let peers: Vec<String> = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        let mut stmt = conn
            .prepare(
                "SELECT i.base_url
             FROM federation_agreements fa
             JOIN instances i ON fa.remote_instance_id = i.id
             WHERE fa.active = 1 AND i.status = 'ACTIVE'",
            )
            .map_err(FederationError::DbError)?;

        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(FederationError::DbError)?;

        let mut urls = Vec::new();
        for row in rows {
            urls.push(row.map_err(FederationError::DbError)?);
        }
        Ok::<_, FederationError>(urls)
    })
    .await
    .map_err(|e| {
        FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
    })??;

    if peers.is_empty() {
        return Ok(());
    }

    // 2. Construct Envelope
    let envelope = FederatedMessageEnvelope {
        message_id: message.message_id.clone(),
        channel_id: message.channel_id.clone(),
        content: message.content.clone(),
        sender_pseudonym: message.sender_pseudonym.clone(),
        originating_server: state.public_url.clone(),
        attestation_ref: "implicit".to_string(), // Redundant as receiver has federated_identities
        signature: "".to_string(),
        created_at: message.created_at.clone(),
    };

    // 3. Sign
    let payload_string = format!(
        "{}{}{}{}{}{}{}",
        envelope.message_id,
        envelope.channel_id,
        envelope.content,
        envelope.sender_pseudonym,
        envelope.originating_server,
        envelope.attestation_ref,
        envelope.created_at
    );
    let signature = state.signing_key.sign(payload_string.as_bytes());
    let mut envelope = envelope;
    envelope.signature = hex::encode(signature.to_bytes());

    // 4. Send to Peers
    let client = reqwest::Client::new();
    let env_json = serde_json::to_value(&envelope).map_err(|_| {
        FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(
            std::io::Error::new(std::io::ErrorKind::InvalidData, "serialization failed"),
        )))
    })?;

    for base_url in peers {
        let url = format!("{}/api/federation/messages", base_url);
        let client = client.clone();
        let json = env_json.clone();

        tokio::spawn(async move {
            match client.post(&url).json(&json).send().await {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        tracing::warn!(
                            "Failed to relay message to {}: status {}",
                            url,
                            resp.status()
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to relay message to {}: {}", url, e);
                }
            }
        });
    }

    Ok(())
}

/// Handler for `POST /api/federation/messages`.
pub async fn receive_federated_message_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(envelope): Json<FederatedMessageEnvelope>,
) -> Result<Json<serde_json::Value>, FederationError> {
    let state_clone = state.clone();

    // 1. Verify Originating Server (Resolve Instance & Check Signature)
    let originating_server = envelope.originating_server.clone();
    let (remote_instance_id, public_key_hex, status): (i64, String, String) =
        tokio::task::spawn_blocking(move || {
            let conn = state_clone
                .pool
                .get()
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

            conn.query_row(
                "SELECT id, public_key, status FROM instances WHERE base_url = ?1",
                params![originating_server],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
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

    if status != "ACTIVE" {
        return Err(FederationError::Forbidden(format!(
            "Instance {} is not active",
            envelope.originating_server
        )));
    }

    // Verify Signature
    // Message: SHA256(message_id + channel_id + content + sender_pseudonym + originating_server + attestation_ref + created_at)
    let payload_string = format!(
        "{}{}{}{}{}{}{}",
        envelope.message_id,
        envelope.channel_id,
        envelope.content,
        envelope.sender_pseudonym,
        envelope.originating_server,
        envelope.attestation_ref,
        envelope.created_at
    );

    let public_key_bytes = hex::decode(&public_key_hex)
        .map_err(|e| FederationError::InvalidSignature(format!("Invalid public key hex: {}", e)))?;
    let signature_bytes = hex::decode(&envelope.signature)
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
        .verify(payload_string.as_bytes(), &signature)
        .map_err(|e| FederationError::InvalidSignature(e.to_string()))?;

    // 2. Verify Sender is Attested
    let state_clone = state.clone();
    let sender_pseudonym = envelope.sender_pseudonym.clone();
    let channel_id = envelope.channel_id.clone();
    let message_id = envelope.message_id.clone();
    let content = envelope.content.clone();

    let msg_opt: Option<Message> = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        // Check federated_identities
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM federated_identities WHERE remote_instance_id = ?1 AND pseudonym_id = ?2)",
                params![remote_instance_id, sender_pseudonym],
                |row| row.get(0),
            )
            .map_err(FederationError::DbError)?;

        if !exists {
            return Err(FederationError::Forbidden(format!(
                "Sender {} not attested from instance {}",
                sender_pseudonym, remote_instance_id
            )));
        }

        // 3. Verify Channel is Federated
        let channel = get_channel(&conn, &channel_id).map_err(FederationError::Channel)?;

        if channel.federation_scope == FederationScope::Local {
            return Err(FederationError::Forbidden(format!(
                "Channel {} is LOCAL only",
                channel_id
            )));
        }

        // 4. Verify Sender is a Member
        if !is_member(&conn, &channel_id, &sender_pseudonym).map_err(FederationError::Channel)? {
            // Option: Auto-join if policy allows?
            // For now, strict: must have joined via explicit join endpoint.
            return Err(FederationError::Forbidden(format!(
                "Sender {} is not a member of channel {}",
                sender_pseudonym, channel_id
            )));
        }

        // 5. Insert Message
        // Important: check if message_id already exists (idempotency)
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE message_id = ?1)",
                params![message_id],
                |row| row.get(0),
            )
            .map_err(FederationError::DbError)?;

        if exists {
            return Ok(None);
        }

        let params = CreateMessageParams {
            channel_id: channel_id.clone(),
            message_id: message_id.clone(),
            sender_pseudonym: sender_pseudonym.clone(),
            content: content.clone(),
            reply_to_message_id: None, // Replies across federation not fully supported yet or handled via ID matching
        };

        let msg = create_message(&conn, &params).map_err(FederationError::Channel)?;

        Ok::<_, FederationError>(Some(msg))
    })
    .await
    .map_err(|e| FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))??; // Flatten join error + result error

    // 6. Broadcast
    if let Some(msg) = msg_opt {
        let out = OutgoingMessage::Message(msg);
        if let Ok(json) = serde_json::to_string(&out) {
            state
                .connection_manager
                .broadcast(&envelope.channel_id, json)
                .await;
        }
    }

    Ok(Json(serde_json::json!({ "status": "received" })))
}
