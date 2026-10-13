//! One transport selection for every durable federation operation.
use super::{federation_service::federation_http_client, FederationService};
use crate::AppState;
use annex_federation::transport::{ApplicationError, TransportError};
use axum::response::IntoResponse;
use rusqlite::OptionalExtension;
use std::sync::Arc;

const MAX_BODY: usize = annex_federation::transport::MAX_APPLICATION_RESPONSE_BYTES;
const MAX_REQUEST: usize = annex_federation::transport::MAX_APPLICATION_REQUEST_BYTES;

#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    #[error("{0}")]
    Remote(ApplicationError),
    #[error("{0}")]
    Unavailable(String),
}
impl From<String> for DeliveryError {
    fn from(message: String) -> Self {
        Self::Unavailable(message)
    }
}
impl From<&str> for DeliveryError {
    fn from(message: &str) -> Self {
        Self::Unavailable(message.into())
    }
}
fn rejection(error: impl std::fmt::Display + IntoResponse) -> ApplicationError {
    let message = error.to_string();
    let status = error.into_response().status().as_u16();
    ApplicationError::new(
        status,
        if status >= 500 {
            "receiver internal error".to_string()
        } else {
            message
        },
    )
}
fn malformed(error: impl std::fmt::Display) -> ApplicationError {
    ApplicationError::new(400, error.to_string())
}

/// Resolve current authority for every attempt, including established channels.
fn peer(state: &AppState, peer_id: i64) -> Result<(String, String), String> {
    let conn = state.pool.get().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT i.base_url, i.public_key FROM instances i JOIN federation_agreements a \
         ON a.remote_instance_id = i.id WHERE i.id = ?1 AND i.status = 'ACTIVE' \
         AND a.local_server_id = ?2 AND a.active = 1",
        rusqlite::params![peer_id, state.server_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "peer has no active federation agreement".into())
}

