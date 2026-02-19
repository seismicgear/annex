# Annex Production Gap Report

**Standard**: System must run correctly for 100 years unattended.
**Date**: 2026-02-18
**Last Audit**: 2026-02-18
**Codebase**: ~14,500 lines of Rust across 11 crates
**Method**: Line-by-line audit of every `.rs` file, every SQL migration, every test, every `Cargo.toml`

---

## Summary

| Severity | Total | Fixed | Acknowledged | Open |
|----------|-------|-------|--------------|------|
| CRITICAL | 14 | 12 | 2 | 0 |
| HIGH     | 28 | 24 | 4 | 0 |
| MODERATE | 37 | 28 | 9 | 0 |
| LOW      | 28 | 12 | 16 | 0 |
| NITPICK  | 11 | 0 | 11 | 0 |
| **Total** | **118** | **76** | **42** | **0** |

Legend: **Fixed** = code change applied. **Acknowledged** = known limitation, design decision, or deferred to future phase.

---

## CRITICAL (14) — 12 Fixed, 2 Acknowledged

These will cause data corruption, security breaches, or system failure.

### C-01: Pseudonym-as-bearer-token authentication — ACKNOWLEDGED
- **File**: `crates/annex-server/src/middleware.rs:20-45`
- **Status**: Known design limitation. Real authentication is deferred to a future phase. The current pseudonym-based authentication is documented as a placeholder in the middleware.
- **Impact**: Anyone who knows a pseudonym can impersonate that user. Pseudonyms are deterministically derived and appear in public event logs, SSE streams, and message histories. The entire auth model is effectively public.

### C-02: No request body size limit — FIXED
- **File**: `crates/annex-server/src/lib.rs`
- **Fix**: Added `MAX_REQUEST_BODY_BYTES` constant (1 MiB) and `DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES)` layer to the Axum router.

### C-03: Observe event sequence number race condition — FIXED
- **File**: `crates/annex-observe/src/store.rs`
- **Fix**: `emit_event` now uses an atomic `INSERT ... SELECT COALESCE(MAX(seq), 0) + 1` subquery, eliminating the read-then-write race.

### C-04: Merkle tree / database state divergence on failed persist — FIXED
- **File**: `crates/annex-identity/src/merkle.rs`
- **Fix**: `insert_and_persist()` now uses `preview_insert()` to calculate changes without modifying the in-memory tree. The tree is only updated via `apply_updates()` after the database transaction commits successfully.

### C-05: Merkle tree state leak on registration error after DB commit — FIXED
- **File**: `crates/annex-identity/src/registry.rs`
- **Fix**: `register_identity()` now uses `preview_insert()` pattern. Tree state is only applied after successful DB commit. Proof generation happens on the committed state.

### C-06: O(N*M) brute-force scan in federation message relay — FIXED
- **File**: `crates/annex-server/src/api_federation.rs`, `crates/annex-db/src/migrations/024_nullifier_lookup_columns.sql`
- **Fix**: Added denormalized `pseudonym_id` and `commitment_hex` columns to `zk_nullifiers` with an indexed lookup (migration 024). `find_commitment_for_pseudonym()` now uses O(1) indexed path with fallback to legacy scan only for unmigrated rows.

### C-07: SSRF + no timeout on federation attestation HTTP request — FIXED
- **File**: `crates/annex-server/src/api_federation.rs`
- **Fix**: Created `federation_http_client()` factory with 10s connect timeout, 30s total timeout, and `redirect::Policy::none()` to prevent SSRF via redirects. All federation HTTP calls use this client.

### C-08: No timeout on federation message relay HTTP requests — FIXED
- **File**: `crates/annex-server/src/api_federation.rs`
- **Fix**: All relay and RTX forwarding now uses the same `federation_http_client()` with timeouts and redirect protection.

### C-09: Non-atomic federation agreement deactivation + insertion — FIXED
- **File**: `crates/annex-federation/src/db.rs`
- **Fix**: `create_agreement()` now wraps the deactivation UPDATE and new INSERT in a savepoint transaction.

### C-10: IDENTITY_VERIFIED audit event emitted before validation completes — FIXED
- **File**: `crates/annex-server/src/api.rs`
- **Fix**: `IDENTITY_VERIFIED` event is now emitted only after public signal validation succeeds.

### C-11: Missing G1/G2 curve point validation in ZK proof parsing — FIXED
- **File**: `crates/annex-identity/src/zk.rs`
- **Fix**: Added `validate_g1()` and `validate_g2()` functions that verify points are on the curve and in the correct prime-order subgroup. All `parse_g1`/`parse_g2` calls now validate before returning. Tests added for off-curve rejection.

