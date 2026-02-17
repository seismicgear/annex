//! RTX (Reflection Transfer Exchange) API handlers.
//!
//! Implements `POST /api/rtx/publish` — the endpoint that accepts a
//! `ReflectionSummaryBundle` from an authenticated agent, validates it
//! against the sender's VRP registration and transfer scope, stores it,
//! and delivers it to matching subscribers.

use crate::api::ApiError;
use crate::middleware::IdentityContext;
use crate::AppState;
use annex_rtx::{check_redacted_topics, enforce_transfer_scope, validate_bundle_structure};
use annex_vrp::VrpTransferScope;
use axum::{extract::Extension, Json};
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
            let conn = state.pool.get().map_err(|e| {
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

            // 8. Store bundle in DB
            let domain_tags_json =
                serde_json::to_string(&stored_bundle.domain_tags).map_err(|e| {
                    ApiError::InternalServerError(format!("json serialization failed: {}", e))
                })?;
            let caveats_json = serde_json::to_string(&stored_bundle.caveats).map_err(|e| {
                ApiError::InternalServerError(format!("json serialization failed: {}", e))
            })?;

            conn.execute(
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

            // 9. Log the publish operation
            let redactions =
                if stored_bundle.reasoning_chain.is_none() && bundle.reasoning_chain.is_some() {
                    Some("reasoning_chain_stripped")
                } else {
                    None
                };

            conn.execute(
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
            .map_err(|e| ApiError::InternalServerError(format!("failed to log transfer: {}", e)))?;

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

                // Parse domain filters
                let domain_filters: Vec<String> =
                    serde_json::from_str(&domain_filters_json).unwrap_or_default();

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

                    let _ = conn.execute(
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
                    );

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
                serde_json::from_str(&filters_back).unwrap_or_default();

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
                    let domain_filters: Vec<String> =
                        serde_json::from_str(&filters_json).unwrap_or_default();
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
    match s {
        "FULL_KNOWLEDGE_BUNDLE" => Some(VrpTransferScope::FullKnowledgeBundle),
        "REFLECTION_SUMMARIES_ONLY" => Some(VrpTransferScope::ReflectionSummariesOnly),
        "NO_TRANSFER" => Some(VrpTransferScope::NoTransfer),
        _ => None,
    }
}

/// Extracts redacted topics from a capability contract JSON string.
///
/// The `redacted_topics` field may or may not be present in the stored JSON
/// (backward compatibility with contracts created before this field existed).
fn extract_redacted_topics(contract_json: &str) -> Vec<String> {
    serde_json::from_str::<annex_vrp::VrpCapabilitySharingContract>(contract_json)
        .map(|c| c.redacted_topics)
        .unwrap_or_default()
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
        let topics = extract_redacted_topics("not json");
        assert!(topics.is_empty());
    }
}
