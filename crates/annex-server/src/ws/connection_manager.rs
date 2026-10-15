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
    sync::{Arc, Weak},
};

use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};

use tokio::sync::{mpsc, Mutex, OwnedMutexGuard, RwLock};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// One live session: its id, the writer channel, and the token that tells its
/// reader loop to stop.
///
/// The token is the part that was missing. `add_session` replaces an existing
/// session by dropping its `Sender`, which ends the *writer* task — but the
/// reader loop in `ws::session` is `while let Some(Ok(msg)) = receiver.next()`,
/// which ends only when the CLIENT closes. So an evicted socket stayed open,
/// holding a reader task, an ICE task and a renegotiation task, and simply
/// stopped receiving anything. One valid token could therefore pin an
/// unbounded number of connections: open, get evicted, open again. Nothing in
/// the registry grew, which is why it did not look like a leak.
#[derive(Clone)]
pub(crate) struct Session {
    pub(crate) id: Uuid,
    pub(crate) sender: mpsc::Sender<String>,
    pub(crate) cancel: CancellationToken,
    /// Set before cancellation; the writer transmits this disposition on wire.
    pub(crate) close_code: Arc<AtomicU16>,
}

/// Type alias for session map to satisfy clippy complexity checks.
type SessionMap = HashMap<String, Session>;

/// Maximum concurrent WebSocket connections across all identities.
///
/// A per-identity cap of one already follows from replacement, but that only
/// bounds an identity, not the process. Each socket costs a reader task, a
/// writer task, two event-relay tasks and a 1024-slot channel, so the ceiling
/// is about memory and scheduler pressure rather than about any one abuser.
/// Refusing the upgrade with 503 is the honest answer — the client retries,
/// and the alternative is accepting the connection and then failing to serve
/// it.
pub const MAX_WS_CONNECTIONS: usize = 10_000;

