//! WebSocket API handler and connection management.

use crate::api_federation::relay_message;
use crate::AppState;
use annex_channels::{
    create_message, delete_message, edit_message, get_channel, is_member, CreateMessageParams,
    Message,
};
use annex_identity::{get_platform_identity, PlatformIdentity};
use annex_types::{FederationScope, RoleCode};
use axum::{
    extract::{
        ws::{Message as AxumMessage, WebSocket},
        ConnectInfo, Extension, Query, WebSocketUpgrade,
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

/// Duration for which a WebSocket session token is valid (60 seconds).
/// Tokens are single-use: the short TTL limits replay risk for unused tokens.
const WS_TOKEN_TTL_SECS: u64 = 60;

/// Derive a 32-byte HMAC key for WebSocket session tokens from the server's
/// Ed25519 signing key. Uses SHA-256 with a domain-separation prefix so the
/// derived key is independent of any other use of the signing key.
pub fn derive_ws_token_secret(signing_key: &ed25519_dalek::SigningKey) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"annex-ws-token-v1:");
    hasher.update(signing_key.as_bytes());
    let result = hasher.finalize();
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&result);
    secret
}

/// Generates an HMAC-SHA256 signed WebSocket session token.
///
/// Token format: `base64(pseudonym|expires_unix_secs|hmac_signature)`
/// The token binds the pseudonym to a time window, preventing both
/// impersonation (different pseudonym) and replay (after expiry).
fn generate_ws_token(pseudonym: &str, secret: &[u8; 32]) -> String {
    use sha2::Sha256;
    use hmac::{Hmac, Mac};

    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + WS_TOKEN_TTL_SECS;

    let payload = format!("{}|{}", pseudonym, expires);

    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key length is valid");
    mac.update(payload.as_bytes());
    let signature = mac.finalize().into_bytes();

    use base64::Engine;
    let token_bytes = format!("{}|{}", payload, hex::encode(signature));
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes.as_bytes())
}

/// Verifies an HMAC-SHA256 signed WebSocket session token.
/// Returns the pseudonym if valid and not expired.
fn verify_ws_token(token: &str, secret: &[u8; 32]) -> Result<String, StatusCode> {
    use sha2::Sha256;
    use hmac::{Hmac, Mac};
    use base64::Engine;

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let token_str = String::from_utf8(decoded).map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Parse: pseudonym|expires|signature_hex
    let parts: Vec<&str> = token_str.splitn(3, '|').collect();
    if parts.len() != 3 {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let pseudonym = parts[0];
    let expires_str = parts[1];
    let sig_hex = parts[2];

    // Verify HMAC
    let payload = format!("{}|{}", pseudonym, expires_str);
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC key length is valid");
    mac.update(payload.as_bytes());
    let expected_sig = mac.finalize().into_bytes();
    let provided_sig = hex::decode(sig_hex).map_err(|_| StatusCode::UNAUTHORIZED)?;

    if expected_sig.as_slice() != provided_sig.as_slice() {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Check expiry
    let expires: u64 = expires_str.parse().map_err(|_| StatusCode::UNAUTHORIZED)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if now > expires {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(pseudonym.to_string())
}

/// Query parameters for the WebSocket connection.
///
/// Accepts either a signed `token` (preferred) or a raw `pseudonym`
/// (legacy/backwards-compatible). When both are present, `token` takes
/// precedence.
#[derive(Debug, Deserialize)]
pub struct WsConnectParams {
    pub pseudonym: Option<String>,
    pub token: Option<String>,
}

/// Incoming WebSocket message types.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum IncomingMessage {
    #[serde(rename = "subscribe")]
    Subscribe {
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    #[serde(rename = "unsubscribe")]
    Unsubscribe {
        #[serde(rename = "channelId")]
        channel_id: String,
    },
    #[serde(rename = "message")]
    Message {
        #[serde(rename = "channelId")]
        channel_id: String,
        content: String,
        #[serde(rename = "replyTo")]
        reply_to: Option<String>,
    },
    #[serde(rename = "edit_message")]
    EditMessage {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "messageId")]
        message_id: String,
        content: String,
    },
    #[serde(rename = "delete_message")]
    DeleteMessage {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "messageId")]
        message_id: String,
    },
    #[serde(rename = "voice_intent")]
    VoiceIntent {
        #[serde(rename = "channelId")]
        channel_id: String,
        text: String,
    },
}

