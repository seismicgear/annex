# ADR 0013 — Tamper-evident public_event_log

Status: Accepted — hash chain shipped (2026-05-12); per-event signing shipped (2026-06-10)
Context tag: `hardening-pass`

## Context

`public_event_log` was append-only by *convention*: the `seq` column got `MAX(seq)+1` inside the INSERT, no UNIQUE constraint, no hash chain, no signature. A server operator with shell access to the SQLite file could rewrite history with no detectable signal.

That is acceptable under Annex's sovereignty model — the operator owns their server — but the project's "everything is auditable" claim (`docs/refactor/invariants.md`, README's invariant 5) is meaningful only if tampering is at minimum *detectable* by a peer or auditor consuming the log.

## Decision

1. **Schema** (migration `038_event_log_hash_chain.sql`) — add `prev_hash`, `event_hash`, `event_signature` columns; add `UNIQUE(server_id, seq)` via a unique index. The unique index turns concurrent-writer races on `MAX(seq)` from "silent duplicate seq" into a constraint violation the caller handles.
2. **Hash chain** — `event_hash = SHA-256(canonical(server_id, seq, domain, event_type, entity_type, entity_id, payload_json, occurred_at, prev_hash))`. The first event's `prev_hash` is the literal `"GENESIS"`. Each subsequent event's `prev_hash` is the previous event's `event_hash` on the same server.
3. **Verification function** — `annex_observe::verify_event_log_chain(conn, server_id)` walks the rows in seq order and returns the first inconsistent seq (or `None` if the chain is intact). Operators / federation peers consume this to assert integrity.
4. **Signature column** — `event_signature` is nullable for now. The signing key is already on `AppState`; wiring the signature into `emit_event` is small but requires a second pass to verify the signing-input canonical form is correct under all edge cases (empty payload, very long payload, non-ASCII content). Deferred to a follow-up ADR.

## Consequences

- A localised tamper (edit one row) is detectable by anyone who has a copy of the chain — the recomputed hash diverges from the recorded one at the tampered row, and every subsequent row's `prev_hash` is wrong.
- A full chain rewrite by an attacker who has the SQLite file is undetectable by hash alone. Signing (deferred) closes this — once events are signed by the server key, a chain rewrite would require the key.
- The unique index catches `MAX(seq)` races. Concurrent writers now serialise through the constraint instead of producing silent duplicate sequence numbers.

## Out of scope (deferred)

- **Chain export endpoint.** An auditor would benefit from `GET /api/public/events/chain?from_seq=N` that returns events + hashes + signatures so they can verify offline. Deferred.

## Amendment (2026-06-10) — per-event Ed25519 signatures

The deferred signing pass landed:

1. **Canonical signing input** — `"annex-event-v1\n" + event_hash_hex` (`annex_observe::event_signing_input`). The `annex-event-v1` literal is a domain-separation prefix: the server's signing key also signs federation envelopes (ADR-0007), and the prefix makes the two signature domains non-interchangeable. Signing the canonical hash rather than the raw fields keeps the input fixed-length and inherits the hash's field ordering — including `prev_hash`, so the signature binds the event's chain position. Edge cases (empty payload, very long payload, non-ASCII content) are absorbed by the hash computation, which operates on the exact bytes stored in the row.
2. **Writer** — `annex_observe::emit_event_signed` signs every row; `annex-server`'s `emit_and_broadcast` (the funnel for all production emission) now requires the signing key, so every live event row carries a 64-byte hex-encoded signature. The unsigned `emit_event` remains for tooling/tests.
3. **Verifier** — `annex_observe::verify_event_log_signatures(conn, server_id, verifying_key)` returns the first seq whose signature is present but invalid. The signature is checked against the *recomputed* canonical hash, so one pass catches both "fields edited, hash stale" and the attack the hash chain alone cannot see: a full chain rewrite by an attacker with file access, who can recompute every hash but cannot re-sign without the key. Rows with NULL signatures (pre-signing legacy, or rows rewritten by `backfill_event_log_chain`) are skipped.
4. **Backfill interaction** — `backfill_event_log_chain` clears `event_signature` on rows it rewrites: re-signing rewritten history with the current key would let a repair pass attest content it cannot vouch for. An auditor sees backfilled rows as unsigned, which is the honest state.

Tests: `crates/annex-observe/src/tests.rs` (signed-emit round-trip, full-rewrite detection, wrong-key rejection, legacy-row skip, backfill clearing) and `crates/annex-server/tests/observe_integration.rs::handler_emitted_events_carry_verifiable_signatures` (end-to-end through a real handler).
