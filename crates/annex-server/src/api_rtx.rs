//! RTX (Reflection Transfer Exchange) API handlers.
//!
//! Implements `POST /api/rtx/publish` — the endpoint that accepts a
//! `ReflectionSummaryBundle` from an authenticated agent, validates it
//! against the sender's VRP registration and transfer scope, stores it,
//! and delivers it to matching subscribers.

use crate::api::ApiError;
use crate::middleware::IdentityContext;
use crate::AppState;
use annex_federation::FederatedRtxEnvelope;
use annex_rtx::{
    check_redacted_topics, enforce_transfer_scope, validate_bundle_structure, BundleProvenance,
    ReflectionSummaryBundle,
};
use annex_vrp::VrpTransferScope;
use axum::{
    extract::{Extension, Query},
    Json,
};
use ed25519_dalek::Signer;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Response returned after a successful bundle publish.
#[derive(Debug, Serialize)]
pub struct PublishResponse {
    pub ok: bool,
    #[serde(rename = "bundleId")]
    pub bundle_id: String,
    pub delivered_to: usize,
}

/// Handler for `POST /api/rtx/publish`.
///
/// Accepts a `ReflectionSummaryBundle`, validates the sender's agent
/// registration and transfer scope, enforces redacted topics, stores
/// the bundle, and delivers it to matching local subscribers.
pub async fn publish_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Json(bundle): Json<annex_rtx::ReflectionSummaryBundle>,
) -> Result<Json<PublishResponse>, ApiError> {
    // 1. Validate bundle structure
    validate_bundle_structure(&bundle).map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // 2. Verify sender matches bundle source_pseudonym
    if identity.pseudonym_id != bundle.source_pseudonym {
        return Err(ApiError::Forbidden(
            "bundle source_pseudonym does not match authenticated identity".to_string(),
        ));
    }

    // 3. Verify source_server matches this server
    if bundle.source_server != state.public_url {
        return Err(ApiError::BadRequest(format!(
            "source_server '{}' does not match this server '{}'",
            bundle.source_server, state.public_url,
        )));
    }

    let bundle_id = bundle.bundle_id.clone();

    let (delivered_count, deliveries) = tokio::task::spawn_blocking({
        let state = state.clone();
        let bundle = bundle.clone();
        move || -> Result<(usize, Vec<(String, String)>), ApiError> {
            let mut conn = state.pool.get().map_err(|e| {
                ApiError::InternalServerError(format!("db connection failed: {}", e))
            })?;

            // 4. Check sender has an active agent registration with sufficient transfer scope
            let (transfer_scope_str, capability_contract_json): (String, String) = conn
                .query_row(
                    "SELECT transfer_scope, capability_contract_json
                     FROM agent_registrations
                     WHERE server_id = ?1 AND pseudonym_id = ?2 AND active = 1",
                    rusqlite::params![state.server_id, bundle.source_pseudonym],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => ApiError::Forbidden(format!(
                        "sender '{}' does not have an active agent registration",
                        bundle.source_pseudonym
                    )),
                    _ => ApiError::InternalServerError(format!("db query failed: {}", e)),
                })?;

            // 5. Parse and validate transfer scope
            let sender_scope = parse_transfer_scope(&transfer_scope_str).ok_or_else(|| {
                ApiError::Forbidden(
                    "sender's transfer scope does not permit RTX publishing".to_string(),
                )
            })?;

            if sender_scope < VrpTransferScope::ReflectionSummariesOnly {
                return Err(ApiError::Forbidden(
                    "sender's transfer scope does not permit RTX publishing".to_string(),
                ));
            }

            // 6. Extract redacted topics from capability contract and enforce
            let redacted_topics = extract_redacted_topics(&capability_contract_json);
            check_redacted_topics(&bundle, &redacted_topics)
                .map_err(|e| ApiError::Forbidden(e.to_string()))?;

            // 7. Apply sender's transfer scope (strips reasoning_chain if scope is ReflectionSummariesOnly)
            let stored_bundle = enforce_transfer_scope(&bundle, sender_scope)
                .map_err(|e| ApiError::Forbidden(e.to_string()))?;

            // 8-9. Store bundle + log initial transfer atomically in a transaction.
            //      If either fails, neither is persisted.
            let domain_tags_json =
                serde_json::to_string(&stored_bundle.domain_tags).map_err(|e| {
                    ApiError::InternalServerError(format!("json serialization failed: {}", e))
                })?;
            let caveats_json = serde_json::to_string(&stored_bundle.caveats).map_err(|e| {
                ApiError::InternalServerError(format!("json serialization failed: {}", e))
            })?;

            let redactions =
                if stored_bundle.reasoning_chain.is_none() && bundle.reasoning_chain.is_some() {
                    Some("reasoning_chain_stripped")
                } else {
                    None
                };

            {
                let tx = conn.transaction().map_err(|e| {
                    ApiError::InternalServerError(format!("failed to begin transaction: {}", e))
                })?;

                tx.execute(
                    "INSERT INTO rtx_bundles (
                        server_id, bundle_id, source_pseudonym, source_server,
                        domain_tags_json, summary, reasoning_chain, caveats_json,
                        created_at_ms, signature, vrp_handshake_ref
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    rusqlite::params![
                        state.server_id,
                        stored_bundle.bundle_id,
                        stored_bundle.source_pseudonym,
                        stored_bundle.source_server,
                        domain_tags_json,
                        stored_bundle.summary,
                        stored_bundle.reasoning_chain,
                        caveats_json,
                        stored_bundle.created_at as i64,
                        stored_bundle.signature,
                        stored_bundle.vrp_handshake_ref,
                    ],
                )
                .map_err(|e| {
                    if let rusqlite::Error::SqliteFailure(ref err, _) = e {
                        if err.code == rusqlite::ErrorCode::ConstraintViolation {
                            return ApiError::Conflict(format!(
                                "bundle '{}' already published",
                                stored_bundle.bundle_id
                            ));
                        }
                    }
                    ApiError::InternalServerError(format!("failed to store bundle: {}", e))
                })?;

                tx.execute(
                    "INSERT INTO rtx_transfer_log (
                        server_id, bundle_id, source_pseudonym, destination_pseudonym,
                        transfer_scope_applied, redactions_applied
                    ) VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
                    rusqlite::params![
                        state.server_id,
                        stored_bundle.bundle_id,
                        stored_bundle.source_pseudonym,
                        sender_scope.to_string(),
                        redactions,
                    ],
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("failed to log transfer: {}", e))
                })?;

                tx.commit().map_err(|e| {
                    ApiError::InternalServerError(format!("failed to commit transaction: {}", e))
                })?;
            }

            // 10. Find matching subscribers and prepare deliveries
            let mut deliveries: Vec<(String, String)> = Vec::new();

            let mut stmt = conn
                .prepare(
                    "SELECT s.subscriber_pseudonym, s.domain_filters_json, a.transfer_scope
                     FROM rtx_subscriptions s
                     JOIN agent_registrations a
                       ON a.server_id = s.server_id AND a.pseudonym_id = s.subscriber_pseudonym
                     WHERE s.server_id = ?1 AND a.active = 1
                       AND s.subscriber_pseudonym != ?2",
                )
                .map_err(|e| ApiError::InternalServerError(format!("db prepare failed: {}", e)))?;

            let rows = stmt
                .query_map(
                    rusqlite::params![state.server_id, bundle.source_pseudonym],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .map_err(|e| ApiError::InternalServerError(format!("db query failed: {}", e)))?;

            for row in rows {
                let (sub_pseudonym, domain_filters_json, scope_str) =
                    row.map_err(|e| ApiError::InternalServerError(format!("db row error: {}", e)))?;

                // Parse domain filters (empty = accept all; log if corrupted)
                let domain_filters: Vec<String> =
                    serde_json::from_str(&domain_filters_json).unwrap_or_else(|e| {
                        tracing::warn!(
                            subscriber = %sub_pseudonym,
                            "corrupted domain_filters_json in rtx subscription, defaulting to accept-all: {}",
                            e
                        );
                        Vec::new()
                    });

                // Check domain tag match (empty filters = accept all)
                let matches = domain_filters.is_empty()
                    || stored_bundle
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
                let scoped = match enforce_transfer_scope(&stored_bundle, receiver_scope) {
                    Ok(b) => b,
                    Err(_) => continue,
                };

                let payload = serde_json::json!({
                    "type": "rtx_bundle",
                    "bundle": scoped,
                });

                if let Ok(json) = serde_json::to_string(&payload) {
                    // Log delivery
                    let delivery_redactions = if receiver_scope
                        == VrpTransferScope::ReflectionSummariesOnly
                        && stored_bundle.reasoning_chain.is_some()
                    {
                        Some("reasoning_chain_stripped")
                    } else {
                        None
                    };

                    if let Err(e) = conn.execute(
                        "INSERT INTO rtx_transfer_log (
                            server_id, bundle_id, source_pseudonym, destination_pseudonym,
                            transfer_scope_applied, redactions_applied
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![
                            state.server_id,
                            stored_bundle.bundle_id,
                            stored_bundle.source_pseudonym,
                            sub_pseudonym,
                            receiver_scope.to_string(),
                            delivery_redactions,
                        ],
                    ) {
                        tracing::warn!(
                            bundle_id = %stored_bundle.bundle_id,
                            destination = %sub_pseudonym,
                            "failed to write rtx transfer log: {}",
                            e
                        );
                    }

                    deliveries.push((sub_pseudonym, json));
                }
            }

            let count = deliveries.len();
            Ok((count, deliveries))
        }
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    // 11. Deliver via WebSocket (async, outside spawn_blocking)
    for (pseudonym, json) in &deliveries {
        state.connection_manager.send(pseudonym, json.clone()).await;
    }

    // 12. Relay to federated peers (background task — does not block response)
    tokio::spawn(relay_rtx_bundles(state.clone(), bundle));

    Ok(Json(PublishResponse {
        ok: true,
        bundle_id,
        delivered_to: delivered_count,
    }))
}