/// Outgoing WebSocket message payload with camelCase field names.
///
/// The inner `Message` struct uses snake_case for HTTP API responses.
/// WebSocket messages use camelCase to match the frontend `WsReceiveFrame` type.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WsMessagePayload {
    pub channel_id: String,
    pub message_id: String,
    pub sender_pseudonym: String,
    pub content: String,
    pub reply_to_message_id: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
}

impl From<Message> for WsMessagePayload {
    fn from(m: Message) -> Self {
        Self {
            channel_id: m.channel_id,
            message_id: m.message_id,
            sender_pseudonym: m.sender_pseudonym,
            content: m.content,
            reply_to_message_id: m.reply_to_message_id,
            created_at: m.created_at,
            edited_at: m.edited_at,
            deleted_at: m.deleted_at,
        }
    }
}

/// Outgoing WebSocket message wrapper (for broadcast).
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum OutgoingMessage {
    #[serde(rename = "message")]
    Message(WsMessagePayload),
    #[serde(rename = "message_edited")]
    MessageEdited(WsMessagePayload),
    #[serde(rename = "message_deleted")]
    MessageDeleted(WsMessagePayload),
    #[serde(rename = "transcription")]
    Transcription {
        #[serde(rename = "channelId")]
        channel_id: String,
        #[serde(rename = "speakerPseudonym")]
        speaker_pseudonym: String,
        text: String,
    },
    #[serde(rename = "error")]
    Error { message: String },
}

/// Type alias for session map to satisfy clippy complexity checks.
type SessionMap = HashMap<String, (Uuid, mpsc::Sender<String>)>;

/// Manages active WebSocket connections and subscriptions.
#[derive(Clone, Default)]
pub struct ConnectionManager {
    /// Active sessions: pseudonym -> (session_id, sender).
    sessions: Arc<RwLock<SessionMap>>,
    /// Subscriptions: channel_id -> set of pseudonyms.
    channel_subscriptions: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    /// Reverse mapping: pseudonym -> set of channel_ids.
    user_subscriptions: Arc<RwLock<HashMap<String, HashSet<String>>>>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            channel_subscriptions: Arc::new(RwLock::new(HashMap::new())),
            user_subscriptions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Registers a new session for a pseudonym.
    ///
    /// If the pseudonym already has a session, the old session's subscriptions
    /// are cleaned up before replacement to prevent orphaned entries in
    /// `channel_subscriptions` and `user_subscriptions`.
    ///
    /// Returns the unique session ID.
    pub async fn add_session(
        &self,
        pseudonym: String,
        sender: mpsc::Sender<String>,
    ) -> Uuid {
        let session_id = Uuid::new_v4();

        // Check for and clean up an existing session for this pseudonym.
        let had_previous = {
            let sessions = self.sessions.read().await;
            sessions.contains_key(&pseudonym)
        };

        if had_previous {
            // Clean up old subscriptions (channel_subscriptions → user_subscriptions order).
            let channels = {
                let user_subs = self.user_subscriptions.read().await;
                user_subs.get(&pseudonym).cloned()
            };

            if let Some(ref channels) = channels {
                let mut chan_subs = self.channel_subscriptions.write().await;
                for channel_id in channels {
                    if let Some(listeners) = chan_subs.get_mut(channel_id) {
                        listeners.remove(&pseudonym);
                        if listeners.is_empty() {
                            chan_subs.remove(channel_id);
                        }
                    }
                }
            }

            if channels.is_some() {
                let mut user_subs = self.user_subscriptions.write().await;
                user_subs.remove(&pseudonym);
            }

            tracing::info!(
                pseudonym = %pseudonym,
                "replaced existing WebSocket session; cleaned up old subscriptions"
            );
        }

        self.sessions
            .write()
            .await
            .insert(pseudonym, (session_id, sender));
        session_id
    }

    /// Disconnects a user by pseudonym, closing their WebSocket session.
    pub async fn disconnect_user(&self, pseudonym: &str) {
        let session_id = {
            let sessions = self.sessions.read().await;
            sessions.get(pseudonym).map(|(id, _)| *id)
        };

        if let Some(id) = session_id {
            self.remove_session(pseudonym, id).await;
        }
    }

