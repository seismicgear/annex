//! Annex server library logic.
//!
//! Top-level public API:
//!
//! * [`AppState`]   — shared request-handler state ([`state`]).
//! * [`prepare_server`], [`init_tracing`], [`StartupError`] — server boot
//!   path ([`startup`]).
//! * [`app`] — Axum router assembly ([`routes`]).
//!
//! Per-feature handler modules (`api`, `api_admin`, `api_channels`, …) and
//! the cross-cutting helpers (`config`, `middleware`, `policy`, `retention`,
//! `services`) are exposed unchanged so existing call sites and integration
//! tests keep their current paths.

pub mod api;
pub mod api_admin;
pub mod api_agent;
pub mod api_channels;
pub mod api_e2e;
pub mod api_federation;
pub mod api_graph;
pub mod api_invite;
pub mod api_link_preview;
pub mod api_observe;
pub mod api_rtx;
pub mod api_sse;
pub mod api_upload;
pub mod api_usernames;
pub mod api_vrp;
pub mod api_ws;
pub mod api_zk_circuits;
pub mod at_rest;
pub mod background;
pub mod config;
pub mod http;
pub mod middleware;
pub mod policy;
pub mod retention;
pub mod routes;
pub mod services;
pub mod startup;
pub mod state;
pub mod storage_health;
pub mod ws;

pub use routes::app;
pub use startup::{init_tracing, prepare_server, StartupError};
pub use state::AppState;

use tokio::sync::broadcast;

/// Emits an observe event to the database and broadcasts it to the SSE stream.
///
/// This is a convenience wrapper that calls
/// [`annex_observe::emit_event_signed`] — every production event row
/// carries an Ed25519 signature from the server's signing key
/// (ADR-0013) — and, on success, sends the resulting
/// [`annex_observe::PublicEvent`] through the broadcast channel.
/// Failures are logged as warnings but never block the caller.
pub fn emit_and_broadcast(
    conn: &rusqlite::Connection,
    server_id: i64,
    entity_id: &str,
    payload: &annex_observe::EventPayload,
    observe_tx: &broadcast::Sender<annex_observe::PublicEvent>,
    signing_key: &ed25519_dalek::SigningKey,
) {
    let domain = payload.domain();
    match annex_observe::emit_event_signed(
        conn,
        server_id,
        domain,
        payload.event_type(),
        payload.entity_type(),
        entity_id,
        payload,
        signing_key,
    ) {
        Ok(event) => {
            if let Err(e) = observe_tx.send(event) {
                tracing::warn!(
                    domain = domain.as_str(),
                    event_type = payload.event_type(),
                    "observe broadcast channel send failed (no receivers or lagged): {}",
                    e
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                domain = domain.as_str(),
                event_type = payload.event_type(),
                "failed to emit observe event: {}",
                e
            );
        }
    }
}

/// Parses a transfer scope string from the database into a [`VrpTransferScope`].
///
/// Returns `None` for unrecognized strings.
pub(crate) fn parse_transfer_scope(s: &str) -> Option<annex_vrp::VrpTransferScope> {
    s.parse().ok()
}
