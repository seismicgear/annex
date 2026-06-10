# ADR 0011 — Delete + redaction semantics (federation-aware)

Status: Accepted (2026-05-12); amended 2026-06-10 — option B (tombstone protocol) implemented
Context tag: `hardening-pass`

## Context

`annex_channels::delete_message` performs a *local* soft delete: `deleted_at` is set, `content` is replaced with an empty string. The retention sweep later hard-deletes rows whose `expires_at` has passed. There is no federation-level redaction envelope. If a peer received the message (via the pre-hardening fan-out or the new outbox in ADR-0008), the peer keeps the message under the peer's own retention policy. There is no propagation, no tombstone, and no operator-facing signal that "delete locally means delete locally, not delete everywhere."

This contradicts the natural reader expectation that "delete a message" means "the network forgets it." The reviewer asked for either (A) loud documentation of current behaviour, or (B) implementation of signed tombstone envelopes.

## Decision

This pass implements **option A** (clear documentation). Option B is deferred with a concrete protocol sketch. *(Option B has since been implemented — see the 2026-06-10 amendment at the bottom.)*

### Current behaviour (documented, enforced by code)

1. Local delete is local. `delete_message` writes `deleted_at` and blanks `content` on the originating server only.
2. Federated peers that have already received the message keep it. The peer's retention policy is the only mechanism that eventually removes it.
3. Local retention is a *time-based* eviction (`expires_at <= now`), not a propagation signal.

### Why option B is deferred

Implementing tombstones correctly requires three pieces that are not yet in place:

1. A canonical signing input for a tombstone (analogous to `message_signing_input` for messages).
2. A receive-path verification flow that proves the redactor had authority — minimally, the same `attestation_ref` as the original message, signed by the same originating server's signing key.
3. Outbox + replay-ledger integration so a tombstone retries durably and can be safely re-delivered to a peer that already saw it.

(1) and (2) are roughly the same size as the message-envelope work in ADR-0007. (3) reuses the outbox machinery from ADR-0008. The risk of doing this in the same pass as the federation reliability rework is that a bug in the redaction envelope verifier could mask a bug in the message envelope verifier, and vice versa. Splitting them into two passes is safer.

### Sketch of the deferred tombstone protocol

```text
FederatedRedactionEnvelope {
    envelope_version: "v1",
    message_id: <original>,
    channel_id: <original>,
    originating_server: <original sender's server>,
    redacted_by: <pseudonym of redactor>,
    redaction_reason: <enum: deleted | moderation | requested>,
    attestation_ref: <redactor's attestation>,
    created_at: <RFC 3339>,
    signature: <Ed25519 over canonical input>,
}
```

Receiver behaviour:

- Verify signature against the originating server's published key.
- Verify the redactor's `attestation_ref` proves they have authority (same pseudonym as the message sender, or a moderator capability on the channel).
- Blank `content` on the local row, set `deleted_at = received_at`, keep `message_id` + `created_at` + `sender_pseudonym` for audit.
- Insert a receipt-ledger row so re-delivery is idempotent (same trick as ADR-0007).

## Consequences

- Operators and reviewers now have a clear statement of what `delete` does and does not do, instead of inferring it from code.
- A future change can implement tombstones without requiring a wire-format break — the protocol shape is fixed in this ADR.
- The existing `redacted_topics` field in the VRP capability contract continues to govern RTX *topic* filtering; it is unrelated to message redaction and is not renamed.

## Amendment (2026-06-10) — tombstone protocol implemented

Option B landed, following the sketch above with these concretions:

- **Envelope**: `FederatedRedactionEnvelope` in `annex-federation` with an `envelopeKind: "redaction"` discriminator (message envelopes have no such field) and `envelopeVersion: "v1"`. The Ed25519 signing input is newline-joined and prefixed with the domain-separation literal `annex-redaction-v1`, so a redaction signature can never verify as a message envelope or vice versa (`federation_service::redaction_signing_input`).
- **Sender path**: a WS `delete_message` on a `FEDERATED`-scoped channel enqueues one signed tombstone per active peer into the existing federation outbox (`relay_redaction`, mirroring `relay_message` including the enqueue-time transfer-scope and SSRF filters). Outbox rows are keyed `redaction:<message_id>` so they cannot collide with the original message's row under `UNIQUE(peer_instance_id, message_id)`. The outbox worker routes rows to `POST /api/federation/redactions` by peeking at `envelopeKind`.
- **Receiver path** (`POST /api/federation/redactions` → `receive_federated_redaction`): instance + agreement gates, always-on freshness window, signature verification, then two authority checks: (1) *origin authority* — a receipt for the original message from the same peer must exist, so only the delivering server can redact, never for locally-authored or third-party-delivered messages; (2) *redactor authority* — `redacted_by` must equal the stored `sender_pseudonym`, except `reason: "moderation"` which is accepted on the origin's signature alone (the channel lives on the originating server; its moderators govern it).
- **Effect + idempotency**: `content` blanked and `deleted_at` set (audit fields kept), committed atomically with a receipt row keyed `redaction:<message_id>` (IMMEDIATE transaction, same rationale as the message-path atomicity fix). Replays with a matching envelope hash are benign no-ops; hash mismatches are rejected. A `MessageDeleted` frame is broadcast to local subscribers, mirroring the local-delete flow.
- **Retention interplay**: a tombstone for a message already hard-deleted by retention records the receipt and reports `applied: false` — nothing to blank, replays stay idempotent.

Tests: `crates/annex-server/tests/api_federation_redaction.rs` (8 integration tests) plus outbox-routing unit tests in `background.rs`.