/// Request body for `POST /api/rtx/subscribe`.
#[derive(Debug, Deserialize)]
pub struct SubscribeRequest {
    /// Domain tags to filter incoming bundles (empty = accept all).
    #[serde(default)]
    pub domain_filters: Vec<String>,
    /// Whether to accept bundles relayed from federated servers.
    #[serde(default)]
    pub accept_federated: bool,
}

/// Response from subscribe/unsubscribe operations.
#[derive(Debug, Serialize)]
pub struct SubscribeResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription: Option<SubscriptionInfo>,
}

/// Serialized representation of an RTX subscription.
#[derive(Debug, Serialize)]
pub struct SubscriptionInfo {
    pub subscriber_pseudonym: String,
    pub domain_filters: Vec<String>,
    pub accept_federated: bool,
    pub created_at: String,
}

/// Handler for `POST /api/rtx/subscribe`.
///
/// Creates or updates an RTX subscription for the authenticated agent.
/// The agent must have an active registration with transfer scope
/// `>= ReflectionSummariesOnly` to subscribe.
pub async fn subscribe_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Json(req): Json<SubscribeRequest>,
) -> Result<Json<SubscribeResponse>, ApiError> {
    let pseudonym = identity.pseudonym_id.clone();

    let info = tokio::task::spawn_blocking({
        let state = state.clone();
        let pseudonym = pseudonym.clone();
        let domain_filters = req.domain_filters.clone();
        let accept_federated = req.accept_federated;
        move || -> Result<SubscriptionInfo, ApiError> {
            let conn = state.pool.get().map_err(|e| {
                ApiError::InternalServerError(format!("db connection failed: {}", e))
            })?;

            // 1. Verify agent has active registration with sufficient scope
            let scope_str: String = conn
                .query_row(
                    "SELECT transfer_scope FROM agent_registrations
                     WHERE server_id = ?1 AND pseudonym_id = ?2 AND active = 1",
                    rusqlite::params![state.server_id, pseudonym],
                    |row| row.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => ApiError::Forbidden(format!(
                        "agent '{}' does not have an active registration",
                        pseudonym
                    )),
                    _ => ApiError::InternalServerError(format!("db query failed: {}", e)),
                })?;

            let scope = parse_transfer_scope(&scope_str).ok_or_else(|| {
                ApiError::Forbidden(
                    "agent's transfer scope does not permit RTX subscriptions".into(),
                )
            })?;

            if scope < VrpTransferScope::ReflectionSummariesOnly {
                return Err(ApiError::Forbidden(
                    "agent's transfer scope does not permit RTX subscriptions".to_string(),
                ));
            }

            // 2. UPSERT subscription
            let filters_json = serde_json::to_string(&domain_filters).map_err(|e| {
                ApiError::InternalServerError(format!("json serialization failed: {}", e))
            })?;
            let accept_fed_int: i32 = if accept_federated { 1 } else { 0 };

            conn.execute(
                "INSERT INTO rtx_subscriptions (
                    server_id, subscriber_pseudonym, domain_filters_json, accept_federated
                ) VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(server_id, subscriber_pseudonym) DO UPDATE SET
                    domain_filters_json = excluded.domain_filters_json,
                    accept_federated = excluded.accept_federated",
                rusqlite::params![state.server_id, pseudonym, filters_json, accept_fed_int],
            )
            .map_err(|e| {
                ApiError::InternalServerError(format!("failed to create subscription: {}", e))
            })?;

            // 3. Read back for response
            let (filters_back, fed_back, created_at): (String, bool, String) = conn
                .query_row(
                    "SELECT domain_filters_json, accept_federated, created_at
                     FROM rtx_subscriptions
                     WHERE server_id = ?1 AND subscriber_pseudonym = ?2",
                    rusqlite::params![state.server_id, pseudonym],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("failed to read subscription: {}", e))
                })?;

            let parsed_filters: Vec<String> =
                serde_json::from_str(&filters_back).unwrap_or_else(|e| {
                    tracing::warn!(
                        subscriber = %pseudonym,
                        "corrupted domain_filters_json in subscription read-back: {}",
                        e
                    );
                    Vec::new()
                });

            Ok(SubscriptionInfo {
                subscriber_pseudonym: pseudonym,
                domain_filters: parsed_filters,
                accept_federated: fed_back,
                created_at,
            })
        }
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(SubscribeResponse {
        ok: true,
        subscription: Some(info),
    }))
}

