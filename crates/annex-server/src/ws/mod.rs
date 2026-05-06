//! WebSocket plumbing: wire protocol types, HMAC session tokens, the
//! connection / subscription registry, the per-connection session
//! orchestrator, the message dispatcher, and the per-command handlers.
//!
//! Submodules:
//!
//!   * [`protocol`] — `WsConnectParams`, `IncomingMessage`,
//!     `WsMessagePayload`, and `OutgoingMessage`. Serialisation tags and
//!     casing are unchanged.
//!   * [`tokens`] — HMAC session-token format (base64url over
//!     `pseudonym|expires|hex(hmac)`), the two TTL constants
//!     (`WS_TOKEN_TTL_SECS`, `SESSION_TOKEN_TTL_SECS`), and the derive /
//!     generate / verify helpers.
//!   * [`connection_manager`] — `ConnectionManager`, the session →
//!     channel-subscription registry that backs broadcast / send /
//!     unsubscribe.
//!   * [`error`] — `send_ws_error` / `send_ws_error_with_id`.
//!   * [`context`] — `CommandContext<'a>`, the borrowed view of
//!     per-connection state passed to each command handler.
//!   * [`dispatch`] — the [`crate::ws::dispatch::dispatch`] entry point
//!     plus the membership gate that every command shares.
//!   * [`session`] — `WsSession::run`, the per-connection lifecycle
//!     called from [`crate::api_ws::handle_socket`].
//!   * [`commands`] — per-`IncomingMessage` handlers.
//!
//! The crate-public symbols are also re-exported from `api_ws` (via
//! the `pub use` lines at the top of that file) so that callers and
//! integration tests that name `annex_server::api_ws::ConnectionManager`,
//! `OutgoingMessage`, `generate_session_token`, etc. keep their existing
//! paths.

pub mod commands;
pub mod connection_manager;
pub mod context;
pub mod dispatch;
pub mod error;
pub mod protocol;
pub mod session;
pub mod tokens;

pub use connection_manager::ConnectionManager;
pub use protocol::{IncomingMessage, OutgoingMessage, WsConnectParams, WsMessagePayload};
pub use session::WsSession;
pub use tokens::{
    derive_ws_token_secret, generate_session_token, verify_token_allow_expired,
    verify_ws_token_for_auth, SESSION_TOKEN_TTL_SECS, WS_TOKEN_TTL_SECS,
};
