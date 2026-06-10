//! Admin API handlers for the Annex server.

use crate::{
    api::ApiError, config::derive_server_slug_from_public_url, middleware::IdentityContext,
    policy::recalculate_all_alignments, AppState,
};
use annex_identity::update_capabilities;
use annex_observe::EventPayload;
use annex_types::{Capabilities, ServerPolicy};
use axum::{
    extract::{Extension, Json, Path, Query},
    response::{IntoResponse, Response},
    Json as AxumJson,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Handler for `GET /api/admin/policy`.
///
/// Returns the current server policy. Requires `can_moderate` permission.
pub async fn get_policy_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to view policy".to_string(),
        ));
    }

    let policy = state
        .policy
        .read()
        .map_err(|_| ApiError::InternalServerError("policy lock poisoned".to_string()))?
        .clone();

    Ok(AxumJson(policy).into_response())
}

/// Handler for `PUT /api/admin/policy`.
///
/// Updates the server's policy, persists it to the database, logs the version,
/// and triggers re-evaluation of all agent and federation alignments.
///
/// Requires `can_moderate` permission.
pub async fn update_policy_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Json(new_policy): Json<ServerPolicy>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to update policy".to_string(),
        ));
    }

    // Validate access_mode to prevent typos from silently breaking access control.
    // An unrecognized value would be treated as "public" by the register handler,
    // potentially opening the server to unrestricted registrations.
    const VALID_ACCESS_MODES: &[&str] = &["public", "invite_only", "password"];
    if !VALID_ACCESS_MODES.contains(&new_policy.access_mode.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "invalid access_mode '{}'. Must be one of: {}",
            new_policy.access_mode,
            VALID_ACCESS_MODES.join(", ")
        )));
    }

    let version_id = Uuid::new_v4().to_string();
    let policy_json = serde_json::to_string(&new_policy)
        .map_err(|e| ApiError::BadRequest(format!("failed to serialize policy: {e}")))?;

    let state_clone = state.clone();
    let policy_clone = new_policy.clone();
    let version_id_clone = version_id.clone();
    let policy_json_clone = policy_json.clone();
    let moderator_pseudonym = identity.pseudonym_id.clone();

    tokio::task::spawn_blocking(move || {
        let mut conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {e}"))
        })?;

        let tx = conn.transaction().map_err(|e| {
            ApiError::InternalServerError(format!("failed to start transaction: {e}"))
        })?;

        tx.execute(
            "UPDATE servers SET policy_json = ?1 WHERE id = ?2",
            rusqlite::params![policy_json_clone, state_clone.server_id],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to update servers table: {e}")))?;

        tx.execute(
            "INSERT INTO server_policy_versions (server_id, version_id, policy_json) VALUES (?1, ?2, ?3)",
            rusqlite::params![state_clone.server_id, version_id_clone, policy_json_clone],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to insert policy version: {e}")))?;

        let observe_payload = EventPayload::ModerationAction {
            moderator_pseudonym: moderator_pseudonym.clone(),
            action_type: "policy_update".to_string(),
            target_pseudonym: None,
            description: format!("Server policy updated to version {version_id_clone}"),
        };
        crate::emit_and_broadcast(
            &tx,
            state_clone.server_id,
            &moderator_pseudonym,
            &observe_payload,
            &state_clone.observe_tx,
            &state_clone.signing_key,
        );

        tx.commit().map_err(|e| {
            ApiError::InternalServerError(format!("failed to commit transaction: {e}"))
        })?;

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    {
        let mut policy_lock = state
            .policy
            .write()
            .map_err(|_| ApiError::InternalServerError("policy lock poisoned".to_string()))?;
        *policy_lock = new_policy;
    }

    recalculate_all_alignments(state.clone()).await?;

    tracing::info!(
        version_id = %version_id,
        moderator = %identity.pseudonym_id,
        "server policy updated and alignments recalculated"
    );

    Ok(AxumJson(serde_json::json!({
        "status": "ok",
        "version_id": version_id,
        "policy": policy_clone
    }))
    .into_response())
}

/// Handler for `DELETE /api/admin/federation/:id`.
///
/// Revokes a federation agreement by ID, emitting a `FederationSevered` event.
/// Requires `can_moderate` permission.
pub async fn revoke_federation_handler(
    Path(agreement_id): Path<i64>,
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to revoke federation agreement".to_string(),
        ));
    }

    let state_clone = state.clone();
    let moderator = identity.pseudonym_id.clone();

    let remote_url = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        // Look up the remote instance base_url before revoking so we can emit the event.
        let remote_url: Option<String> = conn
            .query_row(
                "SELECT i.base_url FROM federation_agreements fa
                 JOIN instances i ON fa.remote_instance_id = i.id
                 WHERE fa.id = ?1 AND fa.local_server_id = ?2 AND fa.active = 1",
                rusqlite::params![agreement_id, state_clone.server_id],
                |row| row.get(0),
            )
            .ok();

        let revoked =
            annex_federation::revoke_agreement(&conn, agreement_id, state_clone.server_id)
                .map_err(|e| {
                    ApiError::InternalServerError(format!("failed to revoke agreement: {e}"))
                })?;

        if !revoked {
            return Err(ApiError::NotFound(
                "federation agreement not found or already revoked".to_string(),
            ));
        }

        // Emit FederationSevered event
        if let Some(ref url) = remote_url {
            let observe_payload = annex_observe::EventPayload::FederationSevered {
                remote_url: url.clone(),
                reason: format!("revoked by moderator {moderator}"),
            };
            crate::emit_and_broadcast(
                &conn,
                state_clone.server_id,
                &moderator,
                &observe_payload,
                &state_clone.observe_tx,
                &state_clone.signing_key,
            );
        }

        Ok::<Option<String>, ApiError>(remote_url)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({
        "status": "ok",
        "agreement_id": agreement_id,
        "remote_url": remote_url,
    }))
    .into_response())
}

