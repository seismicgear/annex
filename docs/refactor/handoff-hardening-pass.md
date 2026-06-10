# Hardening pass — handoff

Branch: `claude/add-architecture-diagrams-f1Kza`
Date: 2026-05-12

This document is the engineering handoff for the hardening pass that landed on this branch. It records what shipped, what is intentionally deferred (with concrete next steps), and what verification was run.

## Completed work

### 1. Stale docs reconciled
- **`docs/architecture/mermaid-diagrams.md`** — agent join sequence now shows `POST /api/ws/token` → `/ws?token=<…>` (preferred) with the legacy `/ws?pseudonym=…` path called out as enforce_zk_proofs-off only.
- **`docs/deployment.md`** — env var table now matches `crates/annex-server/src/config.rs` (`ANNEX_HOST`, `ANNEX_WEBRTC_*`); the LiveKit sidecar section is gone; new federation / storage / maintenance env vars are documented.
- **`docker-compose.yml`** — LiveKit sidecar removed; voice runs inside the Annex process via the native WebRTC SFU. Federation reliability + storage knobs added with defaults.
- **`docs/refactor/zk-merkle-production.md`** — epoch model entry marked implemented (migration 034 + `is_root_acceptable`).

### 2. WebSocket idempotency (ADR-0010)
- Migration `035_ws_request_idempotency.sql` adds `message_request_ids (server_id, channel_id, sender_pseudonym, client_request_id, message_id)` with `UNIQUE(server_id, sender_pseudonym, client_request_id)`.
- `ChannelService::send_message` now persists `client_request_id` and returns `SendOutcome::Replayed` on hit. Federate relay only fires on `Inserted` so replays don't double-enqueue.
- Tests: `crates/annex-server/tests/ws_idempotency.rs` (3 tests passing).

### 3. Federation replay ledger + freshness gate (ADR-0007)
- Migration `036_federation_receipts.sql` adds `federation_message_receipts (remote_instance_id, message_id, envelope_hash, envelope_created_at, received_at, delivery_mode)`.
- `receive_federated_message`:
  - Rejects `created_at` outside `[now-future_skew, now+freshness_window]` on v2 envelopes.
  - Rejects `(remote_instance_id, message_id)` with mismatching envelope hashes (same id, different signed body).
  - Treats benign duplicates (same id, same hash) as no-op.
- `Config::federation::{freshness_window_seconds, future_skew_seconds}` (defaults 300s / 60s) + env overrides.
- Tests: `crates/annex-server/tests/federation_freshness.rs` (8 unit tests passing).

### 4. Federation envelope versioning (ADR-0007)
- `FederatedMessageEnvelope.envelope_version: Option<String>` added; v1 (or absent) → legacy 7-line signing input; v2 → 8-line signing input prepended with the literal version string.
- `message_signing_input` and `message_envelope_hash` exported as `pub`.
- Default outbound envelope version is `v1` for one release (configurable via `ANNEX_FEDERATION_DEFAULT_ENVELOPE_VERSION`); flip to `v2` once receivers ship the verifier.

### 5. Durable federation outbox (ADR-0008)
- Migration `037_federation_outbox.sql` adds `federation_outbox` with `UNIQUE(peer_instance_id, message_id)`.
- `relay_message` now serialises the signed envelope once and inserts one row per active peer (the worker handles HTTP delivery).
- `background::start_federation_outbox_task` drains pending rows with bounded exponential backoff (`min(60 * 2^attempts, 3600)` seconds, capped at `outbox_max_attempts`).
- Storage gate interaction: enqueue failures from `SQLITE_FULL`/`SQLITE_IOERR` trip `storage_health`.

### 6. SQLite maintenance (ADR-0009)
- `background::start_db_maintenance_task` runs every `maintenance_interval_hours` (default 24h) and performs `PRAGMA wal_checkpoint(TRUNCATE)` + `ANALYZE`, optionally followed by `VACUUM` (off by default).
- `Config::storage::{warn_free_bytes, block_free_bytes, maintenance_enabled, maintenance_interval_hours, maintenance_vacuum}` + env overrides.