### C-12: `expect()` reachable in production WebSocket voice handler — FIXED
- **File**: `crates/annex-server/src/api_ws.rs`
- **Fix**: Replaced `expect()` with `Entry` pattern that safely handles both insert and existing cases without panic paths.

### C-13: Corrupted capability contract silently propagated to graph metadata — FIXED
- **File**: `crates/annex-server/src/api.rs`
- **Fix**: Now returns `ApiError::InternalServerError` if `capability_contract_json` fails to deserialize, preventing corrupt data propagation.

### C-14: Unauthenticated federation endpoints on public router — ACKNOWLEDGED
- **File**: `crates/annex-server/src/lib.rs`
- **Status**: Known design decision. Federation endpoints are authenticated via Ed25519 signature verification on the request payload rather than HTTP-level auth middleware. The `get_federated_channels_handler` returns only public channel metadata.

---

## HIGH (28) — 24 Fixed, 4 Acknowledged

These will cause outages, data loss, or exploitable behavior under load or over time.

### H-01: ConnectionManager deadlock risk from inconsistent lock ordering — ACKNOWLEDGED
- **File**: `crates/annex-server/src/api_ws.rs`
- **Status**: Lock ordering has been documented and is consistent. The documented acquisition order is: sessions -> user_subscriptions -> channel_subscriptions.

### H-02: Unbounded mpsc channel per WebSocket connection — FIXED
- **File**: `crates/annex-server/src/api_ws.rs`
- **Fix**: Replaced `mpsc::unbounded_channel()` with `mpsc::channel(256)`. Slow clients that fill the buffer are disconnected.

### H-03: `std::sync::RwLock` held in async context blocks tokio runtime — ACKNOWLEDGED
- **File**: `crates/annex-server/src/api_ws.rs`
- **Status**: Documented as intentional design choice. The locks are held for very short durations (HashMap lookups/inserts) and `std::sync::RwLock` avoids the overhead of `tokio::sync::RwLock` for these fast operations.

### H-04: Missing transaction in RTX publish handler — FIXED
- **File**: `crates/annex-server/src/api_rtx.rs`
- **Fix**: Bundle insert and initial transfer log are now wrapped in a single transaction. Delivery logs for individual subscribers remain outside the transaction (non-critical, logged with warning on failure).

### H-05: Missing transaction in federation RTX receive handler — FIXED
- **File**: `crates/annex-server/src/api_federation.rs`
- **Fix**: Bundle insert, transfer log, and subscriber delivery logs are now wrapped in a single transaction.

### H-06: Missing transaction in federation attestation handler — FIXED
- **File**: `crates/annex-server/src/api_federation.rs`
- **Fix**: The three writes (federated_identities, platform_identities, ensure_graph_node) are now wrapped in a single transaction with rollback on failure.

### H-07: Missing transaction in VRP handshake handler — FIXED
- **File**: `crates/annex-server/src/api_vrp.rs`
- **Fix**: record_vrp_outcome, check_reputation_score, and agent_registrations upsert are now wrapped in a single transaction. Events are emitted after commit.

### H-08: Missing transaction in policy re-evaluation (agents) — FIXED
- **File**: `crates/annex-server/src/policy.rs`
- **Fix**: All agent alignment updates are now performed within a single transaction, committed atomically.

### H-09: Missing transaction in policy re-evaluation (federation) — FIXED
- **File**: `crates/annex-server/src/policy.rs`
- **Fix**: All federation agreement updates are now performed within a single transaction, committed atomically.

### H-10: Ephemeral signing key regenerated on every restart — ACKNOWLEDGED
- **File**: `crates/annex-server/src/main.rs`
- **Status**: Warning logged when ephemeral key is generated. The `ANNEX_SIGNING_KEY` environment variable must be set for production deployments.

### H-11: Background task panics silently swallowed — FIXED
- **File**: `crates/annex-server/src/main.rs`
- **Fix**: Background task `JoinHandle`s are now monitored with `tokio::select!`. If any task completes unexpectedly (panic or cancellation), a critical error is logged.

### H-12: Broadcast send failure silently drops observe events — FIXED
- **File**: `crates/annex-server/src/lib.rs`
- **Fix**: Broadcast send failures are now logged at warn level. The persistent audit log write (database INSERT) is the primary durability mechanism; broadcast is best-effort for real-time subscribers.

### H-13: Rate limiter thundering-herd bypass — FIXED
- **File**: `crates/annex-server/src/middleware.rs`
- **Fix**: Replaced the `clear()` approach with a sliding window that uses `retain()` to evict only expired entries. No mass reset occurs.

### H-14: Rate limiter 2x burst at window boundary — FIXED
- **File**: `crates/annex-server/src/middleware.rs`
- **Fix**: Replaced fixed-window counter with sliding window counter that tracks individual request timestamps, preventing boundary bursts.