// ── Server Settings ──

#[derive(Debug, Deserialize)]
pub struct UpdateServerRequest {
    pub label: Option<String>,
    pub description: Option<String>,
}

/// Handler for `PATCH /api/admin/server`.
pub async fn rename_server_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Json(body): Json<UpdateServerRequest>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to update server".to_string(),
        ));
    }

    let label = body.label.map(|l| {
        let trimmed = l.trim().to_string();
        trimmed
    });
    if let Some(ref l) = label {
        if l.is_empty() || l.len() > 128 {
            return Err(ApiError::BadRequest(
                "label must be 1–128 characters".to_string(),
            ));
        }
    }

    let description = body.description.map(|d| {
        let trimmed = d.trim().to_string();
        trimmed
    });
    if let Some(ref d) = description {
        if d.len() > 300 {
            return Err(ApiError::BadRequest(
                "description must be at most 300 characters".to_string(),
            ));
        }
    }

    if label.is_none() && description.is_none() {
        return Err(ApiError::BadRequest(
            "at least one of label or description must be provided".to_string(),
        ));
    }

    let state_clone = state.clone();
    let label_clone = label.clone();
    let description_clone = description.clone();
    let moderator = identity.pseudonym_id.clone();

    tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        if let Some(ref l) = label_clone {
            conn.execute(
                "UPDATE servers SET label = ?1 WHERE id = ?2",
                rusqlite::params![l, state_clone.server_id],
            )
            .map_err(|e| ApiError::InternalServerError(format!("failed to update label: {e}")))?;
        }

        if let Some(ref d) = description_clone {
            conn.execute(
                "UPDATE servers SET description = ?1 WHERE id = ?2",
                rusqlite::params![d, state_clone.server_id],
            )
            .map_err(|e| {
                ApiError::InternalServerError(format!("failed to update description: {e}"))
            })?;
        }

        let event_desc = match (&label_clone, &description_clone) {
            (Some(l), Some(_)) => format!("Server renamed to \"{l}\" and description updated"),
            (Some(l), None) => format!("Server renamed to \"{l}\""),
            (None, Some(_)) => "Server description updated".to_string(),
            (None, None) => unreachable!(),
        };

        let observe_payload = EventPayload::ModerationAction {
            moderator_pseudonym: moderator.clone(),
            action_type: "server_update".to_string(),
            target_pseudonym: None,
            description: event_desc,
        };
        crate::emit_and_broadcast(
            &conn,
            state_clone.server_id,
            &moderator,
            &observe_payload,
            &state_clone.observe_tx,
            &state_clone.signing_key,
        );

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    let mut resp = serde_json::json!({ "status": "ok" });
    if let Some(l) = label {
        resp["label"] = serde_json::Value::String(l);
    }
    if let Some(d) = description {
        resp["description"] = serde_json::Value::String(d);
    }
    Ok(AxumJson(resp).into_response())
}

