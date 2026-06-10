# ADR 0010 — WebSocket command idempotency via `clientRequestId`

Status: Accepted (2026-05-12); amended 2026-06-10 (TTL eviction landed)
Context tag: `hardening-pass`

## Context

The WebSocket `IncomingMessage::Message` arm accepted an optional `clientRequestId` from the client but only echoed it in the broadcast for sender-side pending-send correlation. The server generated a fresh `message_id` (UUIDv4) on every accepted send. A replay of the same WS frame — whether from a hijacked session, a flaky network retrying client, or an in-TLS MitM — produced a *new* row with a *new* `message_id`.

For chat that is annoying. For AI agents that act on chat content (the "replay pay me $100" scenario) it is dangerous.

## Decision

Persist `clientRequestId` on first accept; on repeat, return the original message instead of inserting a new row.

1. **Schema** (migration `035_ws_request_idempotency.sql`) — `message_request_ids (server_id, channel_id, sender_pseudonym, client_request_id, message_id)` with `UNIQUE(server_id, sender_pseudonym, client_request_id)`.
2. **Scope** — per `(server_id, sender_pseudonym, client_request_id)`. Two different senders may share a `client_request_id` without affecting each other. Two requests from the same sender with the same `client_request_id` collapse to the same `message_id`.
3. **Service path** — `ChannelService::send_message` now takes `Option<String> client_request_id` and returns `(Message, bool, SendOutcome)` where `SendOutcome` is `Inserted` or `Replayed`. The transaction is: lookup → if hit, hydrate and return; otherwise INSERT message + INSERT request mapping in one tx.
4. **WS arm behaviour** — broadcast on both `Inserted` and `Replayed` (so the sender's pending-send promise resolves even on retry). Federate-relay only on `Inserted` — replay must not double-enqueue into the outbox.
5. **Missing `clientRequestId`** — backwards compatible: no idempotency, each send produces a new row. Clients that want the guarantee opt in.

## Consequences

- A WS replay inside an authenticated session no longer duplicates messages.
- An AI agent receiving channel messages cannot be tricked by simple replay; it sees the same `message_id` and can dedupe.
- A racing pair of identical sends inserts at most one row (the second sees the unique-constraint conflict and falls back to the lookup branch).

## Out of scope (deferred)

- **Generalised idempotency for non-message commands** (edit, delete, channel join). The same shape applies: scope by `(server_id, sender, request_id)`, persist the result identifier. Edit and delete already operate on a fixed `message_id`, so replaying them is more naturally idempotent against the underlying message state; a future ADR can cover the channel-join replay surface if needed.
- **Time-bound idempotency window.** ~~Currently the table grows forever (modulo the existing retention sweep on parent messages — see `crates/annex-server/src/retention.rs`). A future task adds a TTL on `message_request_ids` so a stale request_id from a year ago does not silently collide with a fresh one.~~ *Landed 2026-06-10:* the retention task now prunes `message_request_ids` rows older than `server.idempotency_ttl_seconds` (default 604800 = 7 days, floor 60, env override `ANNEX_IDEMPOTENCY_TTL_SECONDS`) in the same batched-DELETE shape as the message sweep (`annex_channels::prune_expired_request_ids`, index added in migration 040). After eviction a replayed `clientRequestId` is treated as a new send — the TTL exceeds any realistic client retry window by orders of magnitude, and a stale request id from months ago no longer collides with a fresh one.
