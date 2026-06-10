//! Borrowed view of per-connection state passed into each WebSocket
//! command handler.
//!
//! [`CommandContext`] holds short-lived references to the four things
//! every command needs: the shared `AppState`, the authenticated
//! `PlatformIdentity` (so handlers can run capability / role checks),
//! the resolved pseudonym (cached as `&str` to avoid re-cloning out of
//! identity), and the `mpsc::Sender<String>` used to enqueue protocol
//! frames back to this socket.
//!
//! The struct is intentionally a borrow rather than an owned bundle so
//! the dispatcher in [`crate::ws::session`] can construct one cheaply on
//! every loop iteration without touching the connection-lifetime state.

use std::sync::Arc;

use annex_identity::PlatformIdentity;
use tokio::sync::mpsc;

use crate::ws::command_rate_limit::CommandRateLimiter;
use crate::ws::typing_throttle::TypingThrottle;
use crate::AppState;

/// Borrowed per-message context shared across every command handler.
pub struct CommandContext<'a> {
    /// Shared application state (pool, voice service, connection manager,
    /// policy, …). Cloned cheaply via `Arc::clone` when a handler needs
    /// to spawn a `tokio::task::spawn_blocking` closure.
    pub state: &'a Arc<AppState>,
    /// Identity of the authenticated WebSocket peer.
    pub identity: &'a PlatformIdentity,
    /// `identity.pseudonym_id` re-borrowed for ergonomic use in arms
    /// that pass the pseudonym to multiple helpers without cloning.
    pub pseudonym: &'a str,
    /// Sender for the per-connection outbound mpsc queue. Handlers push
    /// JSON-serialised [`crate::ws::protocol::OutgoingMessage`] payloads
    /// here; the session task forwards them to the WebSocket sink.
    pub tx: &'a mpsc::Sender<String>,
    /// Per-session, per-channel debouncer for `IncomingMessage::Typing`.
    /// Owned by [`crate::ws::session::WsSession::run`] for the lifetime
    /// of the connection; borrowed here so the typing handler can
    /// suppress floods without touching shared global state.
    pub typing_throttle: &'a TypingThrottle,
    /// Per-session token bucket for state-mutating commands (message,
    /// edit, delete, voice intent, resume). Owned by the session task;
    /// borrowed here so the dispatcher can clamp WS command floods that
    /// the HTTP rate-limit middleware never sees.
    pub command_rate_limiter: &'a CommandRateLimiter,
}