/// Handler for `GET /api/admin/server`.
pub async fn get_server_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden("insufficient permissions".to_string()));
    }

    let state_clone = state.clone();
    let (slug, label, description) = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;
        conn.query_row(
            "SELECT slug, label, description FROM servers WHERE id = ?1",
            rusqlite::params![state_clone.server_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to read server: {e}")))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({
        "slug": slug,
        "label": label,
        "description": description,
        "public_url": state.get_public_url(),
    }))
    .into_response())
}

// ── Public URL ──

#[derive(Debug, Deserialize)]
pub struct SetPublicUrlRequest {
    pub public_url: String,
}

/// Handler for `PUT /api/admin/public-url`.
///
/// Allows an admin to explicitly set the server's public URL so that invite
/// links, federation handshakes, and relay paths use a globally-reachable
/// address instead of an auto-detected localhost.
pub async fn set_public_url_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Json(body): Json<SetPublicUrlRequest>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden("insufficient permissions".to_string()));
    }

    let url = body.public_url.trim().trim_end_matches('/').to_string();
    if !url.is_empty() && !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ApiError::BadRequest(
            "public_url must start with http:// or https://".to_string(),
        ));
    }

    // Persist to database so the URL survives server restarts
    let url_clone = url.clone();
    let next_slug = derive_server_slug_from_public_url(&url);
    let next_slug_clone = next_slug.clone();
    let state_clone = state.clone();
    tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;
        conn.execute(
            "UPDATE servers SET public_url = ?1, slug = ?2 WHERE id = ?3",
            rusqlite::params![url_clone, next_slug_clone, state_clone.server_id],
        )
        .map_err(|e| ApiError::InternalServerError(format!("failed to persist public_url: {e}")))?;
        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    // Update in-memory state
    {
        let mut current = state.public_url.write().unwrap_or_else(|p| p.into_inner());
        *current = url.clone();
    }

    tracing::info!(
        public_url = %url,
        server_slug = %next_slug,
        "public URL updated via admin API; server slug re-derived and persisted"
    );

    Ok(AxumJson(serde_json::json!({
        "status": "ok",
        "public_url": url,
        "server_slug": next_slug
    }))
    .into_response())
}

// ── WebRTC Public URL (runtime update for Tauri) ──

#[derive(Debug, Deserialize)]
pub struct SetWebrtcPublicUrlRequest {
    pub public_webrtc_url: String,
}

/// Handler for `PUT /api/admin/webrtc-public-url`.
///
/// Allows the Tauri host to push the router-provided public WebRTC URL
/// into the running server so remote voice join responses return a
/// globally-reachable URL instead of a loopback address.
pub async fn set_webrtc_public_url_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Json(body): Json<SetWebrtcPublicUrlRequest>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden("insufficient permissions".to_string()));
    }

    let url = body.public_webrtc_url.trim().to_string();
    if !url.is_empty()
        && !url.starts_with("ws://")
        && !url.starts_with("wss://")
        && !url.starts_with("http://")
        && !url.starts_with("https://")
    {
        return Err(ApiError::BadRequest(
            "public_webrtc_url must start with ws://, wss://, http://, or https://".to_string(),
        ));
    }

    state.voice_service.set_public_url(url.clone());

    tracing::info!(public_webrtc_url = %url, "WebRTC public URL updated via admin API");

    Ok(AxumJson(serde_json::json!({ "status": "ok", "public_webrtc_url": url })).into_response())
}