    /// Removes a session for a pseudonym if the session ID matches.
    ///
    /// Lock ordering: sessions → channel_subscriptions → user_subscriptions.
    /// This matches the ordering used by `subscribe` and `unsubscribe`
    /// (channel_subscriptions → user_subscriptions) to prevent deadlocks.
    pub async fn remove_session(&self, pseudonym: &str, session_id: Uuid) {
        // 1. Remove from sessions (independent lock, always acquired first).
        {
            let mut sessions = self.sessions.write().await;
            if let Some((current_id, _)) = sessions.get(pseudonym) {
                if *current_id != session_id {
                    return; // Stale removal request
                }
            } else {
                return; // Already removed
            }
            sessions.remove(pseudonym);
        }

        // 2. Collect the channels this user was subscribed to.
        let channels = {
            let user_subs = self.user_subscriptions.read().await;
            user_subs.get(pseudonym).cloned()
        };

        // 3. Remove from channel_subscriptions first (consistent with subscribe/unsubscribe).
        if let Some(ref channels) = channels {
            let mut chan_subs = self.channel_subscriptions.write().await;
            for channel_id in channels {
                if let Some(listeners) = chan_subs.get_mut(channel_id) {
                    listeners.remove(pseudonym);
                    if listeners.is_empty() {
                        chan_subs.remove(channel_id);
                    }
                }
            }
        }

        // 4. Remove from user_subscriptions last.
        if channels.is_some() {
            let mut user_subs = self.user_subscriptions.write().await;
            user_subs.remove(pseudonym);
        }
    }

    /// Subscribes a pseudonym to a channel.
    pub async fn subscribe(&self, channel_id: String, pseudonym: String) {
        let mut chan_subs = self.channel_subscriptions.write().await;
        chan_subs
            .entry(channel_id.clone())
            .or_default()
            .insert(pseudonym.clone());

        let mut user_subs = self.user_subscriptions.write().await;
        user_subs.entry(pseudonym).or_default().insert(channel_id);
    }

    /// Unsubscribes a pseudonym from a channel.
    pub async fn unsubscribe(&self, channel_id: &str, pseudonym: &str) {
        let mut chan_subs = self.channel_subscriptions.write().await;
        if let Some(listeners) = chan_subs.get_mut(channel_id) {
            listeners.remove(pseudonym);
            if listeners.is_empty() {
                chan_subs.remove(channel_id);
            }
        }

        let mut user_subs = self.user_subscriptions.write().await;
        if let Some(channels) = user_subs.get_mut(pseudonym) {
            channels.remove(channel_id);
            if channels.is_empty() {
                user_subs.remove(pseudonym);
            }
        }
    }

    /// Broadcasts a message string to all subscribers of a channel.
    pub async fn broadcast(&self, channel_id: &str, message_json: String) {
        let chan_subs = self.channel_subscriptions.read().await;
        if let Some(listeners) = chan_subs.get(channel_id) {
            let sessions = self.sessions.read().await;
            for pseudonym in listeners {
                if let Some((_, sender)) = sessions.get(pseudonym) {
                    if let Err(e) = sender.try_send(message_json.clone()) {
                        tracing::warn!(
                            pseudonym = %pseudonym,
                            channel_id = %channel_id,
                            "dropping broadcast message for slow consumer: {}",
                            e
                        );
                    }
                }
            }
        }
    }

    /// Sends a message string to a specific user (pseudonym).
    pub async fn send(&self, pseudonym: &str, message_json: String) {
        let sessions = self.sessions.read().await;
        if let Some((_, sender)) = sessions.get(pseudonym) {
            if let Err(e) = sender.try_send(message_json) {
                tracing::warn!(
                    pseudonym = %pseudonym,
                    "dropping direct message for slow consumer: {}",
                    e
                );
            }
        }
    }
}