### 7. Storage health gate (ADR-0009)
- `crates/annex-server/src/storage_health.rs` — `StorageHealth { Healthy | Warn | Degraded }` `AtomicU8`.
- `storage_health::interpret_sqlite_error` trips the gate on `SQLITE_FULL`/`SQLITE_IOERR`.
- `storage_health::evaluate_db_file_size` for proactive warn/block when an operator wires a max-bytes cap.
- `auth_middleware` rejects mutating methods (POST/PUT/PATCH/DELETE) with HTTP 507 when the gate is `Degraded`.
- Unit tests in `storage_health.rs` (7 tests passing).

### 8. Migration integrity (ADR-0012)
- Migration `039_migration_checksums.sql` adds `sha256_hex` + `ordinal` columns to `_annex_migrations`.
- `run_migrations_from_list`:
  - Boot-time duplicate-ordinal scan (`MigrationError::DuplicateOrdinal`).
  - Integrity check against recorded SHA-256 (`MigrationError::ChecksumMismatch`).
  - Backfills `sha256_hex` for rows written before migration 039 — production deployments self-upgrade on first boot.
- Unit tests: `parse_ordinal`, duplicate-ordinal rejection, edit-after-apply rejection, idempotent rerun (4 passing).

### 9. Tamper-evident public_event_log (ADR-0013)
- Migration `038_event_log_hash_chain.sql` adds `prev_hash`, `event_hash`, `event_signature` columns; adds `UNIQUE INDEX (server_id, seq)`.
- `annex_observe::emit_event` computes the hash chain on insert; `prev_hash = "GENESIS"` for the first event.
- `annex_observe::verify_event_log_chain(conn, server_id)` returns the first inconsistent `seq` or `None` if intact.
- Unit tests: chain intact case + tamper-detection case (2 passing).

### 10. ADRs landed
- `docs/adr/0007-federation-delivery-and-replay.md`
- `docs/adr/0008-federation-outbox.md`
- `docs/adr/0009-storage-degradation-and-maintenance.md`
- `docs/adr/0010-ws-command-idempotency.md`
- `docs/adr/0011-delete-and-redaction-semantics.md` (current behaviour documented; tombstones deferred with protocol sketch)
- `docs/adr/0012-migration-integrity.md`
- `docs/adr/0013-event-log-tamper-evidence.md`
- `docs/adr/0014-federation-catch-up-deferred.md` (with prerequisites enumerated)

## Verification

Run inside `/home/user/annex`:

| Command | Result |
| --- | --- |
| `cargo check --workspace --exclude annex-desktop` | clean |
| `cargo fmt --all` | applied (6 files reformatted) |
| `cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings` | clean |
| `cargo test --workspace --exclude annex-desktop` | **650 passed, 0 failed** |

`annex-desktop` is excluded per the project's standing CLAUDE.md guidance (Tauri API version mismatch, pre-existing).

No frontend / E2E run in this pass — none of the changes touch `client/`. The new schema columns are server-only and the WS message protocol is wire-compatible (clients that omit `clientRequestId` continue to work; `envelope_version: None` on inbound federation envelopes is interpreted as v1).

## Files changed

### New files
- `crates/annex-db/src/migrations/035_ws_request_idempotency.sql`
- `crates/annex-db/src/migrations/036_federation_receipts.sql`
- `crates/annex-db/src/migrations/037_federation_outbox.sql`
- `crates/annex-db/src/migrations/038_event_log_hash_chain.sql`
- `crates/annex-db/src/migrations/039_migration_checksums.sql`
- `crates/annex-server/src/storage_health.rs`
- `crates/annex-server/tests/ws_idempotency.rs`
- `crates/annex-server/tests/federation_freshness.rs`
- `docs/adr/0007-federation-delivery-and-replay.md`
- `docs/adr/0008-federation-outbox.md`
- `docs/adr/0009-storage-degradation-and-maintenance.md`
- `docs/adr/0010-ws-command-idempotency.md`
- `docs/adr/0011-delete-and-redaction-semantics.md`
- `docs/adr/0012-migration-integrity.md`
- `docs/adr/0013-event-log-tamper-evidence.md`
- `docs/adr/0014-federation-catch-up-deferred.md`
- `docs/refactor/handoff-hardening-pass.md` (this file)

