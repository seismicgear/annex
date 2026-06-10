//! HTTP handlers for the federation surface.
//!
//! Every handler here is a thin shim that constructs a
//! [`crate::services::FederationService`] from the shared `AppState`,
//! deserialises the request, hands it to the matching service method,
//! and serialises the response. Orchestration — DB access, Ed25519 /
//! ZK verification, RTX delivery — lives in
//! `crate::services::federation_service`. Pure SQL helpers live in
//! `crate::services::federation_repository`.
//!
//! The few public free items here are all backwards-compatibility
//! anchors for existing call sites:
//!
//!   * [`relay_message`] — used by `crate::ws::commands::message` to
//!     fan a freshly persisted local message out to active federation
//!     peers.
//!   * [`federation_http_client`] — used by `crate::api_rtx` and
//!     `crate::policy` for outbound federation HTTP.
//!   * [`find_commitment_for_pseudonym`] — used by
//!     `crate::services::channel_service` to bind a ZK proof to the
//!     authenticated identity.
//!   * [`receive_federated_message_from_data_channel`] — entry point
//!     used by the WebRTC data-channel ingress for federated messages.
//!
//! [`FederationError`] is re-exported so existing imports
//! (`crate::api_federation::FederationError`) keep resolving.

use std::sync::Arc;

use annex_channels::Channel;
use annex_federation::{
    AttestationRequest, FederatedMessageEnvelope, FederatedRedactionEnvelope, FederatedRtxEnvelope,
};
use annex_vrp::VrpValidationReport;
use axum::{
    extract::{Extension, Path},
    Json,
};

use crate::api::GetRootResponse;
use crate::services::FederationService;
use crate::AppState;

// ── Public re-exports — preserve `crate::api_federation::Foo` paths ──
//
// `find_commitment_for_pseudonym` is `pub(crate)` in the repository,
// so the re-export here matches that visibility — `services::channel_service`
// and other intra-crate consumers still see it via the legacy path.
pub(crate) use crate::services::federation_repository::find_commitment_for_pseudonym;
pub use crate::services::federation_service::{
    federation_http_client, relay_message, relay_redaction, FederationError, HandshakeRequest,
    JoinFederatedChannelRequest,
};

/// Handler for `POST /api/federation/handshake`.
pub async fn federation_handshake_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<HandshakeRequest>,
) -> Result<Json<VrpValidationReport>, FederationError> {
    let svc = FederationService::new(state);
    let report = svc.process_handshake(payload).await?;
    Ok(Json(report))
}

/// Handler for `GET /api/federation/vrp-root`.
pub async fn get_vrp_root_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<GetRootResponse>, FederationError> {
    let svc = FederationService::new(state);
    let resp = svc.current_vrp_root().await?;
    Ok(Json(resp))
}

/// Handler for `POST /api/federation/attest-membership`.
pub async fn attest_membership_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<AttestationRequest>,
) -> Result<Json<serde_json::Value>, FederationError> {
    let svc = FederationService::new(state);
    let pseudonym_id = svc.attest_membership(payload).await?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "pseudonymId": pseudonym_id
    })))
}

/// Handler for `GET /api/federation/channels`.
pub async fn get_federated_channels_handler(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<Channel>>, FederationError> {
    let svc = FederationService::new(state);
    let channels = svc.list_federated_channels().await?;
    Ok(Json(channels))
}

/// Handler for `POST /api/federation/channels/:channelId/join`.
pub async fn join_federated_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Path(channel_id): Path<String>,
    Json(payload): Json<JoinFederatedChannelRequest>,
) -> Result<Json<serde_json::Value>, FederationError> {
    let svc = FederationService::new(state);
    svc.join_federated_channel(channel_id, payload).await?;
    Ok(Json(serde_json::json!({ "status": "joined" })))
}

/// Handler for `POST /api/federation/messages`.
pub async fn receive_federated_message_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(envelope): Json<FederatedMessageEnvelope>,
) -> Result<Json<serde_json::Value>, FederationError> {
    let svc = FederationService::new(state);
    svc.receive_federated_message(envelope).await?;
    Ok(Json(serde_json::json!({ "status": "received" })))
}

/// Handler for `POST /api/federation/redactions` (ADR-0011 tombstones).
///
/// Applies a signed redaction tombstone from a federation peer: the
/// local copy of the message is blanked (`content = ''`,
/// `deleted_at = now`) after the full verification chain in
/// `FederationService::receive_federated_redaction` passes. Idempotent
/// on the receipt ledger — re-delivery of the same envelope returns
/// `applied: false` without error so outbox retries are safe.
pub async fn receive_federated_redaction_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(envelope): Json<FederatedRedactionEnvelope>,
) -> Result<Json<serde_json::Value>, FederationError> {
    let svc = FederationService::new(state);
    let applied_channel = svc.receive_federated_redaction(envelope).await?;
    Ok(Json(serde_json::json!({
        "status": "received",
        "applied": applied_channel.is_some(),
    })))
}

/// Receive a federated message that arrived over a WebRTC data
/// channel rather than over HTTP. Used by the in-process WebRTC
/// ingress; runs the same orchestration as the HTTP handler.
pub async fn receive_federated_message_from_data_channel(
    state: Arc<AppState>,
    envelope_json: &str,
) -> Result<(), FederationError> {
    let envelope: FederatedMessageEnvelope = serde_json::from_str(envelope_json)?;
    let svc = FederationService::new(state);
    svc.receive_federated_message(envelope).await
}

/// Handler for `POST /api/federation/rtx`.
pub async fn receive_federated_rtx_handler(
    Extension(state): Extension<Arc<AppState>>,
    Json(envelope): Json<FederatedRtxEnvelope>,
) -> Result<Json<serde_json::Value>, FederationError> {
    let svc = FederationService::new(state);
    let (bundle_id, delivered_to) = svc.receive_federated_rtx(envelope).await?;
    Ok(Json(serde_json::json!({
        "ok": true,
        "bundleId": bundle_id,
        "delivered_to": delivered_to,
    })))
}
