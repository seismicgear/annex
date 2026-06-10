# ADR 0008 — Durable federation outbox

Status: Accepted (2026-05-12); amended 2026-06-10 (admin inspect/retry endpoints landed)
Context tag: `hardening-pass`

## Context

`relay_message` previously fan-spawned `tokio::spawn` tasks per peer that POSTed the envelope and discarded the result on failure. Federation under any peer-side intermittency (transient network failure, peer restart, brief overload, slow proxy) lost envelopes with no retry and no visibility. The architecture documents called this out as a deliberate "best-effort" choice; the reviewer correctly observed that it is the highest-leverage reliability bug in the federation layer.

## Decision

Add a durable per-(peer, message) outbox.

1. **Schema** — `federation_outbox (peer_instance_id, message_id, envelope_json, status, attempts, next_retry_at, last_error, created_at, updated_at)` with `UNIQUE(peer_instance_id, message_id)` (migration `037_federation_outbox.sql`).
2. **Enqueue path** — `relay_message` builds the signed envelope exactly as before, serialises it once, then `INSERT OR IGNORE`s one row per active peer. The function returns immediately. UNIQUE makes duplicate enqueue idempotent.
3. **Worker** — `crate::background::start_federation_outbox_task` runs every `Config::federation::outbox_interval_seconds` (default 5s). Each tick pulls up to 32 pending rows whose `next_retry_at <= now`, POSTs each envelope, updates the row on the result.
4. **Backoff** — bounded exponential: `min(60 * 2^attempts, 3600)` seconds. The default `outbox_max_attempts = 12` gives roughly a 3-hour total retry window.
5. **Terminal states** — `delivered` on first 2xx; `failed` after `attempts >= max_attempts`. The worker does NOT delete `failed` rows; an operator can inspect them via `GET /api/admin/federation/outbox` and return them to the rotation via `POST /api/admin/federation/outbox/{id}/retry` (see amendment below).
6. **Storage gate interaction** — the enqueue path catches `SQLITE_FULL`/`SQLITE_IOERR` and trips the storage gate (see ADR-0009). The worker checkpoints WAL only via the maintenance task; it does not VACUUM under degraded storage.

## Consequences

- Fire-and-forget is gone. Local commit no longer waits on peer reachability, and a peer outage no longer means lost envelopes.
- Retries are safe because the receive path is idempotent (ADR-0007's receipt ledger).
- Visible state: an operator can query `federation_outbox WHERE status IN ('pending','failed')` to see the queue depth and stuck deliveries.
- Disk pressure: each pending envelope is held in JSON form until delivered. Operators with very large fan-out should size `block_free_bytes` accordingly.

## Out of scope (deferred)

- **Per-peer rate limiting.** ~~Currently each tick fans out to all pending rows. A misbehaving peer that consistently fails would burn HTTP requests against itself. A per-peer token bucket is the right shape; deferred.~~ *Landed 2026-06-10 as a per-peer batch cap* — see the fairness amendment below. A full token bucket remains unnecessary: the per-row exponential backoff already bounds each row's retry rate, and the batch cap bounds each peer's share of a tick.

## Amendment (2026-06-10) — per-peer fairness cap

The drain query now ranks each peer's due rows with a window function and keeps at most `Config::federation::outbox_per_peer_batch` (default 8, env `ANNEX_FEDERATION_OUTBOX_PER_PEER_BATCH`, floor 1) per peer per tick. Without the cap, one unreachable peer with a deep backlog of due rows filled the whole 32-row batch every tick (the global `ORDER BY next_retry_at` favours its oldest rows), starving healthy peers and burning the tick's entire HTTP fan-out against a host that is down. Test: `crates/annex-server/tests/federation_outbox_fairness.rs`.

## Amendment (2026-06-10) — admin inspect/retry endpoints

The originally-deferred admin surface landed:

- `GET /api/admin/federation/outbox` — lists outbox rows (most recent first) with an optional `status` filter (`pending` / `delivered` / `failed` / `paused`), `limit`/`offset` pagination, the peer's `base_url`/`label` joined in, and aggregate per-status counts for queue-depth triage. The envelope body is omitted (its byte size is reported as `envelope_bytes`).
- `POST /api/admin/federation/outbox/{id}/retry` — returns a `failed` or `paused` row to the retry rotation: `status='pending'`, `attempts=0` (fresh backoff budget), `next_retry_at=now`, `last_error=NULL`. Rows that are `pending` or `delivered` are rejected with 409. A retried row still passes the dequeue-time SSRF gate in the worker, so retrying a row whose peer URL points at a private host re-fails it terminally.

Both endpoints require `can_moderate` and emit a `MODERATION_ACTION` audit event on mutation. Tests: `crates/annex-server/tests/api_admin_storage_outbox.rs`.