pub async fn request_typed(
    state: Arc<AppState>,
    peer_id: i64,
    endpoint: &str,
    body: &str,
) -> Result<String, DeliveryError> {
    if !matches!(
        endpoint,
        "/api/federation/messages"
            | "/api/federation/edits"
            | "/api/federation/redactions"
            | "/api/federation/rtx"
            | "/api/federation/catch-up"
            | "/api/federation/attachments/chunk"
            | "/api/federation/channel-authority/read-permit"
            | "/api/federation/channel-authority/operations"
            | "/api/federation/channel-authority/directory"
            | "/api/federation/channel-authority/catch-up"
            | "/api/federation/human-membership"
    ) {
        return Err("unsupported federation operation".into());
    }
    if body.len() > MAX_REQUEST {
        return Err("federation request exceeds size limit".into());
    }
    let (url, key) = peer(&state, peer_id)?;
    if crate::api_link_preview::is_peer_url_disallowed(
        &url,
        state.federation_config.allow_private_peer_addresses,
    ) {
        return Err("peer URL is not permitted".into());
    }
    if let Some(transport) = state.federation_transport.get() {
        let response = transport
            .request(&url, &key, endpoint, body)
            .await
            .map_err(|e| match e {
                TransportError::RemoteRejected(error) => DeliveryError::Remote(error),
                other => DeliveryError::Unavailable(other.to_string()),
            })?;
        // A revocation while the operation was in flight does not grant a fresh
        // delivery receipt to the local worker.
        peer(&state, peer_id)?;
        tracing::info!(
            peer_id,
            endpoint,
            transport = "webrtc",
            receipt = "verified",
            bytes_sent = transport
                .counters
                .bytes_sent
                .load(std::sync::atomic::Ordering::Relaxed),
            bytes_received = transport
                .counters
                .bytes_received
                .load(std::sync::atomic::Ordering::Relaxed),
            "federation operation acknowledged after receiver completion"
        );
        return Ok(response);
    }
    let mut response = federation_http_client()
        .map_err(|e| e.to_string())?
        .post(format!("{}{endpoint}", url.trim_end_matches('/')))
        .header("content-type", "application/json")
        .body(body.to_owned())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if bytes.len() + chunk.len() > MAX_BODY {
            return Err("federation response exceeds size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let body =
        String::from_utf8(bytes).map_err(|_| "federation response is not UTF-8".to_string())?;
    if !status.is_success() {
        return Err(DeliveryError::Remote(ApplicationError::new(
            status.as_u16(),
            body,
        )));
    }
    peer(&state, peer_id)?;
    tracing::info!(
        peer_id,
        endpoint,
        transport = "http",
        receipt = "received",
        "federation operation acknowledged"
    );
    Ok(body)
}

/// Compatibility for callers which only record retry diagnostics. Authority
/// mutation callers use `request_typed` to retain semantic refusal status.
pub async fn request(
    state: Arc<AppState>,
    peer_id: i64,
    endpoint: &str,
    body: &str,
) -> Result<String, String> {
    request_typed(state, peer_id, endpoint, body)
        .await
        .map_err(|e| e.to_string())
}

/// The authenticated data-channel peer must be the claimed envelope origin.
/// Service handlers still apply their complete signature/membership/receipt
/// checks and only return after their database transaction commits.
pub async fn receive(
    state: Arc<AppState>,
    peer_key: &str,
    endpoint: &str,
    body: &str,
) -> Result<String, ApplicationError> {
    if body.len() > MAX_REQUEST {
        return Err("federation request exceeds size limit".into());
    }
    let value: serde_json::Value = serde_json::from_str(body).map_err(malformed)?;
    let origin_field = match endpoint {
        "/api/federation/catch-up" | "/api/federation/attachments/chunk" => "requesting_server",
        "/api/federation/rtx" => "relaying_server",
        _ => "originating_server",
    };
    let origin = value
        .get(origin_field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApplicationError::new(400, format!("missing {origin_field}")))?;
    {
        let conn = state.pool.get().map_err(|e| e.to_string())?;
        let peer_id =
            annex_federation::db::authorized_signaling_peer(&conn, state.server_id, peer_key)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| {
                    ApplicationError::new(403, "no active agreement for data-channel peer")
                })?;
        let bound: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM instances WHERE id=?1 AND base_url=?2)",
                rusqlite::params![peer_id, origin],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !bound {
            return Err(ApplicationError::new(
                403,
                "envelope origin does not match data-channel identity",
            ));
        }
    }
    let service = FederationService::new(state.clone());
    let output = match endpoint {
        "/api/federation/messages" => {
            service
                .receive_federated_message(serde_json::from_value(value).map_err(malformed)?)
                .await
                .map_err(rejection)?;
            serde_json::json!({"status":"received","transport":"webrtc"})
        }
        "/api/federation/edits" => {
            let applied = service
                .receive_federated_edit(serde_json::from_value(value).map_err(malformed)?)
                .await
                .map_err(rejection)?;
            serde_json::json!({"status":"received","applied":applied.is_some(),"transport":"webrtc"})
        }
        "/api/federation/redactions" => {
            let applied = service
                .receive_federated_redaction(serde_json::from_value(value).map_err(malformed)?)
                .await
                .map_err(rejection)?;
            serde_json::json!({"status":"received","applied":applied.is_some(),"transport":"webrtc"})
        }
        "/api/federation/rtx" => {
            let (id, delivered) = service
                .receive_federated_rtx(serde_json::from_value(value).map_err(malformed)?)
                .await
                .map_err(rejection)?;
            serde_json::json!({"ok":true,"bundleId":id,"delivered_to":delivered,"transport":"webrtc"})
        }
        "/api/federation/catch-up" => serde_json::to_value(
            super::federation_catchup::serve(
                state,
                serde_json::from_value(value).map_err(malformed)?,
            )
            .await
            .map_err(rejection)?,
        )
        .map_err(|e| e.to_string())?,
        "/api/federation/attachments/chunk" => serde_json::to_value(
            super::federated_upload_service::serve_chunk(
                state,
                serde_json::from_value(value).map_err(malformed)?,
            )
            .await
            .map_err(rejection)?,
        )
        .map_err(|e| e.to_string())?,
        "/api/federation/channel-authority/read-permit" => serde_json::to_value(
            super::channel_authority::receive_read_permit(
                state,
                serde_json::from_value(value).map_err(malformed)?,
            )
            .await
            .map_err(rejection)?,
        )
        .map_err(|e| e.to_string())?,
        "/api/federation/channel-authority/operations" => serde_json::to_value(
            super::channel_authority::receive_operation(
                state,
                serde_json::from_value(value).map_err(malformed)?,
            )
            .await
            .map_err(rejection)?,
        )
        .map_err(|e| e.to_string())?,
        "/api/federation/channel-authority/catch-up" => serde_json::to_value(
            super::channel_authority::receive_catchup(
                state,
                serde_json::from_value(value).map_err(malformed)?,
            )
            .await
            .map_err(rejection)?,
        )
        .map_err(|e| e.to_string())?,
        "/api/federation/human-membership" => serde_json::to_value(
            super::human_federation::receive(
                state,
                serde_json::from_value(value).map_err(malformed)?,
            )
            .await
            .map_err(rejection)?,
        )
        .map_err(|e| e.to_string())?,
        "/api/federation/channel-authority/directory" => serde_json::to_value(
            super::authority_directory::receive(
                state,
                serde_json::from_value(value).map_err(malformed)?,
            )
            .await
            .map_err(rejection)?,
        )
        .map_err(|e| e.to_string())?,
        _ => {
            return Err(ApplicationError::new(
                404,
                "unsupported federation operation",
            ))
        }
    };
    serde_json::to_string(&output).map_err(|e| ApplicationError::new(500, e.to_string()))
}