// ── Member Management ──

#[derive(Debug, Serialize)]
pub struct MemberInfo {
    pub pseudonym_id: String,
    pub participant_type: String,
    pub can_voice: bool,
    pub can_moderate: bool,
    pub can_invite: bool,
    pub can_federate: bool,
    pub can_bridge: bool,
    pub active: bool,
    pub created_at: String,
}

/// Handler for `GET /api/admin/members`.
pub async fn list_members_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to list members".to_string(),
        ));
    }

    let state_clone = state.clone();
    let members = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT pseudonym_id, participant_type, can_voice, can_moderate,
                        can_invite, can_federate, can_bridge, active, created_at
                 FROM platform_identities WHERE server_id = ?1
                 ORDER BY created_at ASC",
            )
            .map_err(|e| ApiError::InternalServerError(format!("query failed: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![state_clone.server_id], |row| {
                Ok(MemberInfo {
                    pseudonym_id: row.get(0)?,
                    participant_type: row.get(1)?,
                    can_voice: row.get(2)?,
                    can_moderate: row.get(3)?,
                    can_invite: row.get(4)?,
                    can_federate: row.get(5)?,
                    can_bridge: row.get(6)?,
                    active: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })
            .map_err(|e| ApiError::InternalServerError(format!("query failed: {e}")))?;

        let mut members = Vec::new();
        for row in rows {
            members
                .push(row.map_err(|e| ApiError::InternalServerError(format!("row error: {e}")))?);
        }
        Ok::<_, ApiError>(members)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({ "members": members })).into_response())
}

#[derive(Debug, Deserialize)]
pub struct UpdateCapabilitiesRequest {
    pub can_voice: bool,
    pub can_moderate: bool,
    pub can_invite: bool,
    pub can_federate: bool,
    pub can_bridge: bool,
}

/// Handler for `PATCH /api/admin/members/{pseudonymId}/capabilities`.
pub async fn update_member_capabilities_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(target_pseudonym): Path<String>,
    Json(body): Json<UpdateCapabilitiesRequest>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to update member capabilities".to_string(),
        ));
    }

    let caps = Capabilities {
        can_voice: body.can_voice,
        can_moderate: body.can_moderate,
        can_invite: body.can_invite,
        can_federate: body.can_federate,
        can_bridge: body.can_bridge,
    };

    let state_clone = state.clone();
    let target = target_pseudonym.clone();
    let moderator = identity.pseudonym_id.clone();

    tokio::task::spawn_blocking(move || {
        let conn = state_clone.pool.get().map_err(|e| {
            ApiError::InternalServerError(format!("db connection failed: {e}"))
        })?;

        update_capabilities(&conn, state_clone.server_id, &target, caps).map_err(|e| {
            ApiError::InternalServerError(format!("failed to update capabilities: {e}"))
        })?;

        let observe_payload = EventPayload::ModerationAction {
            moderator_pseudonym: moderator.clone(),
            action_type: "capabilities_update".to_string(),
            target_pseudonym: Some(target.clone()),
            description: format!(
                "Updated capabilities for {}: moderate={}, voice={}, invite={}, federate={}, bridge={}",
                target, caps.can_moderate, caps.can_voice, caps.can_invite, caps.can_federate, caps.can_bridge
            ),
        };
        crate::emit_and_broadcast(
            &conn,
            state_clone.server_id,
            &moderator,
            &observe_payload,
            &state_clone.observe_tx,
            &state_clone.signing_key,
        );

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({ "status": "ok" })).into_response())
}

// ── Storage health gate (ADR-0009) ──

