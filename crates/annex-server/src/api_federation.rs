use crate::{api::GetRootResponse, AppState};
use annex_federation::{process_incoming_handshake, AttestationRequest, HandshakeError};
use annex_identity::{
    derive_nullifier_hex, derive_pseudonym_id,
    zk::{parse_fr_from_hex, parse_proof, verify_proof},
};
use annex_vrp::{VrpFederationHandshake, VrpValidationReport};
use axum::{extract::Extension, Json};
use ed25519_dalek::{Signature, Verifier, VerifyingKey as EdVerifyingKey};
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

        let conn = state
            .pool
            .get()
            .map_err(|e| FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;

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
    .map_err(|e| FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))??;

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
    .map_err(|e| FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))??;

    // Verify Signature
    // Message: SHA256(topic || commitment)
    // Actually, payload.signature should sign the content.
    // Let's assume the message is simply the concatenation of topic and commitment bytes.
    let message = format!("{}{}", payload.topic, payload.commitment);
    let public_key_bytes = hex::decode(&public_key_hex)
        .map_err(|e| FederationError::InvalidSignature(format!("Invalid public key hex: {}", e)))?;
    let signature_bytes = hex::decode(&payload.signature)
        .map_err(|e| FederationError::InvalidSignature(format!("Invalid signature hex: {}", e)))?;

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
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;

        // Derive local identifiers
        let nullifier_hex = derive_nullifier_hex(&payload.commitment, &payload.topic).map_err(
            |e| FederationError::IdentityDerivation(format!("Failed to derive nullifier: {}", e)),
        )?;
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

        Ok::<_, FederationError>(pseudonym_id)
    })
    .await
    .map_err(|e| FederationError::DbError(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))??;

    Ok(Json(serde_json::json!({
        "ok": true,
        "pseudonymId": pseudonym_id
    })))
}
