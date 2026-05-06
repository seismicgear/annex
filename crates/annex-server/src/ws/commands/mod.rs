//! Per-WebSocket-command handlers.
//!
//! Each submodule exposes a single async `handle(ctx, …)` function for
//! one [`crate::ws::protocol::IncomingMessage`] variant (or, for the
//! WebRTC pair, two functions sharing one module). The dispatcher in
//! [`crate::ws::dispatch`] forwards each variant to the right handler.
//!
//! Handlers do not own the connection lifetime; they receive a borrowed
//! [`crate::ws::context::CommandContext`] holding the shared state, the
//! authenticated identity, the resolved pseudonym, and the outbound
//! sender. They are free to clone the `Arc<AppState>` as needed for
//! background tasks.
//!
//! Modules are added incrementally as each command is extracted from
//! the formerly inline match arms. Every extraction is a behaviour-
//! preserving move: the handler runs the same input validation, the
//! same membership / role gates, and emits the same protocol frames as
//! the inline arm it replaces.

pub mod delete;
pub mod edit;
pub mod message;
pub mod resume;
pub mod typing;
pub mod voice;
pub mod webrtc;