/// Response body for `GET /api/admin/storage`.
#[derive(Debug, Serialize)]
pub struct StorageHealthResponse {
    /// Current gate state: `"healthy"`, `"warn"`, or `"degraded"`.
    pub state: String,
    /// Reason the gate left `healthy`. Empty while healthy.
    pub reason: String,
    /// True when mutating requests are being rejected with HTTP 507.
    pub writes_blocked: bool,
}

/// Handler for `GET /api/admin/storage`.
///
/// Returns the storage gate's current state, the recorded reason, and
/// whether writes are currently blocked. Requires `can_moderate`
/// permission.
pub async fn get_storage_health_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to view storage health".to_string(),
        ));
    }

    Ok(AxumJson(StorageHealthResponse {
        state: state.storage_health.state().as_str().to_string(),
        reason: state.storage_health.reason(),
        writes_blocked: state.storage_health.writes_blocked(),
    })
    .into_response())
}

/// Handler for `POST /api/admin/storage/clear`.
///
/// Clears a `warn` / `degraded` storage gate back to `healthy` after an
/// operator has verified the underlying condition (disk freed, volume
/// remounted, cap raised). The gate has no automatic recovery by design
/// (see `crate::storage_health`); before this endpoint existed the only
/// recovery path was a process restart.
///
/// This route is exempt from the auth middleware's degraded-gate 507
/// short-circuit (see `crate::middleware::auth_middleware`) — it must
/// remain reachable while the gate is closed, which is the only time it
/// is needed. The handler performs no SQLite writes on its critical
/// path: clearing the gate is an in-memory atomic store, and the audit
/// event emitted afterwards is best-effort (if the disk is still full,
/// the event write fails with a logged warning and the next failing
/// SQLite write re-trips the gate with a fresh reason).
///
/// Requires `can_moderate` permission.
pub async fn clear_storage_gate_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to clear storage gate".to_string(),
        ));
    }

    let previous = state.storage_health.state();
    let previous_reason = state.storage_health.reason();
    state.storage_health.mark_healthy();

    tracing::info!(
        previous_state = previous.as_str(),
        previous_reason = %previous_reason,
        moderator = %identity.pseudonym_id,
        "storage gate cleared via admin API"
    );

    let state_clone = state.clone();
    let moderator = identity.pseudonym_id.clone();
    let description = if previous_reason.is_empty() {
        format!("Storage gate cleared from '{}'", previous.as_str())
    } else {
        format!(
            "Storage gate cleared from '{}' (reason was: {previous_reason})",
            previous.as_str()
        )
    };
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(conn) = state_clone.pool.get() {
            let observe_payload = EventPayload::ModerationAction {
                moderator_pseudonym: moderator.clone(),
                action_type: "storage_gate_clear".to_string(),
                target_pseudonym: None,
                description,
            };
            crate::emit_and_broadcast(
                &conn,
                state_clone.server_id,
                &moderator,
                &observe_payload,
                &state_clone.observe_tx,
                &state_clone.signing_key,
            );
        }
    })
    .await;

    Ok(AxumJson(serde_json::json!({
        "status": "ok",
        "previous_state": previous.as_str(),
        "state": "healthy",
    }))
    .into_response())
}

// ── Federation outbox inspection / retry (ADR-0008) ──

/// Statuses a `federation_outbox` row can hold. Used to validate the
/// `status` query filter so a typo returns 400 instead of an empty list.
const OUTBOX_STATUSES: &[&str] = &["pending", "delivered", "failed", "paused"];

/// Query parameters for `GET /api/admin/federation/outbox`.
#[derive(Debug, Deserialize)]
pub struct OutboxListParams {
    /// Optional status filter (`pending` / `delivered` / `failed` / `paused`).
    pub status: Option<String>,
    /// Page size, clamped to 1..=200. Defaults to 50.
    pub limit: Option<i64>,
    /// Row offset for pagination. Defaults to 0.
    pub offset: Option<i64>,
}