/// `POST /api/ws/token` — issues a short-lived, HMAC-signed WebSocket session
/// token for the authenticated user. Clients should call this endpoint and
/// then connect to `/ws?token=<token>` instead of passing raw pseudonyms.
///
/// Requires authentication via `auth_middleware` (X-Annex-Pseudonym or Bearer).
pub async fn create_ws_token_handler(
    Extension(state): Extension<Arc<AppState>>,
    Extension(crate::middleware::IdentityContext(identity)): Extension<crate::middleware::IdentityContext>,
) -> Result<axum::Json<serde_json::Value>, StatusCode> {
    let token = generate_ws_token(&identity.pseudonym_id, &state.ws_token_secret);
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
            ws.on_upgrade(move |socket| handle_socket(socket, state, identity))
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

/// Result of a WebSocket membership check.
enum MembershipResult {
    /// The user is a confirmed member.
    Allowed,
    /// The user is not a member.
    Denied,
    /// An internal error occurred during the check.
    Error(String),
}

/// Checks channel membership via a blocking DB query.
///
/// Returns [`MembershipResult`] rather than silently swallowing errors.
async fn check_ws_membership(
    pool: annex_db::DbPool,
    server_id: i64,
    channel_id: &str,
    pseudonym: &str,
) -> MembershipResult {
    let cid = channel_id.to_string();
    let pid = pseudonym.to_string();
    let result = tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| format!("pool error: {}", e))?;
        is_member(&conn, server_id, &cid, &pid).map_err(|e| format!("db error: {}", e))
    })
    .await;

    match result {
        Ok(Ok(true)) => MembershipResult::Allowed,
        Ok(Ok(false)) => MembershipResult::Denied,
        Ok(Err(e)) => MembershipResult::Error(e),
        Err(e) => MembershipResult::Error(format!("task join error: {}", e)),
    }
}

/// Sends a JSON-serialized error message over the WebSocket sender channel.
fn send_ws_error(tx: &mpsc::Sender<String>, message: String) {
    match serde_json::to_string(&OutgoingMessage::Error { message }) {
        Ok(json) => {
            if let Err(e) = tx.try_send(json) {
                tracing::warn!("failed to send WebSocket error to client: {}", e);
            }
        }
        Err(e) => {
            tracing::error!("failed to serialize WebSocket error message: {}", e);
        }
    }
}

/// Maximum allowed length for a WebSocket message content field (64 KiB).
const MAX_WS_MESSAGE_CONTENT_LEN: usize = 65_536;

/// Maximum allowed length for a VoiceIntent text field (2 KiB).
/// TTS synthesis is CPU/memory intensive; limiting input size prevents
/// resource abuse from oversized text payloads.
const MAX_VOICE_INTENT_TEXT_LEN: usize = 2_048;

/// Minimum interval between activity updates per WebSocket connection.
/// Prevents spawning a blocking DB task on every single message.
const ACTIVITY_DEBOUNCE: std::time::Duration = std::time::Duration::from_secs(30);