### H-15: `touch_activity` spawned on every WebSocket message with no debounce — FIXED
- **File**: `crates/annex-server/src/api_ws.rs`
- **Fix**: Activity touches are now debounced using a timestamp comparison. Only fires if more than 30 seconds have elapsed since the last touch.

### H-16: Read-modify-write race in channel update — FIXED
- **File**: `crates/annex-channels/src/lib.rs`
- **Fix**: `update_channel()` now builds a dynamic `UPDATE ... SET` statement that only modifies the fields present in the update request, in a single atomic SQL statement. No read-modify-write cycle.

### H-17: No timeout on external process execution (STT) — FIXED
- **File**: `crates/annex-voice/src/stt.rs`
- **Fix**: Added 120-second timeout (`STT_TIMEOUT_SECS`) on the whisper subprocess. Process is killed if it exceeds the timeout.

### H-18: No timeout on external process execution (TTS) — FIXED
- **File**: `crates/annex-voice/src/tts.rs`
- **Fix**: Added 60-second timeout (`TTS_TIMEOUT_SECS`) on the piper subprocess. Process is killed if it exceeds the timeout.

### H-19: No input size limit on audio data piped to STT — FIXED
- **File**: `crates/annex-voice/src/stt.rs`
- **Fix**: Added `MAX_AUDIO_INPUT_BYTES` (10 MiB) limit. Input exceeding the limit is rejected before spawning the subprocess.

### H-20: No input size limit on TTS text — FIXED
- **File**: `crates/annex-voice/src/tts.rs`
- **Fix**: Added `MAX_TTS_TEXT_BYTES` (64 KiB) limit. Text exceeding the limit is rejected before spawning the subprocess.

### H-21: Secret key silently reduced modulo field order — ACKNOWLEDGED
- **File**: `crates/annex-identity/src/commitment.rs`
- **Status**: Known cryptographic limitation of BN254 field arithmetic. The probability of collision (keys congruent mod field order) is astronomically low for properly generated random keys.

### H-22: Unknown EdgeKind silently defaults to `Connected` — FIXED
- **File**: `crates/annex-graph/src/lib.rs`
- **Fix**: `str_to_edge_kind()` now returns `Result<EdgeKind, GraphError>` and returns an error for unrecognized strings instead of silently defaulting.

### H-23: Unknown NodeType silently defaults to `Human` — FIXED
- **File**: `crates/annex-graph/src/lib.rs`
- **Fix**: `str_to_node_type()` now returns `Result<NodeType, GraphError>` and returns an error for unrecognized strings instead of silently defaulting.

### H-24: Conflict agents not updated in database after VRP handshake — FIXED
- **File**: `crates/annex-server/src/api_vrp.rs`
- **Fix**: When a VRP handshake results in `Conflict`, the agent's DB record is now explicitly updated to `alignment_status = 'Conflict'`, `active = 0`.

### H-25: `filter_map(Result::ok)` silently drops DB read errors — FIXED
- **File**: `crates/annex-server/src/api_federation.rs`
- **Fix**: `find_commitment_for_pseudonym()` now returns `Result<Option<...>, rusqlite::Error>` and properly propagates all database errors instead of swallowing them.

### H-26: Missing FK CASCADE on `messages.channel_id` — FIXED
- **File**: `crates/annex-channels/src/lib.rs`
- **Fix**: `delete_channel()` now explicitly deletes child rows (messages, channel_members) before deleting the channel. SQLite cannot add CASCADE via ALTER TABLE, so explicit deletion is the correct approach.

### H-27: Missing FK CASCADE on `channel_members.channel_id` — FIXED
- **File**: `crates/annex-channels/src/lib.rs`
- **Fix**: Same as H-26. `delete_channel()` deletes channel_members before the channel. Tests verify cascading cleanup.

### H-28: Missing index on `graph_edges` table — FIXED
- **File**: `crates/annex-db/src/migrations/023_production_indexes.sql`
- **Fix**: Added indexes `idx_graph_edges_from(server_id, from_node)`, `idx_graph_edges_to(server_id, to_node)`, and `idx_graph_edges_unique_triple(server_id, from_node, to_node, kind)` UNIQUE constraint.

---

## MODERATE (37) — 28 Fixed, 9 Acknowledged

These cause degraded behavior, confusing errors, or performance cliffs.

### M-01: No CORS configuration — FIXED
- **File**: `crates/annex-server/src/lib.rs`
- **Fix**: Added `CorsLayer::permissive()` to the Axum router.

### M-02: Three inconsistent error response patterns — FIXED
- **File**: `crates/annex-server/src/api_channels.rs`, `api.rs`, `api_federation.rs`
- **Fix**: `ApiError` and `FederationError` both return structured JSON error bodies. `FederationError` now maps client-caused errors to appropriate 4xx status codes instead of 500.

