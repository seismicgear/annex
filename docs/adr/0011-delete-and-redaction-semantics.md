# ADR 0011 — Delete + redaction semantics (federation-aware)

Status: Accepted — current behaviour documented; tombstone protocol deferred (2026-05-12)
Context tag: `hardening-pass`

## Context

`annex_channels::delete_message` performs a *local* soft delete: `deleted_at` is set, `content` is replaced with an empty string. The retention sweep later hard-deletes rows whose `expires_at` has passed. There is no federation-level redaction envelope. If a peer received the message (via the pre-hardening fan-out or the new outbox in ADR-0008), the peer keeps the message under the peer's own retention policy. There is no propagation, no tombstone, and no operator-facing signal that "delete locally means delete locally, not delete everywhere."

This contradicts the natural reader expectation that "delete a message" means "the network forgets it." The reviewer asked for either (A) loud documentation of current behaviour, or (B) implementation of signed tombstone envelopes.

## Decision

This pass implements **option A** (clear documentation). Option B is deferred with a concrete protocol sketch.

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
