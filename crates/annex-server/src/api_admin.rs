//! Admin API handlers for the Annex server.

use crate::{
    api::ApiError, config::derive_server_slug_from_public_url, middleware::IdentityContext,
    policy::recalculate_all_alignments, AppState,
};
use annex_identity::update_capabilities;
use annex_observe::EventPayload;
use annex_types::{Capabilities, ServerPolicy};
use axum::{
    extract::{Extension, Json, Path},
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
        );

        Ok::<(), ApiError>(())
    })
    .await
    .map_err(|e| ApiError::InternalServerError(format!("task join error: {e}")))??;

    Ok(AxumJson(serde_json::json!({ "status": "ok" })).into_response())
}