### Modified files
- `Cargo`-touching: `crates/annex-db/Cargo.toml` (+sha2), `crates/annex-observe/Cargo.toml` (+sha2)
- Runtime: `crates/annex-db/src/migrations.rs`, `crates/annex-observe/src/store.rs`, `crates/annex-observe/src/lib.rs`, `crates/annex-observe/src/tests.rs`
- `crates/annex-federation/src/types.rs` (envelope_version field + version constants), `crates/annex-federation/src/lib.rs` (re-exports)
- `crates/annex-server/src/config.rs` (FederationConfig + StorageConfig + env overrides)
- `crates/annex-server/src/state.rs` (new AppState fields)
- `crates/annex-server/src/middleware.rs` (storage gate short-circuit)
- `crates/annex-server/src/services/channel_service.rs` (`send_message` idempotency + `SendOutcome`)
- `crates/annex-server/src/services/federation_service.rs` (envelope-version dispatch, freshness gate, receipt ledger, outbox enqueue)
- `crates/annex-server/src/services/federation_repository.rs` (`ActiveFederationPeer.id`)
- `crates/annex-server/src/background.rs` (outbox worker + maintenance task)
- `crates/annex-server/src/startup.rs` (spawn new background tasks)
- `crates/annex-server/src/lib.rs` (`pub mod storage_health`)
- `crates/annex-server/src/ws/commands/message.rs` (passes `client_request_id`; skips relay on `Replayed`)
- `crates/annex-server/tests/common/mod.rs` + ~35 individual test files (AppState construction sites updated for new fields)
- `docs/architecture/mermaid-diagrams.md`, `docs/deployment.md`, `docker-compose.yml`, `docs/refactor/zk-merkle-production.md`

## Migrations added

| # | Name | Purpose |
| --- | --- | --- |
| 035 | `ws_request_idempotency` | `message_request_ids` table + indexes |
| 036 | `federation_receipts` | `federation_message_receipts` table + indexes |
| 037 | `federation_outbox` | `federation_outbox` table + composite index |
| 038 | `event_log_hash_chain` | `prev_hash`, `event_hash`, `event_signature` columns + `UNIQUE(server_id, seq)` |
| 039 | `migration_checksums` | `sha256_hex` + `ordinal` columns on `_annex_migrations` |

## Runtime behaviour changes

1. WS `IncomingMessage::Message` with a `clientRequestId` is now idempotent per `(server, sender, request_id)`. The first send creates a row; replays return the same `message_id`. The WS broadcast still fires on replays so the sender's pending-send promise resolves.
2. Federation relay is no longer fire-and-forget. Local commit enqueues an outbox row per peer; the background worker delivers with bounded exponential backoff (default ~3 hours total). Receivers are idempotent on the receipt ledger so retries are safe.
3. v2 federated message envelopes are freshness-checked against `Config::federation::{freshness_window_seconds, future_skew_seconds}` on the live receive path. Catch-up (when implemented — ADR-0014) will write through `delivery_mode = 'catchup'` to bypass the live window.
4. SQLite errors of class `SQLITE_FULL` / `SQLITE_IOERR` trip the storage gate. While `Degraded`, the auth middleware returns HTTP 507 on POST/PUT/PATCH/DELETE. Reads continue. Operator restarts the process (or the future admin-clear endpoint lands) to clear the gate.
5. `public_event_log` rows now carry `prev_hash` + `event_hash`. A tamper to one row is detectable by `verify_event_log_chain`.
6. The migration runner verifies SHA-256 of every embedded migration against the recorded hash on every boot. An edit to a committed migration after deploy is now a startup error rather than silently accepted.
7. Periodic SQLite maintenance (`wal_checkpoint(TRUNCATE)` + `ANALYZE`, optional `VACUUM`) runs every `maintenance_interval_hours` when `maintenance_enabled = true`.

