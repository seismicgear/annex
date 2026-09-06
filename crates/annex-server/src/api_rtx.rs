//! HTTP handlers for the RTX (Reflection Transfer Exchange) surface.
//!
//! Every handler here is a thin shim that constructs an
//! [`crate::services::RtxService`] from the shared `AppState`,
//! deserialises the request, hands it to the matching service method,
//! and serialises the response. Orchestration — DB access,
//! capability-contract redaction enforcement, transfer-scope
//! application, per-subscriber delivery, federation relay — lives in
//! `crate::services::rtx_service`. Pure SQL helpers live in
//! `crate::services::rtx_repository`.
//!
//! Public anchors preserved for existing call sites:
//!
//!   * [`rtx_relay_signing_payload`] — used by
//!     `crate::services::federation_service` for the receive-RTX path
//!     and by `crate::services::rtx_service` for outbound relay.
//!     Re-exported here so `annex_server::api_rtx::rtx_relay_signing_payload`
//!     keeps resolving (used by `tests/api_federation_rtx_relay.rs`).
//!   * Wire-format types (`PublishResponse`, `SubscribeRequest`,
//!     `SubscribeResponse`, `SubscriptionInfo`, `TransferLogQuery`,
//!     `TransferLogEntry`, `TransferLogResponse`,
//!     `GovernanceSummaryResponse`, `ScopeBreakdown`) are re-exported
//!     so any direct `annex_server::api_rtx::Foo` references continue
//!     to compile.

use std::sync::Arc;

use axum::{
    extract::{Extension, Query},
    Json,
};

use crate::api::ApiError;
use crate::middleware::IdentityContext;
use crate::services::RtxService;
use crate::AppState;

// ── Public re-exports — preserve `crate::api_rtx::Foo` paths ──────────
pub use crate::services::rtx_service::{
    relay_rtx_bundles, rtx_bundle_content_hash, rtx_relay_signing_payload,
    GovernanceSummaryResponse, PublishResponse, ScopeBreakdown, SubscribeRequest,
    SubscribeResponse, SubscriptionInfo, TransferLogEntry, TransferLogQuery, TransferLogResponse,
};

/// Handler for `POST /api/rtx/publish`.
pub async fn publish_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(identity): Extension<IdentityContext>,
    Json(bundle): Json<annex_rtx::ReflectionSummaryBundle>,
) -> Result<Json<PublishResponse>, ApiError> {
    let svc = RtxService::new(state);
    let resp = svc.publish_bundle(&identity, bundle).await?;
    Ok(Json(resp))
}

/// Handler for `POST /api/rtx/subscribe`.
pub async fn subscribe_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(identity): Extension<IdentityContext>,
    Json(req): Json<SubscribeRequest>,
) -> Result<Json<SubscribeResponse>, ApiError> {
    let svc = RtxService::new(state);
    let info = svc.subscribe(&identity, req).await?;
    Ok(Json(SubscribeResponse {
        ok: true,
        subscription: Some(info),
    }))
}

/// Handler for `DELETE /api/rtx/subscribe`.
pub async fn unsubscribe_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(identity): Extension<IdentityContext>,
) -> Result<Json<SubscribeResponse>, ApiError> {
    let svc = RtxService::new(state);
    svc.unsubscribe(&identity).await?;
    Ok(Json(SubscribeResponse {
        ok: true,
        subscription: None,
    }))
}

/// Handler for `GET /api/rtx/subscriptions`.
pub async fn get_subscription_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(identity): Extension<IdentityContext>,
) -> Result<Json<SubscribeResponse>, ApiError> {
    let svc = RtxService::new(state);
    let info = svc.get_subscription(&identity).await?;
    Ok(Json(SubscribeResponse {
        ok: true,
        subscription: info,
    }))
}

/// Handler for `GET /api/rtx/governance/transfers`. Operator-only:
/// requires `can_moderate`. The capability check is preserved here
/// (the service does not re-check) so the moderator gate is the first
/// thing the request hits.
pub async fn governance_transfers_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Query(query): Query<TransferLogQuery>,
) -> Result<Json<TransferLogResponse>, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "governance endpoints require can_moderate permission".to_string(),
        ));
    }
    let svc = RtxService::new(state);
    let resp = svc.governance_transfers(query).await?;
    Ok(Json(resp))
}

/// Handler for `GET /api/rtx/governance/summary`. Operator-only:
/// requires `can_moderate`.
pub async fn governance_summary_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Json<GovernanceSummaryResponse>, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "governance endpoints require can_moderate permission".to_string(),
        ));
    }
    let svc = RtxService::new(state);
    let resp = svc.governance_summary().await?;
    Ok(Json(resp))
}