/// One row of `GET /api/admin/federation/outbox`. The envelope JSON is
/// deliberately omitted — it can be large and contains nothing an
/// operator needs for queue triage; `envelope_bytes` conveys its size.
#[derive(Debug, Serialize)]
pub struct OutboxEntry {
    pub id: i64,
    pub peer_instance_id: i64,
    /// Base URL of the peer instance, when the instance row still exists.
    pub peer_base_url: Option<String>,
    /// Label of the peer instance, when the instance row still exists.
    pub peer_label: Option<String>,
    pub message_id: String,
    pub status: String,
    pub attempts: u32,
    pub next_retry_at: String,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Size of the serialized signed envelope in bytes.
    pub envelope_bytes: i64,
}

/// Handler for `GET /api/admin/federation/outbox`.
///
/// Lists federation outbox rows (most recent first) with optional
/// status filtering and pagination, plus aggregate counts by status so
/// an operator can see queue depth and stuck deliveries at a glance.
/// Requires `can_moderate` permission.
pub async fn list_federation_outbox_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Query(params): Query<OutboxListParams>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to inspect federation outbox".to_string(),
        ));
    }

    if let Some(ref s) = params.status {
        if !OUTBOX_STATUSES.contains(&s.as_str()) {
            return Err(ApiError::BadRequest(format!(
                "invalid status '{s}'. Must be one of: {}",
                OUTBOX_STATUSES.join(", ")
            )));
        }
    }
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);
    let status_filter = params.status.clone();

    let state_clone = state.clone();
    let (entries, counts) = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        // Aggregate counts by status — cheap on the status index and
        // lets the UI show queue depth without paging through rows.
        let mut counts = serde_json::Map::new();
        {
            let mut stmt = conn
                .prepare("SELECT status, COUNT(*) FROM federation_outbox GROUP BY status")
                .map_err(|e| ApiError::InternalServerError(format!("query failed: {e}")))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|e| ApiError::InternalServerError(format!("query failed: {e}")))?;
            for r in rows {
                let (status, n) =
                    r.map_err(|e| ApiError::InternalServerError(format!("row error: {e}")))?;
                counts.insert(status, serde_json::Value::from(n));
            }
        }

        let base_sql = "SELECT o.id, o.peer_instance_id, i.base_url, i.label, o.message_id, \
                        o.status, o.attempts, o.next_retry_at, o.last_error, o.created_at, \
                        o.updated_at, LENGTH(o.envelope_json) \
                        FROM federation_outbox o \
                        LEFT JOIN instances i ON i.id = o.peer_instance_id";
        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<OutboxEntry> {
            Ok(OutboxEntry {
                id: row.get(0)?,
                peer_instance_id: row.get(1)?,
                peer_base_url: row.get(2)?,
                peer_label: row.get(3)?,
                message_id: row.get(4)?,
                status: row.get(5)?,
                attempts: row.get(6)?,
                next_retry_at: row.get(7)?,
                last_error: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
                envelope_bytes: row.get(11)?,
            })
        };

        let mut entries = Vec::new();
        if let Some(ref status) = status_filter {
            let sql =
                format!("{base_sql} WHERE o.status = ?1 ORDER BY o.id DESC LIMIT ?2 OFFSET ?3");
            let mut stmt = stmt_or_500(&conn, &sql)?;
            let rows = stmt
                .query_map(rusqlite::params![status, limit, offset], map_row)
                .map_err(|e| ApiError::InternalServerError(format!("query failed: {e}")))?;
            for r in rows {
                entries
                    .push(r.map_err(|e| ApiError::InternalServerError(format!("row error: {e}")))?);
            }
        } else {
            let sql = format!("{base_sql} ORDER BY o.id DESC LIMIT ?1 OFFSET ?2");
            let mut stmt = stmt_or_500(&conn, &sql)?;
            let rows = stmt
                .query_map(rusqlite::params![limit, offset], map_row)
                .map_err(|e| ApiError::InternalServerError(format!("query failed: {e}")))?;
            for r in rows {
                entries
                    .push(r.map_err(|e| ApiError::InternalServerError(format!("row error: {e}")))?);
            }
        }

        Ok::<_, ApiError>((entries, counts))
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({
        "entries": entries,
        "counts": counts,
        "limit": limit,
        "offset": offset,
    }))
    .into_response())
}

