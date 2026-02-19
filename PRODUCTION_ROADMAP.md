# Production Readiness Roadmap

**Standard**: System must run correctly for 100 years unattended.
**Date**: 2026-02-19
**Source**: Comprehensive audit of all `.rs` files, SQL migrations, tests, and operational requirements.
**Depends on**: PRODUCTION_GAP_REPORT.md (129 items: 87 fixed, 42 acknowledged)

This roadmap covers everything that remains between the current codebase and a system that an operator can deploy and walk away from. Each session is scoped to fit in a single working session (2-4 hours of AI-assisted work). Sessions within a phase can be parallelized. Phases must be sequential.

---

## Current State

```
Phase 12: Hardening & Audit ............ COMPLETE
Production Gap Report .................. 87/129 fixed, 42 acknowledged, 0 open

Phase A: Operational Infrastructure .... NOT STARTED
Phase B: Authentication & Authorization  NOT STARTED
Phase C: Data Integrity & Recovery ..... NOT STARTED
Phase D: Resilience Under Load ......... NOT STARTED
Phase E: Test Coverage Gaps ............ NOT STARTED
Phase F: Operational Documentation ..... NOT STARTED
```

---

## Phase A: Operational Infrastructure

**Why first**: Without metrics and health checks, every subsequent fix is invisible. You can't verify improvements you can't measure.

### Session A.1 — Prometheus Metrics Endpoint

**Scope**: Add `/metrics` endpoint with core counters, gauges, and histograms.

**Work**:
- [ ] Add `metrics` and `metrics-exporter-prometheus` (or `axum-prometheus`) to `annex-server/Cargo.toml`
- [ ] Create `crates/annex-server/src/metrics.rs` module with:
  - Counter: `annex_http_requests_total` (method, path, status)
  - Histogram: `annex_http_request_duration_seconds` (method, path)
  - Gauge: `annex_db_pool_connections_active`
  - Gauge: `annex_db_pool_connections_idle`
  - Counter: `annex_db_pool_timeout_total`
  - Counter: `annex_ws_connections_total`
  - Gauge: `annex_ws_connections_active`
  - Counter: `annex_messages_total` (channel_type)
  - Counter: `annex_retention_deleted_total`
  - Counter: `annex_pruning_pruned_total`
  - Counter: `annex_federation_messages_relayed_total`
  - Counter: `annex_broadcast_lag_total` (channel: presence | observe)
- [ ] Add metrics middleware layer to Axum router (request count + latency)
- [ ] Instrument `pool.rs` to expose pool stats
- [ ] Instrument `retention.rs` deletion counter
- [ ] Instrument `background.rs` pruning counter
- [ ] Instrument `api_ws.rs` connection counter
- [ ] Add `GET /metrics` route (unprotected, for Prometheus scraping)
- [ ] Test: start server, make requests, scrape `/metrics`, verify counters increment

**Files touched**: `Cargo.toml`, `lib.rs`, new `metrics.rs`, `pool.rs`, `retention.rs`, `background.rs`, `api_ws.rs`

### Session A.2 — Readiness & Liveness Probes

**Scope**: Replace basic `/health` with proper Kubernetes-compatible probes.

**Work**:
- [ ] `GET /healthz` — Liveness: returns 200 if process is running (trivial)
- [ ] `GET /readyz` — Readiness: returns 200 only if:
  - Database pool can acquire a connection within 1s
  - `SELECT 1` succeeds on the acquired connection
  - Merkle tree is initialized (root exists)
  - Background tasks are alive (not panicked)
- [ ] Add `AtomicBool` flags to `AppState` for background task health
- [ ] Set flags to `false` if retention or pruning task exits unexpectedly
- [ ] Readiness check reads these flags
- [ ] Keep existing `GET /health` for backwards compatibility (alias to `/healthz`)
- [ ] Test: start server → `/readyz` returns 200 → kill pool → `/readyz` returns 503

**Files touched**: `lib.rs`, `main.rs`, `background.rs`, `retention.rs`

### Session A.3 — Database & Disk Monitoring

**Scope**: Detect and alert on resource exhaustion before it causes failures.

