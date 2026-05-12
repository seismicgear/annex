-- Migration 038 — tamper-evident public_event_log.
--
-- Why:
--   `public_event_log` was append-only by convention but not by
--   construction: `seq` was unique by virtue of a `MAX(seq)+1`
--   subquery inside an INSERT, with no UNIQUE constraint to catch
--   the race or a manual edit. There was no `prev_hash`, no
--   `event_hash`, and no signature. A server operator with shell
--   access to the SQLite file could rewrite history with no
--   detectable signal — which is acceptable under Annex's
--   sovereignty model, but the project's "everything is auditable"
--   claim required tampering to at least be *detectable* from a
--   mirrored or exported log.
--
-- This migration adds:
--
--   1. UNIQUE(server_id, seq) — turns the "two writers raced on
--      MAX(seq)" failure mode from "silent" into a constraint
--      violation the application code handles.
--
--   2. event_hash + prev_hash — SHA-256 hash chain. event_hash is
--      computed deterministically over the canonical fields
--      (server_id, seq, domain, event_type, entity_type, entity_id,
--      payload_json, occurred_at, prev_hash). prev_hash links to the
--      previous event's event_hash on the same server. The first
--      event's prev_hash is the literal string "GENESIS".
--
--   3. event_signature (nullable) — populated when a server signing
--      key is available at emit time. The verification path treats
--      a NULL signature as "this server did not sign older events"
--      rather than a failure, so deployments upgrading from a
--      pre-signature build don't lose their existing log.
--
-- The hash chain alone makes localized tampering detectable when a
-- consumer of the log (e.g. a federation peer mirroring events)
-- compares hashes. Signing adds non-repudiation against an attacker
-- who edits the entire chain in place.

ALTER TABLE public_event_log ADD COLUMN prev_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE public_event_log ADD COLUMN event_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE public_event_log ADD COLUMN event_signature TEXT;

-- UNIQUE(server_id, seq) — see annex-observe::store::emit_event.
-- The unique index is partial-emulated via a unique constraint on
-- (server_id, seq) since we cannot ALTER TABLE … ADD CONSTRAINT in
-- SQLite. We do the next-best thing: a UNIQUE INDEX.
CREATE UNIQUE INDEX idx_event_log_server_seq_unique
    ON public_event_log (server_id, seq);