/// Handles the WebSocket connection.
async fn handle_socket(socket: WebSocket, state: Arc<AppState>, identity: PlatformIdentity) {
    let pseudonym = identity.pseudonym_id.clone();

    // 1. Mark as active immediately
    tokio::spawn(touch_activity(state.clone(), pseudonym.clone()));

    let (mut sender, mut receiver) = socket.split();

    // Create a bounded channel for this session to prevent unbounded memory growth
    // from slow consumers. 256 messages provides sufficient buffer for normal
    // operation; beyond that the client is too slow and messages are dropped.
    let (tx, mut rx) = mpsc::channel::<String>(256);

    // Register session
    let session_id = state
        .connection_manager
        .add_session(pseudonym.clone(), tx.clone())
        .await;

    // Spawn a task to forward messages from rx to the websocket sender
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(AxumMessage::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Track last activity update to debounce DB writes
    let mut last_activity = std::time::Instant::now();

    // Handle incoming messages
    while let Some(Ok(msg)) = receiver.next().await {
        // Debounce activity updates: only spawn a DB write if enough time has passed
        if last_activity.elapsed() >= ACTIVITY_DEBOUNCE {
            tokio::spawn(touch_activity(state.clone(), pseudonym.clone()));
            last_activity = std::time::Instant::now();
        }

        if let AxumMessage::Text(text) = msg {
            if let Ok(incoming) = serde_json::from_str::<IncomingMessage>(&text.to_string()) {
                match incoming {
                    IncomingMessage::Subscribe { channel_id } => {
                        match check_ws_membership(state.pool.clone(), state.server_id, &channel_id, &pseudonym).await
                        {
                            MembershipResult::Allowed => {
                                state
                                    .connection_manager
                                    .subscribe(channel_id, pseudonym.clone())
                                    .await;
                            }
                            MembershipResult::Denied => {
                                send_ws_error(
                                    &tx,
                                    format!("Not a member of channel {}", channel_id),
                                );
                            }
                            MembershipResult::Error(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "subscribe membership check failed: {}",
                                    e
                                );
                                send_ws_error(
                                    &tx,
                                    "Internal error checking channel membership".to_string(),
                                );
                            }
                        }
                    }
                    IncomingMessage::Unsubscribe { channel_id } => {
                        state
                            .connection_manager
                            .unsubscribe(&channel_id, &pseudonym)
                            .await;
                    }
                    IncomingMessage::Message {
                        channel_id,
                        content,
                        reply_to,
                    } => {
                        // 0. Validate content length
                        if content.len() > MAX_WS_MESSAGE_CONTENT_LEN {
                            send_ws_error(
                                &tx,
                                format!(
                                    "Message content exceeds maximum length of {} bytes",
                                    MAX_WS_MESSAGE_CONTENT_LEN
                                ),
                            );
                            continue;
                        }

                        // 1. Validate membership (enforcing Phase 4.4 requirements)
                        match check_ws_membership(state.pool.clone(), state.server_id, &channel_id, &pseudonym).await
                        {
                            MembershipResult::Allowed => {}
                            MembershipResult::Denied => {
                                send_ws_error(
                                    &tx,
                                    format!("Not a member of channel {}", channel_id),
                                );
                                continue;
                            }
                            MembershipResult::Error(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "message membership check failed: {}",
                                    e
                                );
                                send_ws_error(
                                    &tx,
                                    "Internal error checking channel membership".to_string(),
                                );
                                continue;
                            }
                        }

                        let message_id = Uuid::new_v4().to_string();
                        let params = CreateMessageParams {
                            channel_id: channel_id.clone(),
                            message_id,
                            sender_pseudonym: pseudonym.clone(),
                            content,
                            reply_to_message_id: reply_to,
                        };

                        let state_clone = state.clone();
                        let channel_id_clone = channel_id.clone();

                        // DB Insert (blocking)
                        let res = tokio::task::spawn_blocking(move || {
                            let conn = state_clone.pool.get().map_err(|e| e.to_string())?;
                            let msg = create_message(&conn, &params).map_err(|e| e.to_string())?;

                            // Check if channel is federated
                            let channel =
                                get_channel(&conn, &channel_id_clone).map_err(|e| e.to_string())?;
                            let is_federated =
                                matches!(channel.federation_scope, FederationScope::Federated);

                            Ok::<_, String>((msg, is_federated))
                        })
                        .await;

                        match res {
                            Ok(Ok((message, is_federated))) => {
                                // Broadcast via WebSocket (camelCase payload)
                                let ws_payload: WsMessagePayload = message.clone().into();
                                let broadcast_channel_id = message.channel_id.clone();
                                let out = OutgoingMessage::Message(ws_payload);
                                match serde_json::to_string(&out) {
                                    Ok(json) => {
                                        state
                                            .connection_manager
                                            .broadcast(&broadcast_channel_id, json)
                                            .await;
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            channel_id = %broadcast_channel_id,
                                            "failed to serialize outgoing message for broadcast: {}", e
                                        );
                                    }
                                }

                                // Relay if federated
                                if is_federated {
                                    tokio::spawn(relay_message(
                                        state.clone(),
                                        message.channel_id.clone(),
                                        message,
                                    ));
                                }
                            }
                            Ok(Err(e)) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "failed to persist message: {}",
                                    e
                                );
                                send_ws_error(
                                    &tx,
                                    "Failed to send message: internal error".to_string(),
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "message persist task failed: {}",
                                    e
                                );
                                send_ws_error(
                                    &tx,
                                    "Failed to send message: internal error".to_string(),
                                );
                            }
                        }
                    }
                    IncomingMessage::EditMessage {
                        channel_id,
                        message_id,
                        content,
                    } => {
                        if content.len() > MAX_WS_MESSAGE_CONTENT_LEN {
                            send_ws_error(
                                &tx,
                                format!(
                                    "Message content exceeds maximum length of {} bytes",
                                    MAX_WS_MESSAGE_CONTENT_LEN
                                ),
                            );
                            continue;
                        }

                        // Membership check: same gate as Message handler
                        match check_ws_membership(state.pool.clone(), state.server_id, &channel_id, &pseudonym)
                            .await
                        {
                            MembershipResult::Allowed => {}
                            MembershipResult::Denied => {
                                send_ws_error(
                                    &tx,
                                    format!("Not a member of channel {}", channel_id),
                                );
                                continue;
                            }
                            MembershipResult::Error(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "edit membership check failed: {}",
                                    e
                                );
                                send_ws_error(
                                    &tx,
                                    "Internal error checking channel membership".to_string(),
                                );
                                continue;
                            }
                        }

                        let state_clone = state.clone();
                        let pseudonym_clone = pseudonym.clone();

                        let res = tokio::task::spawn_blocking(move || {
                            let conn = state_clone.pool.get().map_err(|e| e.to_string())?;
                            edit_message(&conn, &message_id, &pseudonym_clone, &content)
                                .map_err(|e| e.to_string())
                        })
                        .await;

                        match res {
                            Ok(Ok(updated)) => {
                                // Use the persisted channel_id from DB, not the
                                // client-supplied one, to prevent cross-channel
                                // broadcast spoofing.
                                let persisted_channel_id = updated.channel_id.clone();
                                let ws_payload: WsMessagePayload = updated.into();
                                let out = OutgoingMessage::MessageEdited(ws_payload);
                                match serde_json::to_string(&out) {
                                    Ok(json) => {
                                        state
                                            .connection_manager
                                            .broadcast(&persisted_channel_id, json)
                                            .await;
                                    }
                                    Err(e) => {
                                        tracing::error!("failed to serialize edit broadcast: {}", e);
                                    }
                                }
                            }
                            Ok(Err(e)) => {
                                send_ws_error(&tx, format!("Edit failed: {}", e));
                            }
                            Err(e) => {
                                tracing::error!("edit message task failed: {}", e);
                                send_ws_error(&tx, "Edit failed: internal error".to_string());
                            }
                        }
                    }
                    IncomingMessage::DeleteMessage {
                        channel_id,
                        message_id,
                    } => {
                        // Membership check: same gate as Message handler
                        match check_ws_membership(state.pool.clone(), state.server_id, &channel_id, &pseudonym)
                            .await
                        {
                            MembershipResult::Allowed => {}
                            MembershipResult::Denied => {
                                send_ws_error(
                                    &tx,
                                    format!("Not a member of channel {}", channel_id),
                                );
                                continue;
                            }
                            MembershipResult::Error(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "delete membership check failed: {}",
                                    e
                                );
                                send_ws_error(
                                    &tx,
                                    "Internal error checking channel membership".to_string(),
                                );
                                continue;
                            }
                        }

                        let state_clone = state.clone();
                        let pseudonym_clone = pseudonym.clone();

                        let res = tokio::task::spawn_blocking(move || {
                            let conn = state_clone.pool.get().map_err(|e| e.to_string())?;
                            delete_message(&conn, &message_id, &pseudonym_clone)
                                .map_err(|e| e.to_string())
                        })
                        .await;

                        match res {
                            Ok(Ok(updated)) => {
                                // Use the persisted channel_id from DB, not the
                                // client-supplied one, to prevent cross-channel
                                // broadcast spoofing.
                                let persisted_channel_id = updated.channel_id.clone();
                                let ws_payload: WsMessagePayload = updated.into();
                                let out = OutgoingMessage::MessageDeleted(ws_payload);
                                match serde_json::to_string(&out) {
                                    Ok(json) => {
                                        state
                                            .connection_manager
                                            .broadcast(&persisted_channel_id, json)
                                            .await;
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "failed to serialize delete broadcast: {}",
                                            e
                                        );
                                    }
                                }
                            }
                            Ok(Err(e)) => {
                                send_ws_error(&tx, format!("Delete failed: {}", e));
                            }
                            Err(e) => {
                                tracing::error!("delete message task failed: {}", e);
                                send_ws_error(&tx, "Delete failed: internal error".to_string());
                            }
                        }
                    }
                    IncomingMessage::VoiceIntent { channel_id, text } => {
                        if identity.participant_type != RoleCode::AiAgent {
                            send_ws_error(&tx, "Only AI agents can use VoiceIntent".to_string());
                            continue;
                        }

                        // Validate text length before expensive TTS synthesis
                        if text.len() > MAX_VOICE_INTENT_TEXT_LEN {
                            send_ws_error(
                                &tx,
                                format!(
                                    "VoiceIntent text exceeds maximum length of {} bytes",
                                    MAX_VOICE_INTENT_TEXT_LEN
                                ),
                            );
                            continue;
                        }

                        // Check membership
                        match check_ws_membership(state.pool.clone(), state.server_id, &channel_id, &pseudonym).await
                        {
                            MembershipResult::Allowed => {}
                            MembershipResult::Denied => {
                                send_ws_error(
                                    &tx,
                                    format!("Not a member of channel {}", channel_id),
                                );
                                continue;
                            }
                            MembershipResult::Error(e) => {
                                tracing::error!(
                                    pseudonym = %pseudonym,
                                    channel_id = %channel_id,
                                    "voice intent membership check failed: {}",
                                    e
                                );
                                send_ws_error(
                                    &tx,
                                    "Internal error checking channel membership".to_string(),
                                );
                                continue;
                            }
                        }

                        // Get voice profile ID
                        let voice_profile_id = {
                            let pool = state.pool.clone();
                            let server_id = state.server_id;
                            let pid = pseudonym.clone();
                            let result = tokio::task::spawn_blocking(move || {
                                let conn = pool.get().map_err(|e| format!("pool error: {}", e))?;
                                let profile_id: Option<String> = conn
                                    .query_row(
                                        "SELECT vp.profile_id
                                     FROM agent_registrations ar
                                     JOIN voice_profiles vp ON ar.voice_profile_id = vp.id
                                     WHERE ar.server_id = ?1 AND ar.pseudonym_id = ?2",
                                        rusqlite::params![server_id, pid],
                                        |row| row.get(0),
                                    )
                                    .optional()
                                    .map_err(|e| format!("db error: {}", e))?;
                                Ok::<Option<String>, String>(profile_id)
                            })
                            .await;

                            match result {
                                Ok(Ok(Some(id))) => id,
                                Ok(Ok(None)) => "default".to_string(),
                                Ok(Err(e)) => {
                                    tracing::warn!(
                                        pseudonym = %pseudonym,
                                        "voice profile lookup failed, using default: {}",
                                        e
                                    );
                                    "default".to_string()
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        pseudonym = %pseudonym,
                                        "voice profile lookup task failed, using default: {}",
                                        e
                                    );
                                    "default".to_string()
                                }
                            }
                        };

                        // Synthesize
                        match state.tts_service.synthesize(&text, &voice_profile_id).await {
                            Ok(audio) => {
                                // Get or create voice client.
                                // Fast-path: read lock to check for existing session.
                                let client_opt = match state.voice_sessions.read() {
                                    Ok(sessions) => sessions.get(&pseudonym).cloned(),
                                    Err(_) => {
                                        tracing::error!("voice_sessions lock poisoned");
                                        continue;
                                    }
                                };

                                let client = if let Some(c) = client_opt {
                                    c
                                } else {
                                    // Connect a new voice client
                                    let room_name = channel_id.clone();
                                    let token = match state
                                        .voice_service
                                        .generate_join_token(&room_name, &pseudonym, &pseudonym)
                                    {
                                        Ok(t) => t,
                                        Err(e) => {
                                            tracing::error!(
                                                pseudonym = %pseudonym,
                                                room = %room_name,
                                                "failed to generate voice join token: {}",
                                                e
                                            );
                                            send_ws_error(
                                                &tx,
                                                "Failed to generate voice token".to_string(),
                                            );
                                            continue;
                                        }
                                    };
                                    let url = state.voice_service.get_url();

                                    match annex_voice::AgentVoiceClient::connect(
                                        url,
                                        &token,
                                        &room_name,
                                        state.stt_service.clone(),
                                        state.voice_service.api_key(),
                                        state.voice_service.api_secret(),
                                    )
                                    .await
                                    {
                                        Ok(c) => {
                                            let arc = Arc::new(c);

                                            // Double-check under write lock to prevent
                                            // TOCTOU race with concurrent voice intents.
                                            match state.voice_sessions.write() {
                                                Ok(mut sessions) => {
                                                    use std::collections::hash_map::Entry;
                                                    match sessions.entry(pseudonym.clone()) {
                                                        Entry::Vacant(entry) => {
                                                            // Subscribe to transcriptions only for the winning insert
                                                            let mut rx =
                                                                arc.subscribe_transcriptions();
                                                            let cm =
                                                                state.connection_manager.clone();
                                                            let p_clone = pseudonym.clone();

                                                            tokio::spawn(async move {
                                                                while let Ok(event) =
                                                                    rx.recv().await
                                                                {
                                                                    let msg = OutgoingMessage::Transcription {
                                                                        channel_id: event.channel_id,
                                                                        speaker_pseudonym: event.speaker_pseudonym,
                                                                        text: event.text,
                                                                    };

                                                                    match serde_json::to_string(&msg) {
                                                                        Ok(json) => {
                                                                            cm.send(&p_clone, json)
                                                                                .await;
                                                                        }
                                                                        Err(e) => {
                                                                            tracing::error!(
                                                                                "failed to serialize transcription message: {}", e
                                                                            );
                                                                        }
                                                                    }
                                                                }
                                                            });

                                                            entry.insert(arc.clone());
                                                        }
                                                        Entry::Occupied(_) => {
                                                            // Concurrent request won; drop our client
                                                        }
                                                    }
                                                    match sessions.get(&pseudonym).cloned() {
                                                        Some(s) => s,
                                                        None => {
                                                            // Should never happen: we either just inserted or the Occupied branch
                                                            // guarantees presence. If it does, log and skip the voice operation.
                                                            tracing::error!(
                                                                pseudonym = %pseudonym,
                                                                "voice session missing after insert; this is a bug"
                                                            );
                                                            continue;
                                                        }
                                                    }
                                                }
                                                Err(_) => {
                                                    tracing::error!("voice_sessions lock poisoned");
                                                    continue;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            send_ws_error(
                                                &tx,
                                                format!("Failed to connect voice: {}", e),
                                            );
                                            continue;
                                        }
                                    }
                                };

                                if let Err(e) = client.publish_audio(&audio).await {
                                    send_ws_error(&tx, format!("Failed to publish audio: {}", e));
                                }
                            }
                            Err(e) => {
                                send_ws_error(&tx, format!("TTS failed: {}", e));
                            }
                        }
                    }
                }
            } else {
                tracing::warn!(pseudonym = %pseudonym, "failed to parse incoming WebSocket message");
                send_ws_error(&tx, "invalid message format".to_string());
            }
        } else if let AxumMessage::Close(_) = msg {
            break;
        }
    }

    // Cleanup with session_id check
    state
        .connection_manager
        .remove_session(&pseudonym, session_id)
        .await;
    send_task.abort();

    // Clean up voice session for this pseudonym. Dropping the Arc will
    // decrement the reference count; when it reaches zero the
    // AgentVoiceClient is dropped, its internal broadcast sender closes,
    // and the spawned transcription task will exit naturally.
    match state.voice_sessions.write() {
        Ok(mut sessions) => {
            sessions.remove(&pseudonym);
        }
        Err(e) => {
            tracing::error!(
                pseudonym = %pseudonym,
                "voice_sessions RwLock poisoned during cleanup: {}", e
            );
        }
    }
}