**Work**:
- [ ] Add `PRAGMA journal_size_limit = 67108864` (64 MiB WAL cap) to pool connection init
- [ ] Add periodic health check task (runs every 60s):
  - `PRAGMA integrity_check(1)` — quick single-page check
  - `SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()` — database file size
  - Check WAL file size via `std::fs::metadata` on `{db_path}-wal`
  - Check available disk space via `statvfs` (or `fs2` crate)
- [ ] Expose as Prometheus gauges:
  - `annex_db_size_bytes`
  - `annex_db_wal_size_bytes`
  - `annex_disk_available_bytes`
  - `annex_db_integrity_ok` (1 = ok, 0 = failed)
- [ ] Log at WARN if disk < 1 GiB available
- [ ] Log at ERROR if disk < 100 MiB available
- [ ] Log at ERROR if WAL > 64 MiB (checkpoint may be failing)
- [ ] Test: verify gauges are populated after startup

**Files touched**: `pool.rs`, `background.rs` or new `health_monitor.rs`, `metrics.rs`

### Session A.4 — Request Tracing & Correlation IDs

**Scope**: Add request IDs for debugging distributed interactions.

**Work**:
- [ ] Add middleware that generates `X-Request-Id` (UUID v4) for each request
- [ ] If incoming request has `X-Request-Id`, use that (for federation tracing)
- [ ] Inject request ID into tracing span (`tracing::Span::current()`)
- [ ] Include request ID in all log lines for that request
- [ ] Include request ID in error responses
- [ ] Return `X-Request-Id` in response headers
- [ ] Test: send request without ID → get ID in response; send request with ID → same ID returned

**Files touched**: `middleware.rs`, `lib.rs`

---

## Phase B: Authentication & Authorization

**Why second**: Auth is the largest acknowledged gap (C-01). Everything else works correctly only if callers are who they claim to be.

### Session B.1 — Signed Request Authentication

**Scope**: Replace plaintext pseudonym bearer tokens with Ed25519 signed requests for protected endpoints.

**Work**:
- [ ] Design auth token format: `timestamp:pseudonym:signature` where signature = `Ed25519Sign(sk, timestamp + ":" + pseudonym + ":" + method + ":" + path)`
- [ ] Client derives signing key from their secret key (same `sk` used for commitment)
- [ ] Store public key in `platform_identities` during verification (Phase 2 endpoint)
- [ ] Add migration: `ALTER TABLE platform_identities ADD COLUMN public_key_hex TEXT`
- [ ] Update auth middleware to:
  1. Extract token from `Authorization: Bearer` header
  2. Parse `timestamp:pseudonym:signature`
  3. Reject if timestamp > 5 minutes old (replay protection)
  4. Lookup public key from `platform_identities`
  5. Verify Ed25519 signature
  6. Proceed if valid
- [ ] Maintain backwards compatibility: accept old-style pseudonym tokens with deprecation warning log
- [ ] Update WebSocket auth to use signed token in query parameter instead of raw pseudonym
- [ ] Test: valid signature → 200; expired timestamp → 401; wrong key → 401; old-style token → 200 + warn log

**Files touched**: `middleware.rs`, `api.rs`, new migration, `api_ws.rs`, `lib.rs`

### Session B.2 — Protect VRP Agent Handshake Endpoint

**Scope**: Close the critical gap where `/api/vrp/agent-handshake` accepts unauthenticated requests.

**Work**:
- [ ] Move `/api/vrp/agent-handshake` behind auth middleware
- [ ] Verify `payload.pseudonym_id == identity.pseudonym_id` in handler
- [ ] Return 403 if pseudonym doesn't match
- [ ] Update all agent handshake tests to include auth headers
- [ ] Test: unauthenticated request → 401; mismatched pseudonym → 403; correct auth → 200

**Files touched**: `lib.rs` (route move), `api_vrp.rs`, test files

### Session B.3 — Protect Graph Profile Endpoint

**Scope**: Replace forgeable `X-Annex-Viewer` header with authenticated viewer identity.

**Work**:
- [ ] Move `/api/graph/profile/{targetPseudonym}` behind auth middleware
- [ ] Use `IdentityContext` from middleware as the viewer (replace header extraction)
- [ ] Remove `X-Annex-Viewer` header parsing
- [ ] Update graph profile tests
- [ ] Test: unauthenticated → 401; authenticated → correct visibility filtering

