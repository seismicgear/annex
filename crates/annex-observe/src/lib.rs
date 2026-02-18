//! Observability layer for the Annex platform.
//!
//! Implements the public event log, real-time SSE event streaming, and
//! public summary APIs. This is the "trust as public computation" layer:
//! every identity operation, federation change, agent lifecycle event, and
//! moderation action is recorded in an append-only event log that any
//! authorized party can query and audit.
//!
//! # Event domains
//!
//! Events are categorised into five domains:
//!
//! | Domain | Example events |
//! |--------|---------------|
//! | `IDENTITY` | `IDENTITY_REGISTERED`, `IDENTITY_VERIFIED`, `PSEUDONYM_DERIVED` |
//! | `PRESENCE` | `NODE_ADDED`, `NODE_PRUNED`, `NODE_REACTIVATED` |
//! | `FEDERATION` | `FEDERATION_ESTABLISHED`, `FEDERATION_REALIGNED`, `FEDERATION_SEVERED` |
//! | `AGENT` | `AGENT_CONNECTED`, `AGENT_REALIGNED`, `AGENT_DISCONNECTED` |
//! | `MODERATION` | `MODERATION_ACTION` |
//!
//! # Usage
//!
//! ```rust,ignore
//! use annex_observe::{emit_event, EventDomain, EventPayload};
//!
//! emit_event(
//!     &conn,
//!     server_id,
//!     EventDomain::Identity,
//!     "IDENTITY_REGISTERED",
//!     "identity",
//!     &commitment_hex,
//!     &EventPayload::IdentityRegistered {
//!         commitment_hex: commitment_hex.clone(),
//!         role_code: 1,
//!     },
//! )?;
//! ```

mod error;
mod event;
mod store;

pub use error::ObserveError;
pub use event::{EventDomain, EventPayload, ParseEventDomainError, PublicEvent};
pub use store::{emit_event, next_seq, query_events, EventFilter};

#[cfg(test)]
mod tests;