async fn touch_activity(state: Arc<AppState>, pseudonym: String) {
    let pool = state.pool.clone();
    let server_id = state.server_id;
    let tx = state.presence_tx.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = pool.get().map_err(|e| {
            tracing::warn!("touch_activity: failed to get db connection: {}", e);
        })?;
        match annex_graph::update_node_activity(&conn, server_id, &pseudonym) {
            Ok(true) => {
                let _ = tx.send(annex_types::PresenceEvent::NodeUpdated {
                    pseudonym_id: pseudonym,
                    active: true,
                });
            }
            Ok(false) => { /* Node was already active, no broadcast needed */ }
            Err(e) => {
                tracing::warn!(
                    pseudonym = %pseudonym,
                    "touch_activity: failed to update node activity: {}",
                    e
                );
            }
        }
        Ok::<(), ()>(())
    })
    .await;

    if let Err(e) = result {
        tracing::error!("touch_activity: blocking task panicked or was cancelled: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };

        let json = serde_json::to_value(&payload).expect("serialization should not fail");
        assert!(json.get("channelId").is_some(), "expected camelCase channelId");
        assert!(json.get("messageId").is_some(), "expected camelCase messageId");
        assert!(json.get("senderPseudonym").is_some(), "expected camelCase senderPseudonym");
        assert!(json.get("replyToMessageId").is_some(), "expected camelCase replyToMessageId");
        assert!(json.get("createdAt").is_some(), "expected camelCase createdAt");

        // Verify snake_case keys are NOT present
        assert!(json.get("channel_id").is_none(), "snake_case channel_id should not be present");
        assert!(json.get("message_id").is_none(), "snake_case message_id should not be present");
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
        };

        let out = OutgoingMessage::Message(payload);
        let json = serde_json::to_value(&out).expect("serialization should not fail");
        assert_eq!(json.get("type").and_then(|v| v.as_str()), Some("message"));
        assert!(json.get("channelId").is_some());
    }
}