### M-03: No upper bound on message history `limit` parameter — FIXED
- **File**: `crates/annex-server/src/api_channels.rs`
- **Fix**: History limit capped at 200 via `.min(200)`. Channel message list capped at 100.

### M-04: No upper bound on BFS `max_depth` parameter — FIXED
- **File**: `crates/annex-server/src/api_graph.rs`
- **Fix**: Added `MAX_BFS_DEPTH` constant (10). Requests exceeding this limit return 400 Bad Request.

### M-05: SSE presence stream silently drops lagged events — ACKNOWLEDGED
- **File**: `crates/annex-server/src/api_sse.rs`
- **Status**: Known limitation. SSE is best-effort delivery. Clients can use cursor-based polling on the event log for guaranteed delivery.

### M-06: SSE observe stream silently drops lagged events — ACKNOWLEDGED
- **File**: `crates/annex-server/src/api_observe.rs`
- **Status**: Same as M-05. The persistent event log provides guaranteed delivery.

### M-07: Signature payload concatenation without delimiters (federation messages) — FIXED
- **File**: `crates/annex-server/src/api_federation.rs`
- **Fix**: All signature payloads now use newline (`\n`) delimiters between fields.

### M-08: Signature payload concatenation without delimiters (RTX relay) — FIXED
- **File**: `crates/annex-server/src/api_federation.rs`
- **Fix**: RTX relay signing payload now uses newline delimiters via `rtx_relay_signing_payload()`.

### M-09: Signature payload concatenation without delimiters (attestation) — FIXED
- **File**: `crates/annex-server/src/api_federation.rs`
- **Fix**: Attestation signature now uses `format!("{}\n{}\n{}", topic, commitment, participant_type)`.

### M-10: Signature payload concatenation without delimiters (RTX bundle signing) — FIXED
- **File**: `crates/annex-rtx/src/validation.rs`
- **Fix**: `bundle_signing_payload()` now uses newline delimiters: `format!("{}\n{}\n{}\n{}\n{}", ...)`.

### M-11: No input validation on user-supplied strings anywhere — FIXED
- **File**: `crates/annex-server/src/api_channels.rs`, `api_ws.rs`
- **Fix**: Channel creation validates name length (1-200 chars), channel_id format, and topic length. WebSocket message content validated with size limit.

### M-12: `add_session` silently replaces existing WebSocket sessions — FIXED
- **File**: `crates/annex-server/src/api_ws.rs`
- **Fix**: When a pseudonym reconnects, the old session's subscriptions are explicitly cleaned up before the new session is established.

### M-13: Unknown `participant_type` silently defaults to Human in federation — FIXED
- **File**: `crates/annex-server/src/api_federation.rs`
- **Fix**: Unknown participant types now return an error instead of silently defaulting to Human.

### M-14: `retention.rs` pool failure returns `Ok(0)` instead of error — FIXED
- **File**: `crates/annex-server/src/retention.rs`
- **Fix**: Pool connection failure now returns a real error (mapped to `rusqlite::Error::SqliteFailure`) that propagates to the error handling loop.

### M-15: Merkle tree depth hardcoded to 20 — FIXED
- **File**: `crates/annex-server/src/config.rs`
- **Fix**: Merkle tree depth is now configurable via `server.merkle_tree_depth` in config.toml (default: 20).

### M-16: Presence broadcast channel capacity hardcoded to 100 — FIXED
- **File**: `crates/annex-server/src/config.rs`
- **Fix**: Presence broadcast capacity is now configurable via `server.presence_broadcast_capacity` in config.toml (default: 256).

### M-17: `retention_check_interval_seconds` can be set to 0 — FIXED
- **File**: `crates/annex-server/src/config.rs`
- **Fix**: Added `MIN_RETENTION_CHECK_INTERVAL_SECONDS = 1` validation. Values below 1 are rejected at config load time.

### M-18: `INSERT OR IGNORE` silently swallows all constraint violations in `add_member` — FIXED
- **File**: `crates/annex-channels/src/lib.rs`
- **Fix**: Replaced `INSERT OR IGNORE` with plain `INSERT`. Error handling now distinguishes UNIQUE constraint violations (idempotent OK) from FK violations (propagated as error) using SQLite extended error codes.

### M-19: Server policy deserialization failure in `resolve_retention_days` has no fallback — FIXED
- **File**: `crates/annex-channels/src/lib.rs`
- **Fix**: On deserialization failure, falls back to `ServerPolicy::default()` with a `tracing::warn!` log, ensuring message creation never fails due to corrupt policy JSON.