**Files touched**: `lib.rs`, `api_graph.rs`, test files

### Session B.4 — Token Expiration & Rotation

**Scope**: Add session tokens with TTL so compromised pseudonyms can't be used indefinitely.

**Work**:
- [ ] Add `session_tokens` table: `token_hash TEXT, pseudonym_id TEXT, expires_at TEXT, created_at TEXT`
- [ ] Issue session token on successful ZK verification (POST `/api/zk/verify-membership`)
- [ ] Token TTL: 24 hours (configurable via server policy)
- [ ] Auth middleware checks token expiration
- [ ] `POST /api/auth/refresh` — issue new token if current is valid but within 1 hour of expiry
- [ ] `POST /api/auth/revoke` — invalidate a session token
- [ ] Background task: clean up expired tokens periodically
- [ ] Test: fresh token → 200; expired token → 401; revoked token → 401; refresh → new token

**Files touched**: new migration, `api.rs`, `middleware.rs`, `lib.rs`, new `api_auth.rs`

---

## Phase C: Data Integrity & Recovery

**Why third**: Once you can measure (Phase A) and authenticate (Phase B), you need to ensure data survives failures.

### Session C.1 — Transaction Coverage for All Write Paths

**Scope**: Wrap all remaining non-transactional multi-statement writes in transactions.

**Work**:
- [ ] `annex-channels/src/lib.rs`:
  - `create_channel()` — wrap INSERT in transaction (currently single statement, but future-proof for multi-table)
  - `delete_channel()` — wrap the 3 DELETEs (messages, members, channel) in a single transaction
- [ ] `annex-graph/src/lib.rs`:
  - `update_node_activity()` — wrap the two UPDATEs in a transaction to prevent TOCTOU
  - `ensure_graph_node()` — verify INSERT OR IGNORE + UPDATE is atomic
- [ ] `annex-channels/src/lib.rs`:
  - `create_message()` — single INSERT, already atomic, but verify `resolve_retention_days` doesn't race
- [ ] Audit all remaining `conn.execute()` calls that aren't in transactions — wrap any multi-statement paths
- [ ] Test: simulate failure between statements → verify no partial state

**Files touched**: `annex-channels/src/lib.rs`, `annex-graph/src/lib.rs`

### Session C.2 — Database Backup System

**Scope**: Implement automated SQLite backup using the SQLite Online Backup API.

**Work**:
- [ ] Add backup module to `annex-db`: `backup.rs`
- [ ] Implement `create_backup(source_pool, dest_path)` using `rusqlite::backup::Backup`
  - Non-blocking: copies pages in batches (100 pages per step, 10ms sleep between)
  - Progress logging
  - Verify backup integrity with `PRAGMA integrity_check` on destination
- [ ] Add admin endpoint: `POST /api/admin/backup` (requires `can_moderate`)
  - Triggers manual backup to configured path
  - Returns backup file size and duration
- [ ] Add periodic backup task (configurable interval, default: 24 hours)
  - Backup destination: `{db_path}.backup.{timestamp}`
  - Retain last N backups (configurable, default: 7)
  - Delete older backups automatically
- [ ] Add config options: `backup.enabled`, `backup.interval_hours`, `backup.retain_count`, `backup.destination_dir`
- [ ] Add Prometheus counter: `annex_backup_completed_total`, `annex_backup_failed_total`, gauge: `annex_backup_last_success_epoch`
- [ ] Test: create data → backup → corrupt original → restore from backup → verify data

**Files touched**: new `annex-db/src/backup.rs`, `config.rs`, `main.rs`, `lib.rs`, new `api_admin.rs` endpoint

### Session C.3 — Persistent Signing Key Enforcement

**Scope**: Make `ANNEX_SIGNING_KEY` mandatory for non-development mode. Close H-10.

**Work**:
- [ ] Add `server.mode` config option: `development` | `production` (default: `development`)
- [ ] In `production` mode:
  - Require `ANNEX_SIGNING_KEY` env var (fail startup if missing)
  - Require `database.path` is not `:memory:`
  - Require `backup.enabled = true`
  - Log warning if `logging.json = false`
- [ ] In `development` mode:
  - Generate ephemeral key with warning (current behavior)
  - Allow in-memory database
  - Backup optional