/// Handler for `DELETE /api/rtx/subscribe`.
///
/// Removes the authenticated agent's RTX subscription.
pub async fn unsubscribe_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Json<SubscribeResponse>, ApiError> {
    let pseudonym = identity.pseudonym_id.clone();

    tokio::task::spawn_blocking({
        let state = state.clone();
        move || -> Result<(), ApiError> {
            let conn = state.pool.get().map_err(|e| {
                ApiError::InternalServerError(format!("db connection failed: {}", e))
            })?;

            let deleted = conn
                .execute(
                    "DELETE FROM rtx_subscriptions
                     WHERE server_id = ?1 AND subscriber_pseudonym = ?2",
                    rusqlite::params![state.server_id, pseudonym],
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("failed to delete subscription: {}", e))
                })?;

            if deleted == 0 {
                return Err(ApiError::NotFound("no active RTX subscription".to_string()));
            }

            Ok(())
        }
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(SubscribeResponse {
        ok: true,
        subscription: None,
    }))
}

/// Handler for `GET /api/rtx/subscriptions`.
///
/// Returns the authenticated agent's current RTX subscription, if any.
pub async fn get_subscription_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Json<SubscribeResponse>, ApiError> {
    let pseudonym = identity.pseudonym_id.clone();

    let info = tokio::task::spawn_blocking({
        let state = state.clone();
        move || -> Result<Option<SubscriptionInfo>, ApiError> {
            let conn = state.pool.get().map_err(|e| {
                ApiError::InternalServerError(format!("db connection failed: {}", e))
            })?;

            let result = conn
                .query_row(
                    "SELECT domain_filters_json, accept_federated, created_at
                     FROM rtx_subscriptions
                     WHERE server_id = ?1 AND subscriber_pseudonym = ?2",
                    rusqlite::params![state.server_id, pseudonym],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, bool>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(|e| ApiError::InternalServerError(format!("db query failed: {}", e)))?;

            match result {
                Some((filters_json, accept_federated, created_at)) => {
                    let domain_filters: Vec<String> = serde_json::from_str(&filters_json)
                        .unwrap_or_else(|e| {
                            tracing::warn!(
                                subscriber = %pseudonym,
                                "corrupted domain_filters_json in subscription query: {}",
                                e
                            );
                            Vec::new()
                        });
                    Ok(Some(SubscriptionInfo {
                        subscriber_pseudonym: pseudonym,
                        domain_filters,
                        accept_federated,
                        created_at,
                    }))
                }
                None => Ok(None),
            }
        }
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(SubscribeResponse {
        ok: true,
        subscription: info,
    }))
}

/// Parses a transfer scope string from the database.
fn parse_transfer_scope(s: &str) -> Option<VrpTransferScope> {
    s.parse().ok()
}

/// Extracts redacted topics from a capability contract JSON string.
///
/// The `redacted_topics` field may or may not be present in the stored JSON
/// (backward compatibility with contracts created before this field existed).
/// If the JSON is entirely unparseable (data corruption), logs a warning and
/// returns an empty list. This is a fail-open decision: corrupted contracts
/// lose their redaction restrictions. Operators should monitor for this warning
/// and repair the underlying data.
fn extract_redacted_topics(contract_json: &str) -> Vec<String> {
    serde_json::from_str::<annex_vrp::VrpCapabilitySharingContract>(contract_json)
        .map(|c| c.redacted_topics)
        .unwrap_or_else(|e| {
            tracing::warn!(
                "corrupted capability contract JSON, redacted topics unavailable: {}",
                e
            );
            Vec::new()
        })
}

/// Constructs the deterministic signing payload for an RTX relay envelope.
///
/// The signed payload uses newline delimiters between fields to prevent
/// ambiguity where field boundaries overlap (e.g., `"ab" + "c"` vs `"a" + "bc"`).
/// Relay path entries are joined with `|` separators within their field.
pub fn rtx_relay_signing_payload(
    bundle_id: &str,
    relaying_server: &str,
    origin_server: &str,
    relay_path: &[String],
) -> String {
    let relay_path_joined = relay_path.join("|");
    format!(
        "{}\n{}\n{}\n{}",
        bundle_id, relaying_server, origin_server, relay_path_joined
    )
}

/// Relays an RTX bundle to all active federation peers.
///
/// This function is intended to be spawned as a background task after a local
/// publish. For each federation peer with an active agreement and sufficient
/// transfer scope, it constructs a `FederatedRtxEnvelope`, signs it with the
/// server's Ed25519 key, and POSTs it to the peer's `/api/federation/rtx`
/// endpoint.
///
/// Transfer scope enforcement:
/// - `NoTransfer` peers are skipped entirely.
/// - `ReflectionSummariesOnly` peers receive bundles with `reasoning_chain` stripped.
/// - `FullKnowledgeBundle` peers receive the full bundle.
///
/// The provenance chain tracks the original source server and all relay hops.
pub async fn relay_rtx_bundles(state: Arc<AppState>, bundle: ReflectionSummaryBundle) {
    let peers = tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let server_id = state.server_id;
        move || -> Result<Vec<(String, String)>, String> {
            let conn = pool.get().map_err(|e| e.to_string())?;

            let mut stmt = conn
                .prepare(
                    "SELECT i.base_url, fa.transfer_scope
                     FROM federation_agreements fa
                     JOIN instances i ON fa.remote_instance_id = i.id
                     WHERE fa.local_server_id = ?1 AND fa.active = 1 AND i.status = 'ACTIVE'",
                )
                .map_err(|e| e.to_string())?;

            let rows = stmt
                .query_map(rusqlite::params![server_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?;

            let mut peers = Vec::new();
            for row in rows {
                peers.push(row.map_err(|e| e.to_string())?);
            }

            Ok(peers)
        }
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));

    let peers = match peers {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("Failed to fetch federation peers for RTX relay: {}", e);
            return;
        }
    };

    if peers.is_empty() {
        return;
    }

    let client = match crate::api_federation::federation_http_client() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to build federation HTTP client for RTX relay: {}", e);
            return;
        }
    };

    for (base_url, transfer_scope_str) in peers {
        let scope = match parse_transfer_scope(&transfer_scope_str) {
            Some(s) if s >= VrpTransferScope::ReflectionSummariesOnly => s,
            _ => continue, // Skip NoTransfer or unknown peers
        };

        // Apply federation transfer scope to the bundle
        let scoped_bundle = match enforce_transfer_scope(&bundle, scope) {
            Ok(b) => b,
            Err(_) => continue,
        };

        // Build provenance (this server is the first relay hop)
        let provenance = BundleProvenance {
            origin_server: bundle.source_server.clone(),
            relay_path: vec![state.public_url.clone()],
            bundle_id: bundle.bundle_id.clone(),
        };

        // Sign the relay envelope
        let signing_payload = rtx_relay_signing_payload(
            &bundle.bundle_id,
            &state.public_url,
            &bundle.source_server,
            &provenance.relay_path,
        );
        let signature = state.signing_key.sign(signing_payload.as_bytes());
        let signature_hex = hex::encode(signature.to_bytes());

        let envelope = FederatedRtxEnvelope {
            bundle: scoped_bundle,
            provenance,
            relaying_server: state.public_url.clone(),
            signature: signature_hex,
        };

        let url = format!("{}/api/federation/rtx", base_url);
        let client_clone = client.clone();

        tokio::spawn(async move {
            match client_clone.post(&url).json(&envelope).send().await {
                Ok(resp) if !resp.status().is_success() => {
                    tracing::warn!("RTX relay to {} returned status {}", url, resp.status());
                }
                Err(e) => {
                    tracing::warn!("Failed to relay RTX bundle to {}: {}", url, e);
                }
                Ok(_) => {
                    tracing::debug!(
                        "RTX bundle {} relayed to {}",
                        envelope.bundle.bundle_id,
                        url
                    );
                }
            }
        });
    }
}

