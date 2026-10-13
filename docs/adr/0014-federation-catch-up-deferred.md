# ADR 0014 — Authenticated federation catch-up

Status: Implemented (2026-10-12). This amends the original deferred decision;
the filename remains stable for existing references.

## Context

A peer can remain disconnected beyond both the live envelope freshness window
and the outbox retry budget. Durable failed rows preserve the missed messages,
but live delivery correctly rejects their original timestamps. Recovery must
authenticate the requesting peer, preserve ordering across messages and edits,
record progress across restart, and propagate deletions without restoring body
content that the origin has already removed.

## Decision

`POST /api/federation/catch-up` authenticates an Ed25519-signed request containing
`requesting_server`, `channel_id`, `after_seq`, `max_envelopes` (1–64), a fresh
`nonce`, and `created_at`. Request timestamps are accepted for 120 seconds with
30 seconds of future skew. The peer must have an active transfer agreement and
an active attested member of the channel. Once a channel has been deleted, its
previously addressed outbox grants access only to removal markers.

The response includes `originating_server`, `requesting_server`, `channel_id`,
`nonce`, `after_seq`, `next_after_seq`, `status`, `entries`, and `signature`.
Each entry contains the durable outbox `seq`, original `message_id`, and either
an existing signed envelope or a `removed_reason` (`deleted` or `retention`).
The canonical signing inputs are JSON arrays implemented in
`services/federation_catchup.rs`, domain-separated with
`annex-catchup-request-v1` and `annex-catchup-response-v1`. The response signature
covers all entries, their ordering, the cursor, and the exact request nonce.
Responses are limited to 64 entries and 8 MiB at the receiver.

The earlier proposal required a v3 message envelope. This implementation signs
the sequence and complete original envelope in the enclosing recovery batch.
That supplies authenticated ordering for messages, edits, and removals while
preserving existing v1/v2 live-envelope compatibility. SQLite AUTOINCREMENT
outbox IDs provide a stable increasing sequence; IDs may have gaps. Cursors are
scoped to local server, peer, and channel, so an admission failure in one channel
cannot prevent another channel from recovering.

Every delivered envelope runs through the existing signature, peer agreement,
attestation, channel, membership, and receipt-ledger path. Only the authenticated
pull caller can select `DeliveryMode::Catchup`, exempting old timestamps while
retaining future-skew checks. Receipt rows record `delivery_mode = 'catchup'`.
A cursor advances only after the message or removal commits. A crash between
the commit and cursor update causes an idempotent replay on restart.

## Deletion and retention

Migration 048 adds indexed outbox routing metadata, persistent cursors and
recipient suppression tombstones. Startup backfills encrypted legacy rows before
launching retention workers and fails if it cannot do so safely.

Database triggers scrub outbox message and edit payloads on soft deletion and
hard deletion. They append a fresh ordered removal marker atomically with that
deletion, even if the original delivery's cursor has already been acknowledged.
Late enqueues consult the current source row and cannot restore its body. Live
redaction envelopes contain no body and remain eligible for prompt delivery.

The receiver persists removal markers, clears an existing copy only when it has
an original receipt from that peer, removes edit history, and refuses later
original-message deliveries covered by the marker. `history_pruned` explicitly
identifies retention loss. A durable `pruned_through_seq` is exposed to current
members at `GET /api/channels/{channelId}/federation-gaps` as
`[{"originating_server":"…","through_seq":123}]`. Connected members also receive
`federation_history_pruned` WebSocket events. Channel deletion scrubs outstanding
payloads and allows a peer with an existing mirror to recover removal markers.

## Operation and verification

A tracked background worker pulls immediately on startup and then at the
configured outbox interval. Failed admissions or transport calls retry without
advancing the cursor. Agreement revocation and channel eligibility are checked
again before each received entry. Persistent cursors and tombstones survive
process restarts.

`tests/federation_catchup.rs` exercises actual TCP HTTP between Annex routers:
expired failed deliveries, disk-backed restart and automatic recovery, missed
edits, deletion before and after delivery, delayed originals after deletion,
retention reporting, channel deletion, tampered signed sequences, nonce replay,
membership removal, and agreement revocation. These regression fixtures seed
admission state; separate real-session acceptance must prove handshake,
attestation, and join using generated membership proofs.