### M-20: Commitment hex validation allows uppercase but nullifier derivation requires lowercase — FIXED
- **File**: `crates/annex-identity/src/registry.rs`
- **Fix**: `register_identity()` now normalizes commitment hex to lowercase before storage with `commitment_hex.to_ascii_lowercase()`.

### M-21: Integer overflow in Merkle tree capacity check — FIXED
- **File**: `crates/annex-identity/src/merkle.rs`
- **Fix**: Replaced `1 << self.depth` with `1usize.checked_shl(self.depth as u32)`. Returns `TreeFull` error if the shift would overflow.

### M-22: BFS loads entire neighbor set per hop -- no visited-node limit — FIXED
- **File**: `crates/annex-graph/src/lib.rs`
- **Fix**: Added `MAX_BFS_VISITED_NODES = 10_000` cap. BFS terminates early if the visited set exceeds this limit.

### M-23: Broadcast channel capacity for voice transcriptions hardcoded to 100 — ACKNOWLEDGED
- **File**: `crates/annex-voice/src/agent.rs`
- **Status**: Low-priority. Voice transcription channel capacity is a voice subsystem concern.

### M-24: JWT token TTL hardcoded to 1 hour — ACKNOWLEDGED
- **File**: `crates/annex-voice/src/service.rs`
- **Status**: LiveKit JWT token TTL is standard at 1 hour. Configurable in a future phase if needed.

### M-25: `federated_identities.expires_at` declared but never set or checked — ACKNOWLEDGED
- **File**: `crates/annex-db/src/migrations/018_federated_identities.sql`
- **Status**: Column reserved for future expiration enforcement. Deferred to federation maturity phase.

### M-26: `channels.channel_id` globally unique instead of per-server unique — ACKNOWLEDGED
- **File**: `crates/annex-db/src/migrations/009_channels.sql`
- **Status**: Current design uses globally unique UUIDs for channel IDs. Multi-server DB sharing is not a current use case.

### M-27: `vrp_roots` table has no PRIMARY KEY or UNIQUE constraint — ACKNOWLEDGED
- **File**: `crates/annex-db/src/migrations/001_identity.sql`
- **Status**: Root uniqueness is enforced by the Merkle tree insertion serialization (mutex). The table is append-only with active flag management.

### M-28: Federation agreement deactivation not scoped to `local_server_id` — FIXED
- **File**: `crates/annex-federation/src/db.rs`
- **Fix**: The UPDATE that deactivates old agreements now includes `AND local_server_id = ?` in its WHERE clause.

### M-29: `unchecked_transaction()` used for migrations — ACKNOWLEDGED
- **File**: `crates/annex-db/src/migrations.rs`
- **Status**: `unchecked_transaction()` is used because migrations may run within an existing transaction context. The behavior is documented and correct.

### M-30: `f64` to `f32` lossy cast for reputation score — ACKNOWLEDGED
- **File**: `crates/annex-server/src/api_agent.rs`
- **Status**: Reputation scores are bounded [0.0, 1.0] with limited precision requirements. f32 is sufficient for display purposes.

### M-31: `get_channel` error always returns NOT_FOUND — FIXED
- **File**: `crates/annex-server/src/api_channels.rs`
- **Fix**: Channel handler now properly distinguishes `ChannelError::NotFound` (404) from `ChannelError::Database` (500) errors.

### M-32: Hardcoded version string in health endpoint — FIXED
- **File**: `crates/annex-server/src/lib.rs`
- **Fix**: Health endpoint now uses `env!("CARGO_PKG_VERSION")` for the version string.

### M-33: Config file path defaults to relative "config.toml" — ACKNOWLEDGED
- **File**: `crates/annex-server/src/main.rs`
- **Status**: Relative path is standard for containerized deployments. Override via CLI argument or `ANNEX_CONFIG` env var.

### M-34: Missing index on `vrp_handshake_log(server_id, peer_pseudonym, created_at)` — FIXED
- **File**: `crates/annex-db/src/migrations/023_production_indexes.sql`
- **Fix**: Added `idx_vrp_handshake_log_peer(server_id, peer_pseudonym)` index.

### M-35: Missing index on `federation_agreements(remote_instance_id, active)` — FIXED
- **File**: `crates/annex-db/src/migrations/023_production_indexes.sql`
- **Fix**: Added `idx_federation_agreements_remote_active(remote_instance_id, active)` index.

### M-36: Missing index on `vrp_leaves(commitment_hex)` — FIXED
- **File**: `crates/annex-db/src/migrations/023_production_indexes.sql`
- **Fix**: Added `idx_vrp_leaves_commitment(commitment_hex)` index.

### M-37: Missing index on `messages(expires_at)` for retention cleanup — FIXED
- **File**: `crates/annex-db/src/migrations/023_production_indexes.sql`
- **Fix**: Added partial index `idx_messages_expires_at(expires_at) WHERE expires_at IS NOT NULL`.

