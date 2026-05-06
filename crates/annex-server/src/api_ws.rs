//! HTTP entry points for the WebSocket surface: token issuance,
//! session refresh, and the `/ws` upgrade.
//!
//! The per-connection lifecycle, per-message dispatch, command
//! handlers, wire-protocol types, HMAC token helpers, and the
//! connection / subscription registry now live under
//! [`crate::ws`]. Everything that was public from this module before
//! the split is re-exported below so external paths
//! (`annex_server::api_ws::ConnectionManager`, `OutgoingMessage`,
//! `generate_session_token`, `SESSION_TOKEN_TTL_SECS`, …) keep working
//! unchanged for handlers, services, integration tests, and the route
//! registration in [`crate::routes`].

use crate::ws::session::WsSession;
use crate::ws::tokens::verify_ws_token;
use crate::AppState;
use annex_identity::get_platform_identity;
use axum::{
    extract::{ws::WebSocket, ConnectInfo, Extension, Query, WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
};
use std::{net::SocketAddr, sync::Arc};

// ── Public re-exports — preserve `annex_server::api_ws::Foo` paths ──────
pub use crate::ws::connection_manager::ConnectionManager;
pub use crate::ws::protocol::{
    IncomingMessage, OutgoingMessage, WsConnectParams, WsMessagePayload,
};
pub use crate::ws::tokens::{
    derive_ws_token_secret, generate_session_token, verify_token_allow_expired,
    verify_ws_token_for_auth, SESSION_TOKEN_TTL_SECS, WS_TOKEN_TTL_SECS,
};

/// `POST /api/session/refresh` — re-issues a session token for a returning user
/// whose previous token has expired. Accepts expired-but-validly-signed tokens.
///
/// This does NOT go through `auth_middleware` (which rejects expired tokens).
/// Instead it manually verifies the HMAC signature (proving the token was issued
/// by this server) and confirms the identity is still active before issuing
/// a fresh session token.
pub async fn refresh_session_handler(
    Extension(state): Extension<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<axum::Json<serde_json::Value>, StatusCode> {
    // Extract Bearer token from Authorization header
    let auth_val = headers
        .get("Authorization")
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let auth_str = auth_val.to_str().map_err(|_| StatusCode::UNAUTHORIZED)?;
    let token = auth_str
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Verify HMAC (but allow expired)
    let pseudonym = verify_token_allow_expired(token, &state.ws_token_secret)?;

    // Verify identity is still active in the database
    let server_id = state.server_id;
    let pool = state.pool.clone();
    let pseudonym_clone = pseudonym.clone();
    let identity = tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        annex_identity::get_platform_identity(&conn, server_id, &pseudonym_clone)
            .map_err(|_| StatusCode::UNAUTHORIZED)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    if !identity.active {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Issue fresh session token
    let new_token =
        generate_session_token(&pseudonym, &state.ws_token_secret, SESSION_TOKEN_TTL_SECS);
    Ok(axum::Json(serde_json::json!({
        "sessionToken": new_token,
        "expires_in_secs": SESSION_TOKEN_TTL_SECS,
    })))
}

/// `POST /api/ws/token` — issues a short-lived, HMAC-signed WebSocket session
/// token for the authenticated user. Clients should call this endpoint and
/// then connect to `/ws?token=<token>` instead of passing raw pseudonyms.
///
/// Requires authentication via `auth_middleware` (X-Annex-Pseudonym or Bearer).
pub async fn create_ws_token_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(crate::middleware::IdentityContext(identity)): Extension<
        crate::middleware::IdentityContext,
    >,
) -> Result<axum::Json<serde_json::Value>, StatusCode> {
    let token = generate_session_token(
        &identity.pseudonym_id,
        &state.ws_token_secret,
        WS_TOKEN_TTL_SECS,
    );
    Ok(axum::Json(serde_json::json!({
        "token": token,
        "expires_in_secs": WS_TOKEN_TTL_SECS,
    })))
}

/// WebSocket handler: `GET /ws?token=...` (preferred) or `GET /ws?pseudonym=...` (legacy).
///
/// When a signed `token` parameter is present, the server verifies the HMAC
/// signature and expiry, then resolves the bound pseudonym. This prevents
/// impersonation and replay attacks.
///
/// The legacy `pseudonym` parameter is still accepted for backwards compatibility
/// but should be considered deprecated. All new clients should use the token flow.
///
/// All auth attempts (success and failure) are logged with the remote address
/// for security monitoring.
pub async fn ws_handler(
    Extension(state): Extension<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
    Query(params): Query<WsConnectParams>,
) -> impl IntoResponse {
    // 1. Resolve pseudonym — prefer signed token over raw pseudonym
    let pseudonym = if let Some(ref token) = params.token {
        match verify_ws_token(token, &state.ws_token_secret) {
            Ok(p) => p,
            Err(code) => {
                tracing::warn!(
                    remote_addr = %addr,
                    status = %code,
                    "websocket token verification failed"
                );
                return code.into_response();
            }
        }
    } else if let Some(ref p) = params.pseudonym {
        // Reject raw pseudonym connections when ZK proof enforcement is enabled.
        // The raw pseudonym parameter offers no cryptographic binding and allows
        // impersonation by anyone who knows (or can compute) the pseudonym.
        if state.enforce_zk_proofs {
            tracing::warn!(
                pseudonym = %p,
                remote_addr = %addr,
                "websocket raw pseudonym rejected (enforce_zk_proofs is enabled)"
            );
            return StatusCode::UNAUTHORIZED.into_response();
        }
        // Validate pseudonym format before DB lookup — reject injection payloads
        if p.is_empty()
            || p.len() > 128
            || !p
                .as_bytes()
                .iter()
                .all(|&b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            tracing::warn!(
                remote_addr = %addr,
                "websocket raw pseudonym rejected (invalid format)"
            );
            return StatusCode::UNAUTHORIZED.into_response();
        }
        tracing::debug!(
            pseudonym = %p,
            remote_addr = %addr,
            "websocket auth via legacy pseudonym parameter (deprecated)"
        );
        p.clone()
    } else {
        tracing::warn!(remote_addr = %addr, "websocket connect missing token and pseudonym");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    // 2. Authenticate via DB
    let server_id = state.server_id;
    let pseudonym_clone = pseudonym.clone();

    let state_clone = state.clone();
    let auth_result = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        match get_platform_identity(&conn, server_id, &pseudonym_clone) {
            Ok(identity) if identity.active => Ok(identity),
            Ok(_) => Err(StatusCode::FORBIDDEN), // Inactive
            Err(_) => Err(StatusCode::UNAUTHORIZED),
        }
    })
    .await;

    match auth_result {
        Ok(Ok(identity)) => {
            tracing::info!(
                pseudonym = %pseudonym,
                remote_addr = %addr,
                token_auth = params.token.is_some(),
                "websocket auth success"
            );
            ws.on_upgrade(move |socket: WebSocket| WsSession::run(socket, state, identity))
        }
        Ok(Err(code)) => {
            tracing::warn!(
                pseudonym = %pseudonym,
                remote_addr = %addr,
                status = %code,
                "websocket auth failed"
            );
            code.into_response()
        }
        Err(_) => {
            tracing::warn!(
                pseudonym = %pseudonym,
                remote_addr = %addr,
                "websocket auth internal error"
            );
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use annex_channels::Message;

    #[test]
    fn ws_message_payload_serializes_camel_case() {
        let payload = WsMessagePayload {
            channel_id: "ch-1".to_string(),
            message_id: "msg-1".to_string(),
            sender_pseudonym: "alice".to_string(),
            content: "hello".to_string(),
            reply_to_message_id: Some("msg-0".to_string()),
            created_at: "2025-01-01T00:00:00Z".to_string(),
            edited_at: None,
            deleted_at: None,
            client_request_id: None,
        };

        let json = serde_json::to_value(&payload).expect("serialization should not fail");
        assert!(
            json.get("channelId").is_some(),
            "expected camelCase channelId"
        );
        assert!(
            json.get("messageId").is_some(),
            "expected camelCase messageId"
        );
        assert!(
            json.get("senderPseudonym").is_some(),
            "expected camelCase senderPseudonym"
        );
        assert!(
            json.get("replyToMessageId").is_some(),
            "expected camelCase replyToMessageId"
        );
        assert!(
            json.get("createdAt").is_some(),
            "expected camelCase createdAt"
        );

        assert!(
            json.get("channel_id").is_none(),
            "snake_case channel_id should not be present"
        );
        assert!(
            json.get("message_id").is_none(),
            "snake_case message_id should not be present"
        );

        // Verify clientRequestId is omitted when None
        assert!(
            json.get("clientRequestId").is_none(),
            "clientRequestId should be omitted when None"
        );

        // Verify clientRequestId is present when Some
        let payload_with_id = WsMessagePayload {
            client_request_id: Some("req-123".to_string()),
            ..payload
        };
        let json_with_id =
            serde_json::to_value(&payload_with_id).expect("serialization should not fail");
        assert_eq!(
            json_with_id.get("clientRequestId").and_then(|v| v.as_str()),
            Some("req-123"),
            "clientRequestId should be echoed when present"
        );
    }

    #[test]
    fn ws_message_payload_from_message() {
        let msg = Message {
            id: 0,
            server_id: 0,
            channel_id: "ch-2".to_string(),
            message_id: "msg-2".to_string(),
            sender_pseudonym: "bob".to_string(),
            content: "world".to_string(),
            reply_to_message_id: None,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: None,
            edited_at: None,
            deleted_at: None,
        };

        let payload: WsMessagePayload = msg.into();
        assert_eq!(payload.channel_id, "ch-2");
        assert_eq!(payload.message_id, "msg-2");
        assert_eq!(payload.sender_pseudonym, "bob");
        assert_eq!(payload.content, "world");
        assert!(payload.reply_to_message_id.is_none());
    }

    #[test]
    fn outgoing_message_wraps_with_type_tag() {
        let payload = WsMessagePayload {
            channel_id: "ch-1".to_string(),
            message_id: "msg-1".to_string(),
            sender_pseudonym: "alice".to_string(),
            content: "test".to_string(),
            reply_to_message_id: None,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            edited_at: None,
            deleted_at: None,
            client_request_id: None,
        };

        let out = OutgoingMessage::Message(payload);
        let json = serde_json::to_value(&out).expect("serialization should not fail");
        assert_eq!(json.get("type").and_then(|v| v.as_str()), Some("message"));
        assert!(json.get("channelId").is_some());
    }
}