/// Prepare a statement, mapping failure to a 500. Local helper so the
/// two filter branches in [`list_federation_outbox_handler`] stay flat.
fn stmt_or_500<'conn>(
    conn: &'conn rusqlite::Connection,
    sql: &str,
) -> Result<rusqlite::Statement<'conn>, ApiError> {
    conn.prepare(sql)
        .map_err(|e| ApiError::InternalServerError(format!("query failed: {e}")))
}

/// Handler for `POST /api/admin/federation/outbox/{id}/retry`.
///
/// Returns a terminally `failed` (or operator-`paused`) outbox row to
/// the retry rotation: status back to `pending`, attempts reset to 0 so
/// the row gets a fresh backoff budget, `next_retry_at` set to now so
/// the next worker tick picks it up, and `last_error` cleared.
///
/// Rows that are already `pending` or `delivered` are rejected with 409
/// — retrying a delivered envelope would duplicate-deliver (the
/// receiver's receipt ledger would drop it, but the attempt is still
/// wasted work), and retrying a pending row is a no-op the operator
/// should know about.
///
/// A retried row still passes through the dequeue-time SSRF gate in
/// `crate::background::drain_outbox_batch`, so retrying a row whose
/// peer URL points at a private host simply re-fails it.
///
/// Requires `can_moderate` permission.
pub async fn retry_federation_outbox_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(outbox_id): Path<i64>,
) -> Result<Response, ApiError> {
    if !identity.can_moderate {
        return Err(ApiError::Forbidden(
            "insufficient permissions to retry federation outbox rows".to_string(),
        ));
    }

    let state_clone = state.clone();
    let moderator = identity.pseudonym_id.clone();
    let message_id = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|e| ApiError::InternalServerError(format!("db connection failed: {e}")))?;

        // Single conditional UPDATE so a concurrent worker tick can't
        // race between a read and a write. Zero rows affected means
        // the row is missing or in a non-retryable state — distinguish
        // afterwards for the right error code.
        let updated = conn
            .execute(
                "UPDATE federation_outbox SET status = 'pending', attempts = 0, \
                 next_retry_at = datetime('now'), last_error = NULL, \
                 updated_at = datetime('now') \
                 WHERE id = ?1 AND status IN ('failed', 'paused')",
                rusqlite::params![outbox_id],
            )
            .map_err(|e| ApiError::InternalServerError(format!("update failed: {e}")))?;

        use rusqlite::OptionalExtension;
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT message_id, status FROM federation_outbox WHERE id = ?1",
                rusqlite::params![outbox_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| ApiError::InternalServerError(format!("query failed: {e}")))?;

        let (message_id, status) = match row {
            Some(r) => r,
            None => {
                return Err(ApiError::NotFound(format!(
                    "federation outbox row {outbox_id} not found"
                )))
            }
        };

        if updated == 0 {
            return Err(ApiError::Conflict(format!(
                "federation outbox row {outbox_id} is '{status}' — only 'failed' or 'paused' rows can be retried"
            )));
        }

        let observe_payload = EventPayload::ModerationAction {
            moderator_pseudonym: moderator.clone(),
            action_type: "federation_outbox_retry".to_string(),
            target_pseudonym: None,
            description: format!(
                "Federation outbox row {outbox_id} (message {message_id}) returned to retry rotation"
            ),
        };
        crate::emit_and_broadcast(
            &conn,
            state_clone.server_id,
            &moderator,
            &observe_payload,
            &state_clone.observe_tx,
            &state_clone.signing_key,
        );

        Ok::<String, ApiError>(message_id)
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({
        "status": "ok",
        "outbox_id": outbox_id,
        "message_id": message_id,
        "new_status": "pending",
    }))
    .into_response())
}