---

## LOW (28) — 12 Fixed, 16 Acknowledged

These cause minor issues, waste resources, or set bad precedents.

### L-01: `parse_transfer_scope` duplicated in two files — ACKNOWLEDGED
- **File**: `crates/annex-server/src/api_rtx.rs`, `crates/annex-server/src/api_federation.rs`
- **Status**: Minor code duplication. The function is extracted to `lib.rs` as a shared helper.

### L-02: `fetch_platform_identity` re-queries server ID from DB every call — ACKNOWLEDGED
- **File**: `crates/annex-server/src/api.rs`
- **Status**: Low-priority performance concern. The query is cheap and cached by SQLite.

### L-03: Policy `previous_status` hardcoded as "changed" — FIXED
- **File**: `crates/annex-server/src/policy.rs`
- **Fix**: The actual previous status string is now captured and passed through to the event payload.

### L-04: Offered capabilities casing inconsistency — FIXED
- **File**: `crates/annex-server/src/policy.rs`, `api_vrp.rs`
- **Fix**: Both policy re-evaluation and VRP handshake now consistently use uppercase capability strings (`"VOICE"`, `"FEDERATION"`, `"TEXT"`, `"VRP"`).

### L-05: Graph profile endpoint uses unauthenticated custom header — ACKNOWLEDGED
- **File**: `crates/annex-server/src/api_graph.rs`
- **Status**: Known limitation, tied to the broader auth placeholder (C-01). Will be addressed when real auth is implemented.

### L-06: Channel handler errors not logged before returning 500 — FIXED
- **File**: `crates/annex-server/src/api_channels.rs`
- **Fix**: Channel handlers now log errors with `tracing::error!` before returning error responses.

### L-07: `delete_channel()` does not clean up related rows — FIXED
- **File**: `crates/annex-channels/src/lib.rs`
- **Fix**: `delete_channel()` now explicitly deletes messages and channel_members before deleting the channel. Tests verify cascading cleanup works correctly.

### L-08: `delete_expired_messages` has no batch size limit — FIXED
- **File**: `crates/annex-channels/src/lib.rs`
- **Fix**: Added `RETENTION_BATCH_LIMIT = 5_000`. The caller loops until fewer than the batch limit are deleted.

### L-09: No upper bound on `EventFilter.limit` — ACKNOWLEDGED
- **File**: `crates/annex-observe/src/store.rs`
- **Status**: The API handler layer (`api_observe.rs`) caps the user-facing limit. The library function trusts internal callers.

### L-10: Merkle tree `restore` only warns on root mismatch — FIXED
- **File**: `crates/annex-identity/src/merkle.rs`
- **Fix**: `restore()` now returns `Err(IdentityError::MerkleRootMismatch)` when the computed root doesn't match the stored root, preventing operation with corrupt state.

### L-11: Default `busy_timeout_ms` of 5000ms may be too short — ACKNOWLEDGED
- **File**: `crates/annex-db/src/pool.rs`
- **Status**: Now configurable via `database.busy_timeout_ms` in config.toml (validated range: 1-60000ms).

### L-12: Default pool max size of 8 may be too small — ACKNOWLEDGED
- **File**: `crates/annex-db/src/pool.rs`
- **Status**: Now configurable via `database.pool_max_size` in config.toml (validated range: 1-64).

### L-13: `LIKE '%Federated%'` for federation scope filtering — FIXED
- **File**: `crates/annex-channels/src/lib.rs`
- **Fix**: `list_federated_channels()` now uses exact string comparison (`WHERE federation_scope = ?2`) with the serialized `FederationScope::Federated` value.

### L-14: `list_channels()` has no LIMIT — FIXED
- **File**: `crates/annex-channels/src/lib.rs`
- **Fix**: Added `LIMIT 1000` to the channel list query.

### L-15: `list_members()` has no LIMIT — FIXED
- **File**: `crates/annex-channels/src/lib.rs`
- **Fix**: Added `LIMIT 10000` to the member list query.

### L-16: `check_reputation_score()` loads full handshake history — ACKNOWLEDGED
- **File**: `crates/annex-vrp/src/reputation.rs`
- **Status**: Low-priority optimization. The query is now indexed (M-34) so performance is acceptable.

### L-17: `get_agents_handler` returns all agents with no LIMIT — ACKNOWLEDGED
- **File**: `crates/annex-server/src/api_observe.rs`
- **Status**: Admin endpoint. Pagination deferred to future phase.

### L-18: Pruning interval clamped to max 60 seconds — ACKNOWLEDGED
- **File**: `crates/annex-server/src/background.rs`
- **Status**: Conservative design choice. CPU cost of a no-op pruning check every 60s is negligible.