// ===========================================================================
// Governance Mediation — Auditable transfer log queries (Step 9.5)
// ===========================================================================

/// Query parameters for `GET /api/rtx/governance/transfers`.
#[derive(Debug, Deserialize)]
pub struct TransferLogQuery {
    /// Filter by bundle_id.
    pub bundle_id: Option<String>,
    /// Filter by source pseudonym.
    pub source: Option<String>,
    /// Filter by destination pseudonym.
    pub destination: Option<String>,
    /// Filter transfers at or after this ISO 8601 timestamp.
    pub since: Option<String>,
    /// Filter transfers at or before this ISO 8601 timestamp.
    pub until: Option<String>,
    /// Maximum number of results to return (default 50, max 500).
    pub limit: Option<u32>,
    /// Number of results to skip (for pagination).
    pub offset: Option<u32>,
}

/// A single entry from the RTX transfer log.
#[derive(Debug, Serialize)]
pub struct TransferLogEntry {
    pub id: i64,
    pub bundle_id: String,
    pub source_pseudonym: String,
    pub destination_pseudonym: Option<String>,
    pub transfer_scope_applied: String,
    pub redactions_applied: Option<String>,
    pub transferred_at: String,
}

/// Response for `GET /api/rtx/governance/transfers`.
#[derive(Debug, Serialize)]
pub struct TransferLogResponse {
    pub transfers: Vec<TransferLogEntry>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

/// Handler for `GET /api/rtx/governance/transfers`.
///
/// Returns a paginated, filterable view of the RTX transfer log.
/// Requires `can_moderate` permission (server operator access).
pub async fn governance_transfers_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Query(query): Query<TransferLogQuery>,
) -> Result<Json<TransferLogResponse>, ApiError> {
    // Operator-only: require can_moderate
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "governance endpoints require can_moderate permission".to_string(),
        ));
    }

    let limit = query.limit.unwrap_or(50).min(500);
    let offset = query.offset.unwrap_or(0);

    let result = tokio::task::spawn_blocking({
        let state = state.clone();
        move || -> Result<TransferLogResponse, ApiError> {
            let conn = state.pool.get().map_err(|e| {
                ApiError::InternalServerError(format!("db connection failed: {}", e))
            })?;

            // Build dynamic WHERE clause
            let mut conditions = vec!["server_id = ?1".to_string()];
            let mut param_idx = 2u32;
            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(state.server_id)];

            if let Some(ref bid) = query.bundle_id {
                conditions.push(format!("bundle_id = ?{}", param_idx));
                params.push(Box::new(bid.clone()));
                param_idx += 1;
            }
            if let Some(ref src) = query.source {
                conditions.push(format!("source_pseudonym = ?{}", param_idx));
                params.push(Box::new(src.clone()));
                param_idx += 1;
            }
            if let Some(ref dst) = query.destination {
                conditions.push(format!("destination_pseudonym = ?{}", param_idx));
                params.push(Box::new(dst.clone()));
                param_idx += 1;
            }
            if let Some(ref since) = query.since {
                conditions.push(format!("transferred_at >= ?{}", param_idx));
                params.push(Box::new(since.clone()));
                param_idx += 1;
            }
            if let Some(ref until) = query.until {
                conditions.push(format!("transferred_at <= ?{}", param_idx));
                params.push(Box::new(until.clone()));
                param_idx += 1;
            }

            let where_clause = conditions.join(" AND ");

            // Count total matching entries
            let count_sql = format!(
                "SELECT COUNT(*) FROM rtx_transfer_log WHERE {}",
                where_clause
            );
            let total: i64 = conn
                .query_row(
                    &count_sql,
                    rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                    |row| row.get(0),
                )
                .map_err(|e| ApiError::InternalServerError(format!("count query failed: {}", e)))?;

            // Fetch paginated results
            let data_sql = format!(
                "SELECT id, bundle_id, source_pseudonym, destination_pseudonym,
                        transfer_scope_applied, redactions_applied, transferred_at
                 FROM rtx_transfer_log
                 WHERE {}
                 ORDER BY id DESC
                 LIMIT ?{} OFFSET ?{}",
                where_clause,
                param_idx,
                param_idx + 1,
            );

            params.push(Box::new(limit));
            params.push(Box::new(offset));

            let mut stmt = conn.prepare(&data_sql).map_err(|e| {
                ApiError::InternalServerError(format!("prepare query failed: {}", e))
            })?;

            let rows = stmt
                .query_map(
                    rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                    |row| {
                        Ok(TransferLogEntry {
                            id: row.get(0)?,
                            bundle_id: row.get(1)?,
                            source_pseudonym: row.get(2)?,
                            destination_pseudonym: row.get(3)?,
                            transfer_scope_applied: row.get(4)?,
                            redactions_applied: row.get(5)?,
                            transferred_at: row.get(6)?,
                        })
                    },
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("transfer log query failed: {}", e))
                })?;

            let mut transfers = Vec::new();
            for row in rows {
                transfers.push(row.map_err(|e| {
                    ApiError::InternalServerError(format!("row read error: {}", e))
                })?);
            }

            Ok(TransferLogResponse {
                transfers,
                total,
                limit,
                offset,
            })
        }
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(result))
}