- [ ] Persist the public key to database on first startup; verify on subsequent startups
  - If stored key doesn't match loaded key, log CRITICAL and refuse to start (prevents accidental key rotation)
- [ ] Document key generation: `openssl genpkey -algorithm ed25519 | openssl pkey -outform DER | base64`
- [ ] Test: production mode without key → startup fails; with key → startup succeeds; key mismatch → startup fails

**Files touched**: `config.rs`, `main.rs`, new migration for stored public key

### Session C.4 — WAL Checkpoint Strategy

**Scope**: Ensure WAL files don't grow unbounded and checkpoints happen reliably.

**Work**:
- [ ] Set `PRAGMA wal_autocheckpoint = 1000` (checkpoint every 1000 pages, ~4 MiB)
- [ ] Set `PRAGMA journal_size_limit = 67108864` (truncate WAL to 64 MiB after checkpoint)
- [ ] Add periodic manual checkpoint in health monitor (every 5 minutes):
  - `PRAGMA wal_checkpoint(PASSIVE)` — doesn't block writers
  - Log checkpoint result (busy pages, checkpointed pages)
- [ ] Add Prometheus gauge: `annex_db_wal_checkpointed_pages`, `annex_db_wal_busy_pages`
- [ ] Test: write 10,000 rows → verify WAL doesn't exceed limit → verify checkpoint runs

**Files touched**: `pool.rs`, `health_monitor.rs` or `background.rs`

---

## Phase D: Resilience Under Load

**Why fourth**: With monitoring, auth, and backup in place, harden against load-related failures.

### Session D.1 — Graceful Shutdown with Cancellation

**Scope**: Ensure all background tasks stop cleanly on SIGTERM.

**Work**:
- [ ] Add `tokio_util::sync::CancellationToken` to `AppState`
- [ ] Pass token to all background tasks (retention, pruning, health monitor, backup)
- [ ] Each task checks `token.is_cancelled()` before each iteration
- [ ] On SIGTERM: cancel token → wait up to 30s for tasks to finish → force exit
- [ ] Retention task: if mid-batch when cancelled, finish current batch then stop
- [ ] Pruning task: if mid-prune when cancelled, finish current operation then stop
- [ ] Log "shutting down gracefully" with task completion status
- [ ] Test: start server with background tasks → send SIGTERM → verify tasks complete within timeout

**Files touched**: `main.rs`, `retention.rs`, `background.rs`, `lib.rs`

### Session D.2 — Connection Pool Resilience

**Scope**: Add circuit breaker and backpressure for database pool exhaustion.

