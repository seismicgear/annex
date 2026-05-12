# ADR 0007 — Federation delivery + replay defence

Status: Accepted (2026-05-12)
Context tag: `hardening-pass`

## Context

Pre-hardening, `relay_message` in `crates/annex-server/src/services/federation_service.rs` posted to every active peer via `tokio::spawn` with no retry, no outbox, no record of the attempt. The receive path verified Ed25519 signatures and `UNIQUE(message_id)`, but did not enforce envelope freshness and could not distinguish a benign retry from a forged envelope that reused a captured `message_id` with a mutated body. The architecture diagrams correctly described this as "best-effort delivery, not eventual consistency."

The honest review by the external reviewer ("Alwyn") flagged three concrete failure modes:

1. **Cross-server replay.** A peer that captures a `FederatedMessageEnvelope` can replay it to a *different* receiving server which has not seen its `message_id`. The signature is still valid; the receiver has no other replay key.
2. **Same-id, different-body forgery.** Anyone with key access to the originating server can mint a new envelope claiming the same `message_id` as a real one and present it to a peer that hasn't received the original yet.
3. **Stale-envelope delivery.** A captured envelope held for hours or days then re-released is accepted as a live message because `created_at` is signed but not checked.

## Decision

Land a coordinated change with two server-side surfaces and one wire-format change:

1. **Receipt ledger** — new table `federation_message_receipts (remote_instance_id, message_id, envelope_hash, envelope_created_at, received_at, delivery_mode)` with `UNIQUE(remote_instance_id, message_id)`. The receiver INSERTs a row before persisting the message; a same-id-different-hash presentation is rejected with `403 Forbidden`. A same-id-same-hash presentation is a benign retry — no error, no broadcast.
2. **Freshness gate** — `Config::federation::{freshness_window_seconds, future_skew_seconds}` (defaults 300s / 60s). Envelopes that opt into envelope_version `v2` are time-checked on the live path. Catch-up (`delivery_mode = 'catchup'`) is exempt by design.
3. **Envelope versioning** — `FederatedMessageEnvelope.envelope_version: Option<String>`. v1 (or absent) → legacy 7-line signing input. v2 → 8-line signing input prepended with the literal version string, freshness-checked on receive. v1 and v2 signing inputs hash to different values so downgrade/upgrade attacks invalidate signatures.

The signed payload now binds `envelope_version`, `message_id`, `channel_id`, `content`, `sender_pseudonym`, `originating_server`, `attestation_ref`, and `created_at`. Receivers compute the SHA-256 of the signing input as the envelope hash for receipt-ledger lookup.

The default outbound `envelope_version` stays at `v1` for one release so receivers ship the v2 verifier before senders flip the default. The flip is a one-line config change (`ANNEX_FEDERATION_DEFAULT_ENVELOPE_VERSION=v2`).

## Consequences

- Cross-server replay is blocked by the receipt ledger.
- Stale-envelope delivery via the live path is blocked by the freshness gate.
- Same-id, different-body presentations are blocked by the envelope-hash check.
- Wire format remains backward-compatible: v1 peers keep working until they upgrade.
- Federation transport is now durable: see ADR-0008 for the outbox.

## Out of scope (deferred)

- **Per-origin monotonic sequence number.** The receipt ledger handles replay and tampering; a sequence number gives stronger ordering guarantees but adds a coordinated wire-format change for what is currently a non-blocking issue. Deferred until a follow-up envelope_version (`v3`).
- **Mutual key rotation envelope.** When a peer rotates its Ed25519 key, the receipt ledger correctly rejects new envelopes signed by the new key until the operator updates the `instances.public_key_hex`. The rotation handshake is a separate piece of work.
