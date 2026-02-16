use crate::{middleware::IdentityContext, AppState};
use annex_channels::{add_member, get_channel, remove_member};
use annex_types::{AlignmentStatus, RoleCode};
use axum::{
    extract::{Extension, Path},
    http::StatusCode,
    response::Json,
};
use rusqlite::{params, OptionalExtension};
use serde_json::json;
use std::sync::Arc;

/// POST /api/channels/:channelId/join
pub async fn join_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 1. Fetch Channel
    let channel = {
        let pool = state.pool.clone();
        let cid = channel_id.clone();
        let channel_res = tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            get_channel(&conn, &cid).map_err(|_| StatusCode::NOT_FOUND)
        })
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        match channel_res {
            Ok(c) => c,
            Err(code) => return Err(code),
        }
    };

    // 2. Check Capabilities
    if let Some(caps_json) = &channel.required_capabilities_json {
        let required: Vec<String> =
            serde_json::from_str(caps_json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        for req in required {
            let has_cap = match req.as_str() {
                "can_voice" => identity.can_voice,
                "can_moderate" => identity.can_moderate,
                "can_invite" => identity.can_invite,
                "can_federate" => identity.can_federate,
                "can_bridge" => identity.can_bridge,
                _ => false, // Unknown capability required -> deny
            };
            if !has_cap {
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    // 3. Check Agent Alignment
    if identity.participant_type == RoleCode::AiAgent {
        if let Some(min_alignment) = channel.agent_min_alignment {
            // Query agent registration
            let alignment_status: Option<String> = tokio::task::spawn_blocking({
                let pool = state.pool.clone();
                let server_id = state.server_id;
                let pseudo = identity.pseudonym_id.clone();
                move || {
                    let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                    conn.query_row(
                        "SELECT alignment_status FROM agent_registrations WHERE server_id = ?1 AND pseudonym_id = ?2",
                        params![server_id, pseudo],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
                }
            })
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

            let status_str = alignment_status.ok_or(StatusCode::FORBIDDEN)?; // Agent not registered

            // Compare alignment
            // AlignmentStatus enum: Aligned, Partial, Conflict
            // Parsing: The DB stores string representation "Aligned", "Partial", "Conflict" (via serde_json or Display?)
            // Migration 008 says: alignment_status TEXT
            // annex-vrp likely stores via serde_json.
            // If stored as "Aligned" (quoted json string) or Aligned (plain text)?
            // Usually serde_json::to_string gives "\"Aligned\"".
            // Let's assume serde_json serialization.

            // If stored as JSON string "\"Aligned\"", from_str works.
            // If stored as plain text "Aligned", we need to quote it or parse manually.
            // annex-vrp reputation module likely uses serde_json to store it in handshake log,
            // but agent_registrations table?
            // "alignment_status TEXT NOT NULL"
            // Let's assume serde_json.

            let status: AlignmentStatus = serde_json::from_str(&status_str)
                .or_else(|_| serde_json::from_str(&format!("\"{}\"", status_str))) // Try quoting if raw
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            // Logic: Aligned > Partial > Conflict
            let allowed = match min_alignment {
                AlignmentStatus::Conflict => true,
                AlignmentStatus::Partial => status != AlignmentStatus::Conflict,
                AlignmentStatus::Aligned => status == AlignmentStatus::Aligned,
            };

            if !allowed {
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    // 4. Add Member
    tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let server_id = state.server_id;
        let cid = channel_id.clone();
        let pid = identity.pseudonym_id.clone();
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            add_member(&conn, server_id, &cid, &pid).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    Ok(Json(json!({"status": "joined"})))
}

/// POST /api/channels/:channelId/leave
pub async fn leave_channel_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(IdentityContext(identity)): Extension<IdentityContext>,
    Path(channel_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 1. Remove Member
    tokio::task::spawn_blocking({
        let pool = state.pool.clone();
        let cid = channel_id.clone();
        let pid = identity.pseudonym_id.clone();
        move || {
            let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            remove_member(&conn, &cid, &pid).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    // 2. Unsubscribe from WebSocket
    state
        .connection_manager
        .unsubscribe(&channel_id, &identity.pseudonym_id)
        .await;

    Ok(Json(json!({"status": "left"})))
}