**Work**:
- [ ] Add pool health state to `AppState`: `pool_healthy: AtomicBool`
- [ ] When `pool.get()` times out, set `pool_healthy = false`
- [ ] When `pool.get()` succeeds after failure, set `pool_healthy = true`
- [ ] Readiness probe (`/readyz`) checks `pool_healthy`
- [ ] When pool is unhealthy:
  - Return 503 immediately for new requests (don't queue behind 5s timeout)
  - Log at ERROR level with current pool stats
  - Increment `annex_db_pool_timeout_total` counter
- [ ] Retention task: if pool acquisition fails, back off exponentially (1s, 2s, 4s, 8s, 16s, max 60s)
- [ ] Pruning task: same exponential backoff
- [ ] Test: exhaust pool → verify 503 responses → release pool → verify recovery

**Files touched**: `pool.rs` or wrapper, `retention.rs`, `background.rs`, `middleware.rs`

### Session D.3 — Retention Batching Improvements

**Scope**: Prevent retention from holding a database connection for extended periods.

**Work**:
- [ ] Change retention to acquire/release pool connection per batch (not per run)
  - Current: get conn → loop { delete 5k } → release conn
  - New: loop { get conn → delete 5k → release conn → yield }
- [ ] Add configurable inter-batch delay: `retention.batch_delay_ms` (default: 100ms)
- [ ] Add maximum batches per run: `retention.max_batches_per_run` (default: 100 = 500K messages)
- [ ] Add metrics: `annex_retention_batch_duration_seconds` histogram
- [ ] Log total messages deleted and total duration per run
- [ ] Test: insert 50K expired messages → run retention → verify all deleted in batches → verify no long lock hold

**Files touched**: `retention.rs`, `config.rs`, `metrics.rs`

### Session D.4 — VoiceIntent TOCTOU Fix

**Scope**: Fix the race condition where an agent can publish to a voice channel after removal.

**Work**:
- [ ] In VoiceIntent handler (`api_ws.rs`), after TTS synthesis and before voice room connection:
  - Re-check channel membership with fresh DB query
  - If no longer a member, return error and skip voice connection
- [ ] Hold the voice_sessions write lock for the entire connect-or-reject operation (already partially done)
- [ ] Add test: agent joins channel → start VoiceIntent → remove agent from channel concurrently → verify agent cannot connect to voice room
- [ ] Add test: agent in channel → VoiceIntent succeeds → verify audio published

**Files touched**: `api_ws.rs`, new test file `tests/voice_toctou_test.rs`

---

## Phase E: Test Coverage Gaps

**Why fifth**: After fixing the code, verify the fixes work under adversarial conditions.

### Session E.1 — Pool Exhaustion & Error Recovery Tests

**Work**:
- [ ] Test: exhaust all 8 pool connections → send HTTP request → verify 503 (not hang)
- [ ] Test: exhaust pool → retention task gracefully backs off → pool recovers → retention resumes
- [ ] Test: corrupt database JSON (policy, capability contract) → verify graceful fallback
- [ ] Test: `PRAGMA integrity_check` detects corruption → health endpoint returns unhealthy

### Session E.2 — Load & Concurrency Tests

**Work**:
- [ ] Test: 100 concurrent WebSocket connections, each sending 10 msg/sec for 30s → verify no OOM, no deadlocks
- [ ] Test: concurrent message creation + retention deletion on same channel → verify no lost messages in flight
- [ ] Test: 50 concurrent federation relays → verify all delivered with correct signatures
- [ ] Test: rate limiter under 10,000 concurrent requests → verify no counter underflow/overflow

### Session E.3 — Federation Security Edge Cases

**Work**:
- [ ] Test: replay attack — same federation message ID sent twice → verify deduplication
- [ ] Test: truncated signature hex → verify rejection (not panic)
- [ ] Test: empty signature → verify rejection
- [ ] Test: signature from unknown instance → verify rejection
- [ ] Test: key rotation — old signature with new key in DB → verify rejection
- [ ] Test: concurrent attestation deletion + message relay → verify consistent state

### Session E.4 — Shutdown & Lifecycle Tests

**Work**:
- [ ] Test: SIGTERM during active retention batch → verify batch completes, no partial state
- [ ] Test: SIGTERM during active WebSocket connections → verify clean disconnect messages
- [ ] Test: server startup with corrupt database → verify clear error message
- [ ] Test: server startup with missing ZK key → verify clear error message
- [ ] Test: server startup with wrong ANNEX_SIGNING_KEY → verify rejection

---

## Phase F: Operational Documentation

**Why last**: Document what actually exists after all changes are made.

### Session F.1 — Deployment & Operations Guide

**Work**:
- [ ] Document production configuration checklist:
  - Required: `ANNEX_SIGNING_KEY`, `server.mode = production`, database path, backup config
  - Recommended: Prometheus scrape config, alerting rules, log aggregation
  - Optional: LiveKit, voice models, federation peers
- [ ] Document backup & restore procedure step-by-step
- [ ] Document signing key generation and rotation procedure
- [ ] Document database migration procedure (automatic, but document manual steps if needed)
- [ ] Document monitoring & alerting setup:
  - Prometheus scrape target configuration
  - Key metrics to alert on (with thresholds)
  - Grafana dashboard template (JSON)

### Session F.2 — Runbooks

**Work**:
- [ ] Runbook: "Server is returning 503" → check pool health, check disk space, check WAL size
- [ ] Runbook: "Federation messages not relaying" → check signing key, check agreement status, check remote health
- [ ] Runbook: "Agents disconnecting unexpectedly" → check policy changes, check alignment recalculation, check pool health
- [ ] Runbook: "Database growing too large" → check retention config, check WAL checkpoint, manual VACUUM
- [ ] Runbook: "Server won't start" → check config, check signing key, check database integrity, check ZK keys
- [ ] Runbook: "Restore from backup" → step-by-step with verification

### Session F.3 — Audit Log Completeness

**Work**:
- [ ] Add observe events for: channel create/update/delete, member join/leave, message deletion (retention)
- [ ] Add observe events for: backup completed/failed, pool exhaustion, WAL checkpoint
- [ ] Verify all 13 existing event types still emit correctly
- [ ] Document complete event catalog with payload schemas
- [ ] Test: perform every auditable action → query event log → verify complete trail

---

## Dependency Graph

```
Phase A (Operational Infrastructure)
  ├── A.1 Metrics ─────────────┐
  ├── A.2 Health Probes ───────┤
  ├── A.3 Disk Monitoring ─────┤── All independent within phase
  └── A.4 Correlation IDs ─────┘

Phase B (Authentication) ← depends on Phase A (need metrics to verify)
  ├── B.1 Signed Requests ─────┐
  ├── B.2 Protect VRP ─────────┤── B.2 and B.3 depend on B.1
  ├── B.3 Protect Graph ───────┤
  └── B.4 Token Expiration ────┘── depends on B.1

Phase C (Data Integrity) ← depends on Phase A (need monitoring)
  ├── C.1 Transactions ────────┐
  ├── C.2 Backup System ───────┤── All independent within phase
  ├── C.3 Signing Key ─────────┤
  └── C.4 WAL Checkpoint ──────┘

Phase D (Resilience) ← depends on Phase A (metrics) + Phase C (transactions)
  ├── D.1 Graceful Shutdown ───┐
  ├── D.2 Pool Resilience ─────┤── All independent within phase
  ├── D.3 Retention Batching ──┤
  └── D.4 TOCTOU Fix ─────────┘

Phase E (Tests) ← depends on Phases B, C, D (test the fixes)
  ├── E.1 Pool Tests ──────────┐
  ├── E.2 Load Tests ──────────┤── All independent within phase
  ├── E.3 Federation Tests ────┤
  └── E.4 Lifecycle Tests ─────┘

Phase F (Documentation) ← depends on all previous phases
  ├── F.1 Deployment Guide ────┐
  ├── F.2 Runbooks ────────────┤── All independent within phase
  └── F.3 Audit Completeness ──┘
```

---

## Estimated Effort

| Phase | Sessions | Hours (est.) | Blocks Production |
|-------|----------|-------------|-------------------|
| **A: Operational Infrastructure** | 4 | 8-12 | YES |
| **B: Authentication** | 4 | 10-16 | YES (C-01) |
| **C: Data Integrity** | 4 | 8-12 | YES |
| **D: Resilience** | 4 | 8-12 | Partially |
| **E: Test Coverage** | 4 | 6-10 | No |
| **F: Documentation** | 3 | 4-8 | No |
| **Total** | **23 sessions** | **44-70 hours** | |

---

## What This Roadmap Does NOT Cover

These are acknowledged design decisions that require architectural changes beyond hardening:

1. **Multi-node / HA deployment** — SQLite is single-writer. Scaling beyond one node requires PostgreSQL migration or a replication layer. This is a different project.
2. **Semantic alignment embeddings** — Phase 3.3 deferred model integration. Requires ML infrastructure decisions.
3. **Client-side E2E encryption** — Messages are plaintext in the database. Adding E2E encryption changes the entire message flow.
4. **Distributed rate limiting** — Current rate limiter is per-process. Multiple instances don't share state.
5. **Real-time voice latency optimization** — TTS/STT model selection and GPU acceleration are deployment-specific.

---

## Rules for Working Through This Roadmap

1. **Complete Phase A first.** Without metrics, you can't verify anything else works.
2. **Sessions within a phase can run in parallel** unless noted otherwise.
3. **Every session must pass `cargo clippy -- -D warnings` and `cargo test`** before marking complete.
4. **Do not skip sessions.** If a session is blocked, document why and move to a parallel session.
5. **Update this document** when sessions complete. Add a dated entry to the changelog below.
6. **Do not add scope to sessions.** If new work is discovered, add it as a new session at the end of the appropriate phase.

---

## Changelog

| Date | Change |
|------|--------|
| 2026-02-19 | Roadmap created from comprehensive 4-audit analysis (auth, ops, data integrity, test coverage). |
