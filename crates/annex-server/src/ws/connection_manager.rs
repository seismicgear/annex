//! In-memory registry of live WebSocket sessions and their per-channel
//! subscriptions.
//!
//! Three maps are maintained behind separate `tokio::sync::RwLock`s:
//!
//!   * `sessions`: `pseudonym -> (session_id, sender)`. The `session_id`
//!     is a UUID stamped at registration so a stale `remove_session`
//!     after a reconnect does not tear down the new session.
//!   * `channel_subscriptions`: `channel_id -> set<pseudonym>`.
//!   * `user_subscriptions`: `pseudonym -> set<channel_id>`. Reverse of
//!     `channel_subscriptions`, kept in lock-step so disconnect can clean
//!     up in O(channels-per-user) instead of scanning every channel.
//!
//! Lock ordering: `sessions → channel_subscriptions → user_subscriptions`.
//! Every mutating method follows this ordering so concurrent users cannot
//! deadlock; the comment on `remove_session` documents the invariant.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use tokio::sync::{mpsc, RwLock};
use uuid::Uuid;

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
    pub async fn add_session(&self, pseudonym: String, sender: mpsc::Sender<String>) -> Uuid {
        let session_id = Uuid::new_v4();

        // Atomically replace any existing session under a single write lock
        // to prevent TOCTOU races when two connections for the same pseudonym
        // arrive concurrently.
        let had_previous = {
            let mut sessions = self.sessions.write().await;
            let old = sessions.insert(pseudonym.clone(), (session_id, sender));
            old.is_some()
        };

        if had_previous {
            // Clean up old subscriptions (channel_subscriptions → user_subscriptions order).
            let channels = {
                let mut user_subs = self.user_subscriptions.write().await;
                user_subs.remove(&pseudonym)
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

            tracing::info!(
                pseudonym = %pseudonym,
                "replaced existing WebSocket session; cleaned up old subscriptions"
            );
        }

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

    /// Removes all subscriptions for a channel (e.g., when it is deleted).
    ///
    /// Returns the set of pseudonyms that were subscribed, so callers can
    /// notify them if needed.
    pub async fn unsubscribe_channel(&self, channel_id: &str) -> HashSet<String> {
        let mut chan_subs = self.channel_subscriptions.write().await;
        let removed = chan_subs.remove(channel_id).unwrap_or_default();

        let mut user_subs = self.user_subscriptions.write().await;
        for pseudonym in &removed {
            if let Some(channels) = user_subs.get_mut(pseudonym) {
                channels.remove(channel_id);
                if channels.is_empty() {
                    user_subs.remove(pseudonym);
                }
            }
        }

        removed
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

    /// Broadcasts a message string to ALL connected sessions (server-wide events).
    pub async fn broadcast_all(&self, message_json: String) {
        let sessions = self.sessions.read().await;
        for (_, (_, sender)) in sessions.iter() {
            if let Err(e) = sender.try_send(message_json.clone()) {
                tracing::warn!("dropping broadcast_all message for slow consumer: {}", e);
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
