//! WebSocket API handler and connection management.

use crate::AppState;
use annex_channels::{create_message, is_member, CreateMessageParams, Message};
use annex_identity::get_platform_identity;
use axum::{
    extract::{
        ws::{Message as AxumMessage, WebSocket},
        Extension, Query, WebSocketUpgrade,
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

/// Query parameters for the WebSocket connection.
#[derive(Debug, Deserialize)]
pub struct WsConnectParams {
    pub pseudonym: String,
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
}

/// Outgoing WebSocket message wrapper (for broadcast).
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum OutgoingMessage {
    #[serde(rename = "message")]
    Message(Message),
    #[serde(rename = "error")]
    Error { message: String },
}

/// Type alias for session map to satisfy clippy complexity checks.
type SessionMap = HashMap<String, (Uuid, mpsc::UnboundedSender<String>)>;

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
    /// Returns the unique session ID.
    pub async fn add_session(
        &self,
        pseudonym: String,
        sender: mpsc::UnboundedSender<String>,
    ) -> Uuid {
        let session_id = Uuid::new_v4();
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
    pub async fn remove_session(&self, pseudonym: &str, session_id: Uuid) {
        let mut sessions = self.sessions.write().await;

        // Check if session matches
        if let Some((current_id, _)) = sessions.get(pseudonym) {
            if *current_id != session_id {
                // Stale removal request, ignore
                return;
            }
        } else {
            // Already removed
            return;
        }

        // Remove session
        sessions.remove(pseudonym);

        // Clean up subscriptions
        let mut user_subs = self.user_subscriptions.write().await;
        if let Some(channels) = user_subs.remove(pseudonym) {
            let mut chan_subs = self.channel_subscriptions.write().await;
            for channel_id in channels {
                if let Some(listeners) = chan_subs.get_mut(&channel_id) {
                    listeners.remove(pseudonym);
                    if listeners.is_empty() {
                        chan_subs.remove(&channel_id);
                    }
                }
            }
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
                    let _ = sender.send(message_json.clone());
                }
            }
        }
    }
}

/// WebSocket handler: `GET /ws?pseudonym=...`
pub async fn ws_handler(
    Extension(state): Extension<Arc<AppState>>,
    ws: WebSocketUpgrade,
    Query(params): Query<WsConnectParams>,
) -> impl IntoResponse {
    // 1. Authenticate
    // We do a blocking check against the DB.
    let server_id = state.server_id;
    let pseudonym = params.pseudonym.clone();

    let state_clone = state.clone();
    let auth_result = tokio::task::spawn_blocking(move || {
        let conn = state_clone
            .pool
            .get()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        match get_platform_identity(&conn, server_id, &pseudonym) {
            Ok(identity) if identity.active => Ok(identity),
            Ok(_) => Err(StatusCode::FORBIDDEN), // Inactive
            Err(_) => Err(StatusCode::UNAUTHORIZED),
        }
    })
    .await;

    match auth_result {
        Ok(Ok(_identity)) => {
            // Success
            ws.on_upgrade(move |socket| handle_socket(socket, state, params.pseudonym))
        }
        Ok(Err(code)) => code.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Handles the WebSocket connection.
async fn handle_socket(socket: WebSocket, state: Arc<AppState>, pseudonym: String) {
    // 1. Mark as active immediately
    tokio::spawn(touch_activity(state.clone(), pseudonym.clone()));

    let (mut sender, mut receiver) = socket.split();

    // Create a channel for this session
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

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

    // Handle incoming messages
    while let Some(Ok(msg)) = receiver.next().await {
        // Update activity on any message (fire and forget)
        tokio::spawn(touch_activity(state.clone(), pseudonym.clone()));

        if let AxumMessage::Text(text) = msg {
            if let Ok(incoming) = serde_json::from_str::<IncomingMessage>(&text.to_string()) {
                match incoming {
                    IncomingMessage::Subscribe { channel_id } => {
                        // Check membership
                        let allowed = {
                            let pool = state.pool.clone();
                            let cid = channel_id.clone();
                            let pid = pseudonym.clone();
                            tokio::task::spawn_blocking(move || {
                                let conn = pool.get().map_err(|_| "pool error")?;
                                is_member(&conn, &cid, &pid).map_err(|_| "db error")
                            })
                            .await
                            .unwrap_or(Ok(false))
                            .unwrap_or(false)
                        };

                        if allowed {
                            state
                                .connection_manager
                                .subscribe(channel_id, pseudonym.clone())
                                .await;
                        } else {
                            let _ = tx.send(
                                serde_json::to_string(&OutgoingMessage::Error {
                                    message: format!("Not a member of channel {}", channel_id),
                                })
                                .unwrap(),
                            );
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
                        // 1. Validate membership (enforcing Phase 4.4 requirements)
                        let allowed = {
                            let pool = state.pool.clone();
                            let cid = channel_id.clone();
                            let pid = pseudonym.clone();
                            tokio::task::spawn_blocking(move || {
                                let conn = pool.get().map_err(|_| "pool error")?;
                                is_member(&conn, &cid, &pid).map_err(|_| "db error")
                            })
                            .await
                            .unwrap_or(Ok(false))
                            .unwrap_or(false)
                        };

                        if !allowed {
                            let _ = tx.send(
                                serde_json::to_string(&OutgoingMessage::Error {
                                    message: format!("Not a member of channel {}", channel_id),
                                })
                                .unwrap(),
                            );
                            continue;
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
                            create_message(&conn, &params).map_err(|e| e.to_string())
                        })
                        .await;

                        match res {
                            Ok(Ok(message)) => {
                                // Broadcast
                                let out = OutgoingMessage::Message(message);
                                if let Ok(json) = serde_json::to_string(&out) {
                                    state
                                        .connection_manager
                                        .broadcast(&channel_id_clone, json)
                                        .await;
                                }
                            }
                            Ok(Err(e)) => {
                                tracing::error!("Failed to persist message: {}", e);
                                // Ideally send error back to user
                            }
                            Err(e) => {
                                tracing::error!("Task join error: {}", e);
                            }
                        }
                    }
                }
            } else {
                tracing::warn!("Failed to parse incoming WebSocket message");
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
}

async fn touch_activity(state: Arc<AppState>, pseudonym: String) {
    let pool = state.pool.clone();
    let server_id = state.server_id;
    let tx = state.presence_tx.clone();

    tokio::task::spawn_blocking(move || {
        if let Ok(conn) = pool.get() {
            if let Ok(true) = annex_graph::update_node_activity(&conn, server_id, &pseudonym) {
                let _ = tx.send(annex_types::PresenceEvent::NodeUpdated {
                    pseudonym_id: pseudonym,
                    active: true,
                });
            }
        }
    })
    .await
    .ok();
}