### L-19: `FederationError` maps client errors to 500 — FIXED
- **File**: `crates/annex-server/src/api_federation.rs`
- **Fix**: `FederationError::IntoResponse` now maps `ZkVerification`, `InvalidSignature`, `Forbidden`, `IdentityDerivation`, and `UnknownRemote` to appropriate 4xx status codes.

### L-20: WAL mode PRAGMA return value not checked — ACKNOWLEDGED
- **File**: `crates/annex-db/src/pool.rs`
- **Status**: In-memory databases (used in tests) don't support WAL mode. The PRAGMA is best-effort and non-critical for correctness.

### L-21: No UNIQUE constraint on `graph_edges` to prevent duplicates — FIXED
- **File**: `crates/annex-db/src/migrations/023_production_indexes.sql`
- **Fix**: Added `UNIQUE INDEX idx_graph_edges_unique_triple(server_id, from_node, to_node, kind)`.

### L-22: `participant_type` stored as TEXT with no CHECK constraint — ACKNOWLEDGED
- **File**: `crates/annex-db/src/migrations/004_platform_identity.sql`
- **Status**: Application-level validation exists in all code paths that write participant_type. Adding a CHECK constraint via ALTER TABLE is not supported in SQLite.

### L-23: `LiveKitConfig` stores `api_secret` as plain `String` with `Debug`/`Serialize` derives — ACKNOWLEDGED
- **File**: `crates/annex-voice/src/config.rs`
- **Status**: Low-priority. Secret redaction deferred to security hardening phase.

### L-24: Division by zero-adjacent in TTS speed calculation — ACKNOWLEDGED
- **File**: `crates/annex-voice/src/tts.rs`
- **Status**: TTS speed is validated with minimum bound, preventing extreme length_scale values.

### L-25: Config env vars use inconsistent parsing — ACKNOWLEDGED
- **File**: `crates/annex-server/src/config.rs`
- **Status**: String-type env vars don't need numeric parsing. `env::var` is correct for these.

### L-26: Missing FK on `server_policy_versions.server_id` CASCADE — ACKNOWLEDGED
- **File**: `crates/annex-db/src/migrations/006_server_policy_versions.sql`
- **Status**: Server deletion is not a supported operation in production. FK integrity is maintained by the single-server deployment model.

### L-27: Missing FK on `public_event_log.server_id` — ACKNOWLEDGED
- **File**: `crates/annex-db/src/migrations/022_public_event_log.sql`
- **Status**: Same as L-26. Event log integrity is maintained by application-level constraints.

### L-28: `vrp_roots` active flag update not scoped to server — ACKNOWLEDGED
- **File**: `crates/annex-identity/src/merkle.rs`
- **Status**: The `vrp_roots` table does not have a `server_id` column. Multi-server DB sharing is not a current use case; each server has its own database.

---

## NITPICK (11) — 0 Fixed, 11 Acknowledged

Style issues, redundancy, or theoretical concerns.

### N-01: Mixed `std::sync` and `tokio::sync` lock primitives in `AppState` — ACKNOWLEDGED
- **File**: `crates/annex-server/src/lib.rs`
- **Status**: Intentional design. `std::sync` used for fast, non-contended locks; `tokio::sync` used for locks that may be held across await points.

### N-02: `Extension(Arc::new(state))` instead of `.with_state()` — ACKNOWLEDGED
- **File**: `crates/annex-server/src/lib.rs`
- **Status**: Functionally correct. Migration to `.with_state()` deferred to future refactor.

### N-03: `DuplicateNullifier` error variant reused for duplicate commitment — ACKNOWLEDGED
- **File**: `crates/annex-identity/src/registry.rs`
- **Status**: A `DuplicateCommitment` variant has been added to `IdentityError`.

### N-04: `u128` for `created_at` timestamp in RTX types — ACKNOWLEDGED
- **File**: `crates/annex-rtx/src/types.rs`
- **Status**: Custom serde serializer handles JavaScript precision limits. Changing to u64 would be a breaking API change.

### N-05: Duplicate AppState boilerplate across ~20 test files — ACKNOWLEDGED
- **File**: `crates/annex-server/tests/*.rs`
- **Status**: Test helper extraction deferred to test infrastructure phase.

### N-06: Debug `println!` left in test code — ACKNOWLEDGED
- **File**: `crates/annex-server/tests/api_ws.rs`, `crates/annex-server/tests/api_sse.rs`
- **Status**: Non-blocking. Useful for debugging test failures.

### N-07: `std::mem::forget(db_file)` leaks temp files in tests — ACKNOWLEDGED
- **File**: `crates/annex-server/tests/ws_error_handling.rs`
- **Status**: Prevents premature file deletion. OS cleans up on process exit.

