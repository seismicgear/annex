# ADR 0013 — Tamper-evident public_event_log

Status: Accepted — hash chain shipped; signing deferred (2026-05-12)
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

- **Per-event Ed25519 signature.** Schema column is in place; the writer path needs to sign and the verifier path needs to verify. Will land in a follow-up that also documents the canonical signing input the way ADR-0007 documents the message envelope.
- **Chain export endpoint.** An auditor would benefit from `GET /api/public/events/chain?from_seq=N` that returns events + hashes + signatures so they can verify offline. Deferred.