/// Breakdown of transfers by scope.
#[derive(Debug, Serialize)]
pub struct ScopeBreakdown {
    pub scope: String,
    pub count: i64,
}

/// Response for `GET /api/rtx/governance/summary`.
#[derive(Debug, Serialize)]
pub struct GovernanceSummaryResponse {
    /// Total number of transfer log entries on this server.
    pub total_transfers: i64,
    /// Count of distinct bundle IDs.
    pub unique_bundles: i64,
    /// Count of distinct source pseudonyms.
    pub unique_sources: i64,
    /// Count of distinct destination pseudonyms (excluding NULL for publishes).
    pub unique_destinations: i64,
    /// Count of transfers where redactions were applied.
    pub redacted_transfers: i64,
    /// Breakdown by transfer scope.
    pub by_scope: Vec<ScopeBreakdown>,
}

/// Handler for `GET /api/rtx/governance/summary`.
///
/// Returns aggregate statistics about RTX transfers for operator auditing.
/// Requires `can_moderate` permission (server operator access).
pub async fn governance_summary_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Json<GovernanceSummaryResponse>, ApiError> {
    // Operator-only: require can_moderate
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "governance endpoints require can_moderate permission".to_string(),
        ));
    }

    let result = tokio::task::spawn_blocking({
        let state = state.clone();
        move || -> Result<GovernanceSummaryResponse, ApiError> {
            let conn = state.pool.get().map_err(|e| {
                ApiError::InternalServerError(format!("db connection failed: {}", e))
            })?;

            let server_id = state.server_id;

            let total_transfers: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM rtx_transfer_log WHERE server_id = ?1",
                    rusqlite::params![server_id],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("total count query failed: {}", e))
                })?;

            let unique_bundles: i64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT bundle_id) FROM rtx_transfer_log WHERE server_id = ?1",
                    rusqlite::params![server_id],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("unique bundles query failed: {}", e))
                })?;

            let unique_sources: i64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT source_pseudonym) FROM rtx_transfer_log WHERE server_id = ?1",
                    rusqlite::params![server_id],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("unique sources query failed: {}", e))
                })?;

            let unique_destinations: i64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT destination_pseudonym) FROM rtx_transfer_log
                     WHERE server_id = ?1 AND destination_pseudonym IS NOT NULL",
                    rusqlite::params![server_id],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("unique destinations query failed: {}", e))
                })?;

            let redacted_transfers: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM rtx_transfer_log
                     WHERE server_id = ?1 AND redactions_applied IS NOT NULL",
                    rusqlite::params![server_id],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("redacted count query failed: {}", e))
                })?;

            // Breakdown by transfer scope
            let mut stmt = conn
                .prepare(
                    "SELECT transfer_scope_applied, COUNT(*) as cnt
                     FROM rtx_transfer_log
                     WHERE server_id = ?1
                     GROUP BY transfer_scope_applied
                     ORDER BY cnt DESC",
                )
                .map_err(|e| {
                    ApiError::InternalServerError(format!("scope breakdown query failed: {}", e))
                })?;

            let scope_rows = stmt
                .query_map(rusqlite::params![server_id], |row| {
                    Ok(ScopeBreakdown {
                        scope: row.get(0)?,
                        count: row.get(1)?,
                    })
                })
                .map_err(|e| {
                    ApiError::InternalServerError(format!("scope breakdown read failed: {}", e))
                })?;

            let mut by_scope = Vec::new();
            for row in scope_rows {
                by_scope.push(row.map_err(|e| {
                    ApiError::InternalServerError(format!("scope row error: {}", e))
                })?);
            }

            Ok(GovernanceSummaryResponse {
                total_transfers,
                unique_bundles,
                unique_sources,
                unique_destinations,
                redacted_transfers,
                by_scope,
            })
        }
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {}", e)))??;

    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_transfer_scope() {
        assert_eq!(
            parse_transfer_scope("FULL_KNOWLEDGE_BUNDLE"),
            Some(VrpTransferScope::FullKnowledgeBundle)
        );
        assert_eq!(
            parse_transfer_scope("REFLECTION_SUMMARIES_ONLY"),
            Some(VrpTransferScope::ReflectionSummariesOnly)
        );
        assert_eq!(
            parse_transfer_scope("NO_TRANSFER"),
            Some(VrpTransferScope::NoTransfer)
        );
        assert_eq!(parse_transfer_scope("UNKNOWN"), None);
    }

    #[test]
    fn test_extract_redacted_topics_with_field() {
        let json = r#"{"required_capabilities":[],"offered_capabilities":[],"redacted_topics":["politics","finance"]}"#;
        let topics = extract_redacted_topics(json);
        assert_eq!(topics, vec!["politics", "finance"]);
    }

    #[test]
    fn test_extract_redacted_topics_without_field() {
        let json = r#"{"required_capabilities":[],"offered_capabilities":[]}"#;
        let topics = extract_redacted_topics(json);
        assert!(topics.is_empty());
    }

    #[test]
    fn test_extract_redacted_topics_invalid_json() {
        // Corrupted JSON returns empty vec (fail-open) and logs a warning.
        let topics = extract_redacted_topics("not json");
        assert!(topics.is_empty());
    }

    #[test]
    fn test_extract_redacted_topics_truncated_json() {
        // Simulates data corruption: truncated JSON string.
        let topics = extract_redacted_topics(r#"{"required_capabilities":["#);
        assert!(topics.is_empty());
    }

    #[test]
    fn test_extract_redacted_topics_wrong_type() {
        // JSON is valid but wrong shape — field is a string, not array.
        let topics = extract_redacted_topics(r#"{"redacted_topics": "not_an_array"}"#);
        assert!(topics.is_empty());
    }

    #[test]
    fn test_rtx_relay_signing_payload_deterministic() {
        let p1 = rtx_relay_signing_payload("b1", "relay", "origin", &["hop1".into()]);
        let p2 = rtx_relay_signing_payload("b1", "relay", "origin", &["hop1".into()]);
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_rtx_relay_signing_payload_multi_hop() {
        let payload = rtx_relay_signing_payload(
            "bundle-123",
            "http://relay.com",
            "http://origin.com",
            &["http://hop1.com".into(), "http://hop2.com".into()],
        );
        assert_eq!(
            payload,
            "bundle-123\nhttp://relay.com\nhttp://origin.com\nhttp://hop1.com|http://hop2.com"
        );
    }
}