### N-08: Config tests use process-global `set_var` — ACKNOWLEDGED
- **File**: `crates/annex-server/src/config.rs`
- **Status**: Tests use a shared mutex lock to serialize env var access. Correct for current Rust edition.

### N-09: Performance test uses hard assertion on timing — ACKNOWLEDGED
- **File**: `crates/annex-identity/tests/perf_merkle.rs`
- **Status**: Only runs in release mode CI. Threshold is generous for release builds.

### N-10: `VisibilityLevel::Self_` naming — ACKNOWLEDGED
- **File**: `crates/annex-types/src/lib.rs`
- **Status**: Standard Rust convention for keyword-conflicting identifiers.

### N-11: `create_channel` does not return the created channel — ACKNOWLEDGED
- **File**: `crates/annex-channels/src/lib.rs`
- **Status**: Callers use known `channel_id`. Adding RETURNING is a future convenience improvement.

---

## Test Coverage Gaps

These functionalities have **zero or minimal test coverage**:

| Area | File | Status |
|------|------|--------|
| WebSocket VoiceIntent handler | `api_ws.rs` | ~200 lines of TTS, voice client, TOCTOU protection -- untested |
| Policy re-evaluation | `policy.rs` | Multi-agent alignment recalculation -- has integration test coverage via `policy_test.rs` and `federation_policy_test.rs` |
| Channel join alignment restrictions | `api_channels.rs` | Agent alignment checks tested via `api_channels_agent.rs` |
| Federation message relay | `api_federation.rs` | Outbound relay tested via `api_federation_relay.rs` |
| Background pruning task | `background.rs` | Tested via `background_pruning_test.rs`: lifecycle, interval calculation, event emission, selective pruning, multi-node pruning |
| Rate limiter overflow | `middleware.rs` | Sliding window with retain -- tested via unit tests in `middleware.rs` and concurrency tests in `concurrency_rate_limiter.rs` |
| `find_commitment_for_pseudonym` | `api_federation.rs` | Now O(1) indexed path; tested via federation integration tests |
| Bearer token auth path | `middleware.rs` | `Authorization: Bearer` header path -- tested via `api_auth.rs` |
| History cursor pagination | `api_channels.rs` | `before` parameter -- tested via `api_channel_history.rs` |
| Concurrency scenarios | ConnectionManager, RateLimiter | Tested via `concurrency_connection_manager.rs` (8 tests: deadlock, orphan, replacement, broadcast) and `concurrency_rate_limiter.rs` (5 tests: same-key, distinct-key, high-volume, eviction) |

---

## Top 10 Systemic Risks for 100-Year Operation — Status

1. **Unbounded growth with no backpressure** — MITIGATED: WebSocket channels bounded to 256, rate limiter uses sliding window with eviction, BFS capped at 10,000 visited nodes and depth 10, query endpoints have LIMIT caps. Broadcast channels remain best-effort with configurable capacity.

2. **Progressive performance degradation** — FIXED: `find_commitment_for_pseudonym` now O(1) via indexed lookup. Production indexes added for all 6 previously unindexed hot paths (migration 023, 024). All full-table-scan queries addressed.

3. **Silent background task death** — FIXED: Background task JoinHandles are now monitored via `tokio::select!`. Unexpected completion or panic is logged at critical level.

4. **Transaction gaps in 8 write paths** — FIXED: All 8 write paths (RTX publish, RTX federation receive, federation attestation, VRP handshake, policy re-evaluation agents, policy re-evaluation federation, federation agreement creation, channel update) now use proper transaction boundaries.

5. **Signature ambiguity in 4 federation signing paths** — FIXED: All 4 signing paths now use newline (`\n`) delimiters between fields.

6. **No timeout on 5 external I/O paths** — FIXED: Federation HTTP client has 10s connect + 30s total timeout with no-redirect policy. STT subprocess has 120s timeout. TTS subprocess has 60s timeout.

7. **Cryptographic weaknesses** — LARGELY FIXED: G1/G2 curve point validation added. Hex case normalization enforced. Secret key modular reduction remains a known theoretical limitation (H-21) with negligible practical risk.

8. **Authentication is a placeholder** — ACKNOWLEDGED: Pseudonym-based auth documented as a design constraint (C-01). Real auth deferred to future phase.

9. **Merkle tree / DB divergence** — FIXED: `preview_insert` pattern prevents in-memory mutation before DB commit. `restore()` now returns error on root mismatch instead of warning.

10. **No graceful degradation** — LARGELY FIXED: Policy deserialization falls back to default. Pool exhaustion returns real errors. Retention uses batched deletion. Background tasks are monitored. Remaining: vkey must exist at startup (startup requirement, not graceful degradation).
