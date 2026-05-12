# ADR 0009 — Storage degradation + SQLite maintenance

Status: Accepted (2026-05-12)
Context tag: `hardening-pass`

## Context

Pre-hardening, a disk-full event bubbled up as a generic HTTP 500. There was no preflight, no read-only degradation, no health indicator. Operators saw "internal server error" indistinguishable from a bug. There was also no scheduled SQLite maintenance — no `VACUUM`, no `ANALYZE`, no WAL checkpoint — so a long-running deployment's DB file would grow without bound and the WAL could pin disk forever.

## Decision

Three small additions, each scoped to the existing architecture:

1. **`StorageHealth` gate** (`crates/annex-server/src/storage_health.rs`) — an `Arc<StorageHealth>` on `AppState` carrying a `Healthy` / `Warn` / `Degraded` atomic state. Reactive trip on `SQLITE_FULL` / `SQLITE_IOERR` via `interpret_sqlite_error`. Optional proactive evaluation against a configured DB-file-size cap via `evaluate_db_file_size`. No automatic recovery: an operator clears the gate explicitly after they have verified the underlying cause. Auto-recovery would flap under transient I/O errors.
2. **Auth middleware short-circuit** — when the gate is `Degraded` and the request method is `POST`/`PUT`/`PATCH`/`DELETE`, the auth middleware returns `HTTP 507 Insufficient Storage` immediately after auth resolution. Reads still flow.
3. **Maintenance task** (`background::start_db_maintenance_task`) — when `Config::storage::maintenance_enabled = true`, runs every `maintenance_interval_hours`:
   - `PRAGMA wal_checkpoint(TRUNCATE);` — checkpoints + truncates WAL.
   - `ANALYZE;` — rebuilds planner stats.
   - `VACUUM;` only if `maintenance_vacuum = true` AND the gate is healthy (VACUUM transiently needs disk equal to the DB size).

## Why not cross-platform free-disk inspection

The portable signal we already have is the DB file size (`std::fs::metadata`) plus the engine's own `SQLITE_FULL` error. Adding `libc` / `windows_sys` for `statvfs` / `GetDiskFreeSpaceExW` would couple Annex to a platform abstraction and add a dependency for a signal we get reactively from SQLite anyway. The brief explicitly says "Do not over-engineer cross-platform disk inspection if it gets ugly."

## Consequences

- Disk-full is now visible to operators as a typed HTTP 507 / a typed log line, not as a 500.
- Writers fail-fast under degraded storage; reads continue.
- WAL file growth is bounded by the maintenance interval (default 24h).
- `VACUUM` is opt-in (`maintenance_vacuum = true`) because it blocks writers.

## Out of scope (deferred)

- **Admin endpoint to clear the gate** (`POST /api/admin/storage/clear`). For now, an operator restarts the process after freeing disk. This is enough for the hardening pass; the admin endpoint is queued behind the broader admin-surface ADR.
- **Free-disk preflight via syscall.** Not adding `libc`/`windows_sys` in this pass; the reactive trip path is enough for the operator scenarios we have evidence for.
- **Per-write storage gate inside `tokio::task::spawn_blocking` bodies.** Currently the gate is consulted at the HTTP/WS boundary. Internal writes (e.g. the retention sweep) call `interpret_sqlite_error` on their own errors, but do not pre-check the gate. This is intentional: maintenance work should still attempt to run when storage is degraded because it can free space.