## Remaining gaps (intentionally deferred)

These were either out of scope for this pass or have a concrete prerequisite that needs its own scoped task:

| Gap | Status | Why deferred | Next step |
| --- | --- | --- | --- |
| **Federation catch-up endpoint** | Deferred (ADR-0014) | Needs envelope `v3` with per-origin sequence number + receipt ledger schema extension | Land v3 envelope; add `channel_id` + `origin_seq` to `federation_message_receipts`; build endpoint |
| **Federated redaction tombstones** | Documented (ADR-0011) | Same shape as message envelope work; doing both in one pass risks correlated bugs | Build `FederatedRedactionEnvelope` + verifier following the sketch in ADR-0011 |
| **Admin endpoint to clear storage gate** | **Landed 2026-06-10** (ADR-0009 amendment) | — | `GET /api/admin/storage` + `POST /api/admin/storage/clear` (exempt from the degraded-gate 507; `can_moderate`-gated) |
| **Admin endpoint to inspect/retry outbox** | **Landed 2026-06-10** (ADR-0008 amendment) | — | `GET /api/admin/federation/outbox` (filter/pagination/counts) + `POST .../{id}/retry` (resets backoff budget; 409 on `pending`/`delivered`) |
| **Per-event Ed25519 signature** | **Landed 2026-06-10** (ADR-0013 amendment) | — | `emit_event_signed` signs `"annex-event-v1\n" + event_hash`; `verify_event_log_signatures` verifies against the recomputed hash; backfill clears signatures it cannot re-attest |
| **Idempotency TTL** | **Landed 2026-06-10** (ADR-0010 amendment) | — | Retention task prunes `message_request_ids` rows older than `server.idempotency_ttl_seconds` (default 7 days); index in migration 040; no new column needed (`created_at` already existed) |
| **Outbox per-peer rate limiting** | **Landed 2026-06-10** (ADR-0008 fairness amendment) | — | Window-function batch cap: ≤ `outbox_per_peer_batch` (default 8) rows per peer per tick; per-row exponential backoff already bounds retry rate |
| **Free-disk syscalls** | Skipped per brief | `libc`/`windows_sys` would add deps for a signal we already get reactively from `SQLITE_FULL` | Only revisit if reactive trip proves insufficient in practice |

## Things to know before the next pass

- **The outbox + receipt-ledger pair is the new contract.** Any new federation operation (RTX bundles, tombstones, attestations) should write to its own outbox row and be receipt-checked on receive. Don't `tokio::spawn` a bare HTTP POST.
- **The migration runner now requires that committed migration SQL stays bit-identical.** Pre-commit tooling to enforce this (a hook that hashes the embedded migrations and fails on diff against the last-committed checksum) is the natural next step but is out of scope here.
- **AppState gained three fields** (`federation_config`, `storage_config`, `storage_health`). Test files were updated en masse. Any test file added after this pass MUST include them; `crates/annex-server/tests/common/mod.rs::build_app_state` is the canonical source.
- **Outbound `envelope_version` defaults to `v1`.** When you flip it to `v2` (`ANNEX_FEDERATION_DEFAULT_ENVELOPE_VERSION=v2`), every peer that has not picked up the v2 verifier will reject your messages. The current rule is "ship the verifier in a release, wait one release, flip the sender."

## Commands run during this pass

```
cargo check --workspace --exclude annex-desktop
cargo test  --workspace --exclude annex-desktop -p annex-db --lib
cargo test  --workspace --exclude annex-desktop
cargo fmt   --all
cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings
```

Final state: 650 tests pass, 0 fail; clippy clean; fmt clean.