/// Manages active WebSocket connections and subscriptions.
#[derive(Clone, Default)]
pub struct ConnectionManager {
    /// Serializes registration and teardown for one identity, including media
    /// cleanup. Weak entries cannot accumulate after disconnected identities.
    lifecycle_locks: Arc<std::sync::Mutex<HashMap<String, Weak<Mutex<()>>>>>,
    /// Active sessions: pseudonym -> (session_id, sender).
    sessions: Arc<RwLock<SessionMap>>,
    /// Subscriptions: channel_id -> set of pseudonyms.
    channel_subscriptions: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    /// Reverse mapping: pseudonym -> set of channel_ids.
    user_subscriptions: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    /// Sockets that have been admitted and not yet released.
    ///
    /// Counted separately from `sessions.len()`, and that distinction is the
    /// point: a socket is admitted at the upgrade, before the identity is
    /// known, and `sessions` holds at most one entry per identity. Sizing the
    /// cap off `sessions` would have counted every evicted-but-still-open
    /// connection as zero.
    open_connections: Arc<AtomicUsize>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            lifecycle_locks: Default::default(),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            channel_subscriptions: Arc::new(RwLock::new(HashMap::new())),
            user_subscriptions: Arc::new(RwLock::new(HashMap::new())),
            open_connections: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Claim a connection slot, or `None` if the server is at capacity.
    ///
    /// The returned guard releases the slot on drop, so every early return and
    /// every panic in the session task gives the slot back. A manual
    /// decrement at the end of the handler would have leaked on the paths that
    /// matter most — the ones taken when something has already gone wrong.
    pub fn try_admit(&self) -> Option<ConnectionSlot> {
        let prev = self
            .open_connections
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < MAX_WS_CONNECTIONS).then_some(n + 1)
            });
        match prev {
            Ok(_) => Some(ConnectionSlot {
                counter: self.open_connections.clone(),
            }),
            Err(_) => None,
        }
    }

    /// Sockets currently admitted.
    pub fn open_connections(&self) -> usize {
        self.open_connections.load(Ordering::SeqCst)
    }

    /// Registers a new session for a pseudonym.
    ///
    /// If the pseudonym already has a session, the old session's subscriptions
    /// are cleaned up before replacement to prevent orphaned entries in
    /// `channel_subscriptions` and `user_subscriptions`.
    ///
    /// Lock ordering: sessions → channel_subscriptions → user_subscriptions
    /// (matches `remove_session`, `subscribe`, and `unsubscribe`). The
    /// previous version of this method took the user_subscriptions
    /// write lock *before* the channel_subscriptions write lock, which
    /// inverted the documented invariant and could deadlock against
    /// any concurrent `subscribe()` (which takes them in the
    /// chan_subs → user_subs order).
    ///
    /// Returns the unique session ID.
    pub async fn add_session(
        &self,
        pseudonym: String,
        sender: mpsc::Sender<String>,
    ) -> (Uuid, CancellationToken) {
        let (id, cancel, _) = self
            .add_session_with_cleanup(pseudonym, sender, std::future::ready(()))
            .await;
        (id, cancel)
    }

    pub(crate) async fn lock_lifecycle(&self, pseudonym: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self
                .lifecycle_locks
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            locks.retain(|_, lock| lock.strong_count() > 0);
            if let Some(lock) = locks.get(pseudonym).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(Mutex::new(()));
                locks.insert(pseudonym.to_owned(), Arc::downgrade(&lock));
                lock
            }
        };
        lock.lock_owned().await
    }

    pub(crate) async fn add_session_with_cleanup<F: std::future::Future<Output = ()>>(
        &self,
        pseudonym: String,
        sender: mpsc::Sender<String>,
        cleanup_replaced_media: F,
    ) -> (Uuid, CancellationToken, Arc<AtomicU16>) {
        let _lifecycle = self.lock_lifecycle(&pseudonym).await;
        self.add_session_with_cleanup_locked(pseudonym, sender, cleanup_replaced_media)
            .await
    }

    /// Caller holds this identity's lifecycle guard, including any final
    /// authorization check. Used to serialize registration with revocation.
    pub(crate) async fn add_session_with_cleanup_locked<F: std::future::Future<Output = ()>>(
        &self,
        pseudonym: String,
        sender: mpsc::Sender<String>,
        cleanup_replaced_media: F,
    ) -> (Uuid, CancellationToken, Arc<AtomicU16>) {
        let session_id = Uuid::new_v4();
        let cancel = CancellationToken::new();
        let close_code = Arc::new(AtomicU16::new(0));

        // 1. Atomically replace any existing session under a single
        //    write lock to prevent TOCTOU races when two connections
        //    for the same pseudonym arrive concurrently.
        let replaced = {
            let mut sessions = self.sessions.write().await;
            sessions.insert(
                pseudonym.clone(),
                Session {
                    id: session_id,
                    sender,
                    cancel: cancel.clone(),
                    close_code: close_code.clone(),
                },
            )
        };

        // Tell the evicted socket to close. Dropping its `Sender` only ends
        // the writer task; the reader loop waits on the client, which may
        // never speak again, and the socket would otherwise stay open holding
        // four tasks and a 1024-slot channel for nothing.
        if let Some(ref old) = replaced {
            old.close_code.store(4090, Ordering::SeqCst);
            old.cancel.cancel();
        }

        if replaced.is_some() {
            // The new reader cannot dispatch until this method returns. The
            // old finalizer takes the same lifecycle lock and then observes its
            // stale generation, so it cannot tear down this replacement later.
            cleanup_replaced_media.await;
            // 2. Read user_subscriptions briefly to collect the old
            //    session's channel list. A read lock is sufficient
            //    because no other writer can race us — concurrent
            //    `add_session(pseudonym)` callers were serialised by
            //    the sessions write lock above, and any concurrent
            //    `unsubscribe()` will simply observe a smaller set
            //    than we cleaned (idempotent).
            let channels = {
                let user_subs = self.user_subscriptions.read().await;
                user_subs.get(&pseudonym).cloned()
            };

            // 3. Write channel_subscriptions first (lock invariant).
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

            // 4. Write user_subscriptions last.
            if channels.is_some() {
                let mut user_subs = self.user_subscriptions.write().await;
                user_subs.remove(&pseudonym);
            }

            tracing::info!(
                pseudonym = %pseudonym,
                "replaced existing WebSocket session; cleaned up old subscriptions"
            );
        }

        (session_id, cancel, close_code)
    }

    /// Disconnects a user by pseudonym, closing their WebSocket session.
    pub async fn disconnect_user(&self, pseudonym: &str) {
        let _lifecycle = self.lock_lifecycle(pseudonym).await;
        self.disconnect_user_locked(pseudonym).await;
    }

    /// Caller holds this identity's lifecycle guard through persistent epoch
    /// invalidation and media cleanup, so a reconnect cannot escape either.
    pub(crate) async fn disconnect_user_locked(&self, pseudonym: &str) {
        let session_id = {
            let sessions = self.sessions.read().await;
            sessions.get(pseudonym).map(|s| {
                s.close_code.store(4001, Ordering::SeqCst);
                s.id
            })
        };

        if let Some(id) = session_id {
            self.remove_session_locked(pseudonym, id).await;
        }
    }

    /// Removes a session for a pseudonym if the session ID matches.
    ///
    /// Lock ordering: sessions → channel_subscriptions → user_subscriptions.
    /// This matches the ordering used by `subscribe` and `unsubscribe`
    /// (channel_subscriptions → user_subscriptions) to prevent deadlocks.
    pub async fn remove_session(&self, pseudonym: &str, session_id: Uuid) {
        let _lifecycle = self.lock_lifecycle(pseudonym).await;
        self.remove_session_locked(pseudonym, session_id).await;
    }

    /// Final cleanup may run after an explicit server disconnect removed the
    /// registry entry. It still owns media if no replacement has registered.
    pub(crate) async fn finalize_session<F: std::future::Future<Output = ()>>(
        &self,
        pseudonym: &str,
        session_id: Uuid,
        cleanup_media: F,
    ) {
        let _lifecycle = self.lock_lifecycle(pseudonym).await;
        if self
            .sessions
            .read()
            .await
            .get(pseudonym)
            .is_some_and(|s| s.id != session_id)
        {
            return;
        }
        self.remove_session_locked(pseudonym, session_id).await;
        cleanup_media.await;
    }

    async fn remove_session_locked(&self, pseudonym: &str, session_id: Uuid) {
        // 1. Remove from sessions (independent lock, always acquired first).
        {
            let mut sessions = self.sessions.write().await;
            match sessions.get(pseudonym) {
                Some(existing) if existing.id != session_id => return, // Stale removal request
                Some(_) => {}
                None => return, // Already removed
            }
            if let Some(removed) = sessions.remove(pseudonym) {
                // Idempotent, and the reason `disconnect_user` now actually
                // disconnects: before, it cleared the registry and left the
                // socket open.
                removed.cancel.cancel();
            }
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
                if let Some(session) = sessions.get(pseudonym) {
                    if let Err(e) = session.sender.try_send(message_json.clone()) {
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
        for session in sessions.values() {
            if let Err(e) = session.sender.try_send(message_json.clone()) {
                tracing::warn!("dropping broadcast_all message for slow consumer: {}", e);
            }
        }
    }

    /// Sends a message string to a specific user (pseudonym).
    pub async fn send(&self, pseudonym: &str, message_json: String) {
        let sessions = self.sessions.read().await;
        if let Some(session) = sessions.get(pseudonym) {
            if let Err(e) = session.sender.try_send(message_json) {
                tracing::warn!(
                    pseudonym = %pseudonym,
                    "dropping direct message for slow consumer: {}",
                    e
                );
            }
        }
    }
}

/// Holds one WebSocket connection slot; releases it on drop.
pub struct ConnectionSlot {
    counter: Arc<AtomicUsize>,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_sender() -> mpsc::Sender<String> {
        let (tx, _rx) = mpsc::channel::<String>(8);
        tx
    }

    #[tokio::test]
    async fn add_session_replacement_cleans_up_old_subscriptions() {
        // After [F27]: re-registering a pseudonym with a new session
        // must clear stale entries from BOTH chan_subs and user_subs.
        let mgr = ConnectionManager::new();
        let _id1 = mgr.add_session("alice".to_string(), dummy_sender()).await;
        mgr.subscribe("ch1".to_string(), "alice".to_string()).await;
        mgr.subscribe("ch2".to_string(), "alice".to_string()).await;

        // Sanity: alice is in both channel sets.
        assert_eq!(
            mgr.channel_subscriptions
                .read()
                .await
                .get("ch1")
                .map(|s| s.len()),
            Some(1)
        );
        assert_eq!(
            mgr.user_subscriptions
                .read()
                .await
                .get("alice")
                .map(|s| s.len()),
            Some(2)
        );

        // Re-register: stale subscriptions must be wiped.
        let _id2 = mgr.add_session("alice".to_string(), dummy_sender()).await;

        let chan_subs = mgr.channel_subscriptions.read().await;
        // Either the channel entry is gone entirely (last subscriber
        // removed → empty set → entry pruned) or alice is no longer
        // in it.
        assert!(
            chan_subs
                .get("ch1")
                .map(|s| !s.contains("alice"))
                .unwrap_or(true),
            "chan_subs[ch1] must not contain alice after replacement"
        );
        assert!(
            chan_subs
                .get("ch2")
                .map(|s| !s.contains("alice"))
                .unwrap_or(true),
            "chan_subs[ch2] must not contain alice after replacement"
        );
        drop(chan_subs);

        let user_subs = mgr.user_subscriptions.read().await;
        assert!(
            user_subs.get("alice").is_none(),
            "user_subs[alice] must be empty/absent after replacement"
        );
    }

    #[tokio::test]
    async fn add_session_replacement_does_not_deadlock_with_concurrent_subscribe() {
        // Regression test for [F27]: the previous version of
        // `add_session` took user_subscriptions(write) before
        // channel_subscriptions(write), inverting the documented lock
        // order and risking a deadlock with `subscribe()` (which takes
        // chan_subs(write) before user_subs(write)).
        //
        // We can't deterministically force the deadlock from a test
        // — RwLock doesn't expose lock-acquisition timestamps — but we
        // can confirm that interleaved calls complete in bounded
        // time. If the lock ordering regresses, this test will hang
        // until the harness timeout under high contention.
        let mgr = std::sync::Arc::new(ConnectionManager::new());
        let _id = mgr.add_session("bob".to_string(), dummy_sender()).await;
        for i in 0..10 {
            mgr.subscribe(format!("ch{i}"), "bob".to_string()).await;
        }

        // Spawn N concurrent subscribers and replacers, each touching
        // both locks. The test passes by completing.
        let mut handles = vec![];
        for i in 0..16 {
            let mgr = mgr.clone();
            handles.push(tokio::spawn(async move {
                if i % 2 == 0 {
                    mgr.subscribe(format!("ch{i}-extra"), "bob".to_string())
                        .await;
                } else {
                    let _id = mgr.add_session("bob".to_string(), dummy_sender()).await;
                }
            }));
        }
        for h in handles {
            // 5s timeout: a deadlock would block forever.
            tokio::time::timeout(std::time::Duration::from_secs(5), h)
                .await
                .expect("connection_manager deadlocked under contention")
                .expect("task panicked");
        }
    }

    #[tokio::test]
    async fn stale_finalizer_cannot_run_cleanup_for_a_replacement_generation() {
        let mgr = ConnectionManager::new();
        let (old, _) = mgr.add_session("alice".into(), dummy_sender()).await;
        let cleanups = Arc::new(AtomicUsize::new(0));
        let replacement_cleanup = cleanups.clone();
        let (current, _, _) = mgr
            .add_session_with_cleanup("alice".into(), dummy_sender(), async move {
                replacement_cleanup.fetch_add(1, Ordering::SeqCst);
            })
            .await;
        let stale_cleanup = cleanups.clone();
        mgr.finalize_session("alice", old, async move {
            stale_cleanup.fetch_add(100, Ordering::SeqCst);
        })
        .await;
        assert_eq!(cleanups.load(Ordering::SeqCst), 1);
        let current_cleanup = cleanups.clone();
        mgr.finalize_session("alice", current, async move {
            current_cleanup.fetch_add(1, Ordering::SeqCst);
        })
        .await;
        assert_eq!(cleanups.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn new_registration_waits_until_previous_media_teardown_completes() {
        let mgr = ConnectionManager::new();
        let (old, _) = mgr.add_session("alice".into(), dummy_sender()).await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let cleanup_mgr = mgr.clone();
        let cleanup = tokio::spawn(async move {
            cleanup_mgr
                .finalize_session("alice", old, async move {
                    started_tx.send(()).unwrap();
                    release_rx.await.unwrap();
                })
                .await;
        });
        started_rx.await.unwrap();
        let replace_mgr = mgr.clone();
        let mut replacement = tokio::spawn(async move {
            replace_mgr
                .add_session("alice".into(), dummy_sender())
                .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut replacement)
                .await
                .is_err()
        );
        release_tx.send(()).unwrap();
        cleanup.await.unwrap();
        replacement.await.unwrap();
    }
}
