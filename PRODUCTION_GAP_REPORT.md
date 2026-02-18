# Annex Production Gap Report

**Standard**: System must run correctly for 100 years unattended.
**Date**: 2026-02-18
**Codebase**: ~14,500 lines of Rust across 11 crates
**Method**: Line-by-line audit of every `.rs` file, every SQL migration, every test, every `Cargo.toml`

---

## Summary

| Severity | Count |
|----------|-------|
| CRITICAL | 14 |
| HIGH     | 28 |
| MODERATE | 37 |
| LOW      | 28 |
| NITPICK  | 11 |
| **Total** | **118** |

---

## CRITICAL (14)

These will cause data corruption, security breaches, or system failure.

### C-01: Pseudonym-as-bearer-token authentication
- **File**: `crates/annex-server/src/middleware.rs:20-45`
- **Impact**: Anyone who knows a pseudonym can impersonate that user. Pseudonyms are deterministically derived and appear in public event logs, SSE streams, and message histories. The entire auth model is effectively public.

### C-02: No request body size limit
- **File**: `crates/annex-server/src/lib.rs:116-255`
- **Impact**: No `DefaultBodyLimit` on the Axum router. A single client can OOM the server with an arbitrarily large POST body.

### C-03: Observe event sequence number race condition
- **File**: `crates/annex-observe/src/store.rs:80-86`
- **Impact**: `next_seq()` reads `MAX(seq)` then inserts `seq+1` in a separate statement. Two concurrent writers produce duplicate sequence numbers. No `UNIQUE(server_id, seq)` constraint exists on `public_event_log`. This breaks monotonicity guarantees that SSE consumers and cursor-based pagination depend on.

### C-04: Merkle tree / database state divergence on failed persist
- **File**: `crates/annex-identity/src/merkle.rs:286-299`
- **Impact**: `insert_and_persist()` mutates the in-memory tree (line 291) *before* opening the DB transaction (line 294). If the transaction fails, the in-memory tree has a leaf the DB doesn't. The divergence is permanent until restart, and `restore()` only warns on root mismatch (line 226) rather than failing.

### C-05: Merkle tree state leak on registration error after DB commit
- **File**: `crates/annex-identity/src/registry.rs:61-133`
- **Impact**: `register_identity()` commits the DB transaction (line 110), then calls `tree.apply_updates()` (line 114), then calls `get_proof()` (line 117). If `get_proof` fails, the DB has committed but the caller gets an error, potentially retrying and creating a duplicate.

### C-06: O(N*M) brute-force scan in federation message relay
- **File**: `crates/annex-server/src/api_federation.rs:217-260`
- **Impact**: `find_commitment_for_pseudonym()` runs `SELECT * FROM zk_nullifiers` (full table, no WHERE), derives a pseudonym for each row, then scans `vrp_identities` for each candidate. Called on every federated message relay. As tables grow over 100 years, this becomes progressively slower until it causes request timeouts, pool exhaustion, and federation failure.

### C-07: SSRF + no timeout on federation attestation HTTP request
- **File**: `crates/annex-server/src/api_federation.rs:612-624`
- **Impact**: `reqwest::Client::new()` with no timeout makes an HTTP GET to a URL provided by the *caller* (`payload.originating_server`). An attacker can supply a URL pointing to internal services (SSRF) or a slow server (resource exhaustion). No connect/read timeout exists.

### C-08: No timeout on federation message relay HTTP requests
- **File**: `crates/annex-server/src/api_federation.rs:184`, `crates/annex-server/src/api_rtx.rs:654`
- **Impact**: `reqwest::Client::new()` with no timeouts for message and RTX bundle relay. A malicious or slow federation peer ties up the handler indefinitely. Each spawned task can hang forever, eventually exhausting file descriptors and memory.

### C-09: Non-atomic federation agreement deactivation + insertion
- **File**: `crates/annex-federation/src/db.rs:37-63`
- **Impact**: `create_agreement()` runs `UPDATE ... SET active = 0` then `INSERT INTO federation_agreements` without a transaction. A crash between the UPDATE and INSERT permanently breaks the federation link: old agreement deactivated, new agreement never created, no self-healing mechanism.

### C-10: IDENTITY_VERIFIED audit event emitted before validation completes
- **File**: `crates/annex-server/src/api.rs:369-380`
- **Impact**: The `IDENTITY_VERIFIED` event is written to the audit log *before* public signals are validated (lines 384-405). If validation fails, the audit log contains a false positive that can never be corrected. For a system that must be auditable for 100 years, phantom identity verifications are unacceptable.

### C-11: Missing G1/G2 curve point validation in ZK proof parsing
- **File**: `crates/annex-identity/src/zk.rs:59-67`
- **Impact**: `parse_g1` and `parse_g2` construct affine points without verifying they lie on the curve or in the correct subgroup. Off-curve points are a known attack vector against Groth16 verification, potentially allowing forged proofs to pass verification and granting unauthorized identity.

### C-12: `expect()` reachable in production WebSocket voice handler
- **File**: `crates/annex-server/src/api_ws.rs:634`
- **Impact**: `sessions.get(&pseudonym).cloned().expect("just inserted or already present")` -- while the `std::sync::RwLock` write lock is held, this is theoretically safe, but the `Occupied` branch (line 627) does NOT insert. If a concurrent poisoning event occurs, this panics the async task and crashes the WebSocket handler.

### C-13: Corrupted capability contract silently propagated to graph metadata
- **File**: `crates/annex-server/src/api.rs:461`
- **Impact**: `serde_json::from_str::<Value>(&contract).unwrap_or(Value::String(contract))` -- if the stored `capability_contract_json` is malformed, the raw string is silently embedded as the "parsed" value in graph node metadata, propagating corrupt data downstream with no warning.

### C-14: Unauthenticated federation endpoints on public router
- **File**: `crates/annex-server/src/lib.rs:204-230`
- **Impact**: Federation routes are on the public router with no authentication middleware. `get_federated_channels_handler` (`api_federation.rs:738-755`) has no auth at all. Any external actor can query federation state.

---

## HIGH (28)

These will cause outages, data loss, or exploitable behavior under load or over time.

### H-01: ConnectionManager deadlock risk from inconsistent lock ordering
- **File**: `crates/annex-server/src/api_ws.rs:131-205`
- **Impact**: Three `tokio::sync::RwLock`s are acquired in inconsistent order across methods. `remove_session`: sessions -> user_subscriptions -> channel_subscriptions. `subscribe`: channel_subscriptions -> user_subscriptions. `broadcast`: channel_subscriptions -> sessions. Classic ABBA deadlock potential under concurrent subscribe + remove.

### H-02: Unbounded mpsc channel per WebSocket connection
- **File**: `crates/annex-server/src/api_ws.rs:302`
- **Impact**: `mpsc::unbounded_channel()` per session. A slow or malicious client causes messages to queue unboundedly in server memory. With many concurrent connections, this causes OOM.

### H-03: `std::sync::RwLock` held in async context blocks tokio runtime
- **File**: `crates/annex-server/src/api_ws.rs:547-553,594-640`
- **Impact**: `state.voice_sessions` uses `std::sync::RwLock` but is read/written directly in the async `handle_socket` function without `spawn_blocking`. Under contention, this blocks the tokio executor thread, stalling all tasks on that thread.

### H-04: Missing transaction in RTX publish handler
- **File**: `crates/annex-server/src/api_rtx.rs:65-303`
- **Impact**: `publish_handler` performs multiple INSERTs (rtx_bundles, rtx_transfer_log, per-subscriber delivery logs) without a transaction. A crash partway through leaves orphaned records with no delivery log, or delivery log with no bundle.

### H-05: Missing transaction in federation RTX receive handler
- **File**: `crates/annex-server/src/api_federation.rs:888-1180`
- **Impact**: Same as H-04 for federated RTX bundles. Multiple INSERTs without transaction boundary.

### H-06: Missing transaction in federation attestation handler
- **File**: `crates/annex-server/src/api_federation.rs:650-731`
- **Impact**: `attest_membership_handler` performs three sequential writes (federated_identities, platform_identities, ensure_graph_node) without a transaction. Partial failure leaves identity in inconsistent state.

### H-07: Missing transaction in VRP handshake handler
- **File**: `crates/annex-server/src/api_vrp.rs:107-209`
- **Impact**: Agent handshake performs record_vrp_outcome, update_node_activity, agent_registrations upsert, and emit_and_broadcast as separate operations without a transaction.

### H-08: Missing transaction in policy re-evaluation (agents)
- **File**: `crates/annex-server/src/policy.rs:137-153`
- **Impact**: `recalculate_agent_alignments` updates each agent individually without a transaction. A crash mid-loop leaves some agents at the new alignment and others at the old alignment.

### H-09: Missing transaction in policy re-evaluation (federation)
- **File**: `crates/annex-server/src/policy.rs:313-341`
- **Impact**: Same as H-08 for federation agreement re-evaluation.

### H-10: Ephemeral signing key regenerated on every restart
- **File**: `crates/annex-server/src/main.rs:162-163`
- **Impact**: If `ANNEX_SIGNING_KEY` is not set, a new Ed25519 key is generated each startup. All previously issued federation signatures become unverifiable. Existing federation agreements break silently.

### H-11: Background task panics silently swallowed
- **File**: `crates/annex-server/src/main.rs:113-116, 202-205`
- **Impact**: Retention and pruning tasks are spawned with `tokio::spawn` and the `JoinHandle` is dropped. If either panics, the functionality permanently stops with no alert. Messages never get cleaned up; nodes never get pruned.

### H-12: Broadcast send failure silently drops observe events
- **File**: `crates/annex-server/src/lib.rs:94`
- **Impact**: `let _ = observe_tx.send(event);` -- if the broadcast channel is full or has no subscribers, events are permanently lost with no logging. For an audit system, silent event loss violates the 100-year auditability requirement.

### H-13: Rate limiter thundering-herd bypass
- **File**: `crates/annex-server/src/middleware.rs:125-127`
- **Impact**: When the HashMap exceeds 10,000 entries, `state.clear()` resets ALL rate limits for ALL clients instantly. An attacker floods with 10,001 unique IPs, triggers the clear, then sends burst traffic that bypasses all rate limiting.

### H-14: Rate limiter 2x burst at window boundary
- **File**: `crates/annex-server/src/middleware.rs:108-141`
- **Impact**: Fixed-window rate limiter resets counter at boundary. A client sends `limit` requests at end of window, then `limit` more immediately at start of next window, achieving `2 * limit` within seconds.

### H-15: `touch_activity` spawned on every WebSocket message with no debounce
- **File**: `crates/annex-server/src/api_ws.rs:322`
- **Impact**: `tokio::spawn(touch_activity(...))` on every incoming message. Each creates a `spawn_blocking` task that acquires a DB connection. Under high message volume, this exhausts the connection pool and creates massive task churn.

### H-16: Read-modify-write race in channel update
- **File**: `crates/annex-channels/src/lib.rs:170-238`
- **Impact**: `update_channel` reads current state, applies partial updates in memory, writes all fields back. Two concurrent updates: one silently overwrites the other (lost update). No optimistic concurrency control.

### H-17: No timeout on external process execution (STT)
- **File**: `crates/annex-voice/src/stt.rs:20-66`
- **Impact**: `SttService::transcribe()` spawns `whisper.cpp` with `wait_with_output()` and no timeout. A hung binary blocks the tokio task forever.

### H-18: No timeout on external process execution (TTS)
- **File**: `crates/annex-voice/src/tts.rs:58-143`
- **Impact**: Same as H-17 for piper TTS binary.

### H-19: No input size limit on audio data piped to STT
- **File**: `crates/annex-voice/src/stt.rs:20-66`
- **Impact**: `transcribe()` accepts `&[u8]` of arbitrary length. Gigabytes of data can be piped to the child process, causing unbounded memory and I/O pressure.

### H-20: No input size limit on TTS text
- **File**: `crates/annex-voice/src/tts.rs:45-56`
- **Impact**: `synthesize()` accepts `&str` of arbitrary length. Extremely long text causes unbounded CPU and memory for audio synthesis.

### H-21: Secret key silently reduced modulo field order
- **File**: `crates/annex-identity/src/commitment.rs:32`
- **Impact**: `Fr::from_be_bytes_mod_order(&sk_bytes)` silently reduces keys >= BN254 field modulus. Two different secret keys congruent modulo the field order produce identical commitments, collapsing distinct identities.

### H-22: Unknown EdgeKind silently defaults to `Connected`
- **File**: `crates/annex-graph/src/lib.rs:195-204`
- **Impact**: `str_to_edge_kind` returns `EdgeKind::Connected` for unrecognized strings. Database corruption maps to a privilege-bearing relationship type, granting unintended BFS visibility.

### H-23: Unknown NodeType silently defaults to `Human`
- **File**: `crates/annex-graph/src/lib.rs:240-247,362-369`
- **Impact**: Both `ensure_graph_node` and `get_graph_node` fall back to `NodeType::Human` for unknown strings. A corrupted or new type silently gets highest visibility/trust level.

### H-24: Conflict agents not updated in database after VRP handshake
- **File**: `crates/annex-server/src/api_vrp.rs:107-209`
- **Impact**: When an agent's VRP handshake results in `Conflict`, the DB record retains its old status. The stale alignment status persists until a future handshake succeeds.

### H-25: `filter_map(Result::ok)` silently drops DB read errors
- **File**: `crates/annex-server/src/api_federation.rs:246-247`
- **Impact**: In `find_commitment_for_pseudonym`, `.filter_map(Result::ok)` silently swallows row deserialization failures. Database corruption or schema changes produce incorrect results with no indication.

### H-26: Missing FK CASCADE on `messages.channel_id`
- **File**: `crates/annex-db/src/migrations/010_messages.sql:12`
- **Impact**: `delete_channel()` will fail at runtime with a FK violation if messages exist. No code deletes messages before deleting the channel.

### H-27: Missing FK CASCADE on `channel_members.channel_id`
- **File**: `crates/annex-db/src/migrations/011_channel_members.sql:10`
- **Impact**: Same as H-26 for channel members. Channel deletion fails if any members are recorded.

### H-28: Missing index on `graph_edges` table
- **File**: `crates/annex-db/src/migrations/013_graph_edges.sql:1-9`
- **Impact**: The most-queried table for BFS traversal has zero indexes. Every BFS hop (`WHERE server_id=? AND from_node=?`, `WHERE server_id=? AND to_node=?`) is a full table scan. Graph queries degrade to unusable as edges accumulate.

---

## MODERATE (37)

These cause degraded behavior, confusing errors, or performance cliffs.

### M-01: No CORS configuration
- **File**: `crates/annex-server/src/lib.rs:116-255`
- **Impact**: No CORS headers despite browser-facing features (SSE, WebSocket). Browser clients will be blocked by same-origin policy.

### M-02: Three inconsistent error response patterns
- **File**: `crates/annex-server/src/api_channels.rs:48`, `api.rs:147`, `api_federation.rs:28-73`
- **Impact**: `ApiError` (JSON body), `FederationError` (JSON body), and bare `StatusCode` (no body) are used across endpoints. Clients cannot reliably parse error responses.

### M-03: No upper bound on message history `limit` parameter
- **File**: `crates/annex-server/src/api_channels.rs:195`
- **Impact**: User-supplied `limit` passed directly to SQL. `limit=4294967295` loads all messages into memory.

### M-04: No upper bound on BFS `max_depth` parameter
- **File**: `crates/annex-server/src/api_graph.rs:64`
- **Impact**: `max_depth=1000000` causes an extremely expensive graph traversal. Combined with missing indexes (H-28), this is a DoS vector.

### M-05: SSE presence stream silently drops lagged events
- **File**: `crates/annex-server/src/api_sse.rs:34-35`
- **Impact**: `BroadcastStream` `Lagged` errors are filtered out. Slow SSE clients lose events with no sentinel or indication.

### M-06: SSE observe stream silently drops lagged events
- **File**: `crates/annex-server/src/api_observe.rs:153`
- **Impact**: Same as M-05 for the observe event stream.

### M-07: Signature payload concatenation without delimiters (federation messages)
- **File**: `crates/annex-server/src/api_federation.rs:159-168`
- **Impact**: Fields concatenated without separator. `message_id="abc" + channel_id="def"` collides with `message_id="abcd" + channel_id="ef"`. Theoretical signature forgery vector.

### M-08: Signature payload concatenation without delimiters (RTX relay)
- **File**: `crates/annex-server/src/api_rtx.rs:582-593`
- **Impact**: Same concatenation issue for RTX bundle relay signatures.

### M-09: Signature payload concatenation without delimiters (attestation)
- **File**: `crates/annex-server/src/api_federation.rs:588-591`
- **Impact**: Same concatenation issue for attestation request signatures.

### M-10: Signature payload concatenation without delimiters (RTX bundle signing)
- **File**: `crates/annex-rtx/src/validation.rs:95-104`
- **Impact**: `bundle_signing_payload()` concatenates fields without delimiters. Same collision class as M-07 through M-09.

### M-11: No input validation on user-supplied strings anywhere
- **File**: Multiple (all handlers)
- **Impact**: No length, format, or character set validation on pseudonyms, channel names, message content, URLs, topic strings, or any other user input. Allows arbitrarily long strings, control characters, and format exploits.

### M-12: `add_session` silently replaces existing WebSocket sessions
- **File**: `crates/annex-server/src/api_ws.rs:105-116`
- **Impact**: When a pseudonym reconnects, the old session's `channel_subscriptions` and `user_subscriptions` entries are orphaned. The old sender channel is dropped but subscription state persists until the system restarts.

### M-13: Unknown `participant_type` silently defaults to Human in federation
- **File**: `crates/annex-server/src/api_federation.rs:704-710`
- **Impact**: `_ => NodeType::Human` for unknown federation peer types. A non-human entity misclassified as Human could gain unintended access or visibility.

### M-14: `retention.rs` pool failure returns `Ok(0)` instead of error
- **File**: `crates/annex-server/src/retention.rs:34-35`
- **Impact**: When `pool.get()` fails, the function returns `Ok(0)` (pretending success). Persistent pool exhaustion is masked; retention silently stops working.

### M-15: Merkle tree depth hardcoded to 20
- **File**: `crates/annex-server/src/main.rs:121`
- **Impact**: Max 2^20 = 1,048,576 leaves. After ~1M registrations, the tree is full and all registrations fail permanently. Not configurable.

### M-16: Presence broadcast channel capacity hardcoded to 100
- **File**: `crates/annex-server/src/main.rs:167`
- **Impact**: More than 100 concurrent buffered presence events causes `Lagged` errors. Slow SSE consumers silently miss updates.

### M-17: `retention_check_interval_seconds` can be set to 0
- **File**: `crates/annex-server/src/config.rs:93-97`
- **Impact**: No minimum validation. Setting to 0 creates a tight loop consuming 100% CPU.

### M-18: `INSERT OR IGNORE` silently swallows all constraint violations in `add_member`
- **File**: `crates/annex-channels/src/lib.rs:337-341`
- **Impact**: Swallows ALL constraint violations, not just duplicates. A FK violation (invalid `channel_id`) is silently ignored; the caller believes the member was added.

### M-19: Server policy deserialization failure in `resolve_retention_days` has no fallback
- **File**: `crates/annex-channels/src/lib.rs:556-566`
- **Impact**: If stored `policy_json` is malformed (e.g., after a schema migration), every message send to channels without explicit retention fails. No default fallback.

### M-20: Commitment hex validation allows uppercase but nullifier derivation requires lowercase
- **File**: `crates/annex-identity/src/registry.rs:69`
- **Impact**: `register_identity` accepts uppercase hex. `derive_nullifier_hex` requires lowercase. A commitment registered with uppercase hex passes registration but fails identity verification.

### M-21: Integer overflow in Merkle tree capacity check
- **File**: `crates/annex-identity/src/merkle.rs:81`
- **Impact**: `1 << self.depth` panics if `depth >= 64` (on 64-bit) or `depth >= 32` (on 32-bit). No validation in constructor.

### M-22: BFS loads entire neighbor set per hop -- no visited-node limit
- **File**: `crates/annex-graph/src/lib.rs:449-524`
- **Impact**: Two SQL queries per BFS node with no cap on total nodes visited. A dense node with 10,000 neighbors triggers 10,000+ iterations. Combined with missing indexes (H-28), this is a severe DoS vector.

### M-23: Broadcast channel capacity for voice transcriptions hardcoded to 100
- **File**: `crates/annex-voice/src/agent.rs:49`
- **Impact**: High-throughput transcription drops events silently when channel is full. Not configurable.

### M-24: JWT token TTL hardcoded to 1 hour
- **File**: `crates/annex-voice/src/service.rs:56`
- **Impact**: `Duration::from_secs(60 * 60)` is a magic number. Token lifetimes need to be tunable for security policy.

### M-25: `federated_identities.expires_at` declared but never set or checked
- **File**: `crates/annex-db/src/migrations/018_federated_identities.sql:10`
- **Impact**: Column exists but always NULL. Federated identity attestations never expire. Stale attestations from compromised federation peers persist forever.

### M-26: `channels.channel_id` globally unique instead of per-server unique
- **File**: `crates/annex-db/src/migrations/009_channels.sql:4`
- **Impact**: Two servers sharing the same DB cannot have channels with the same ID. Constraint should be `UNIQUE(server_id, channel_id)`.

### M-27: `vrp_roots` table has no PRIMARY KEY or UNIQUE constraint
- **File**: `crates/annex-db/src/migrations/001_identity.sql:8-10`
- **Impact**: Allows duplicate root entries. Nothing prevents two active roots from existing simultaneously if writes are not serialized.

### M-28: Federation agreement deactivation not scoped to `local_server_id`
- **File**: `crates/annex-federation/src/db.rs:37-42`
- **Impact**: `UPDATE ... SET active = 0` filters only on `remote_instance_id`, not `local_server_id`. Multi-tenant DB: deactivates another server's agreements.

### M-29: `unchecked_transaction()` used for migrations
- **File**: `crates/annex-db/src/migrations.rs:181-186`
- **Impact**: `unchecked_transaction()` creates a savepoint if a transaction is already active, and rollback may not fully undo the migration.

### M-30: `f64` to `f32` lossy cast for reputation score
- **File**: `crates/annex-server/src/api_agent.rs:102`
- **Impact**: `reputation_score: score as f32` -- silent precision loss in API response.

### M-31: `get_channel` error always returns NOT_FOUND
- **File**: `crates/annex-server/src/api_channels.rs:132`
- **Impact**: DB connectivity errors are misclassified as "channel not found." Client gets 404 instead of 500.

### M-32: Hardcoded version string in health endpoint
- **File**: `crates/annex-server/src/lib.rs:111`
- **Impact**: `"version": "0.0.1"` is hardcoded. Should use `env!("CARGO_PKG_VERSION")`.

### M-33: Config file path defaults to relative "config.toml"
- **File**: `crates/annex-server/src/main.rs:72`
- **Impact**: Relative path is fragile and depends on working directory at startup.

### M-34: Missing index on `vrp_handshake_log(server_id, peer_pseudonym, created_at)`
- **File**: `crates/annex-db/src/migrations/007_vrp_handshake_log.sql:1-10`
- **Impact**: Reputation score query does full table scan of append-only handshake log. Progressively slower.

### M-35: Missing index on `federation_agreements(remote_instance_id, active)`
- **File**: `crates/annex-db/src/migrations/017_federation_agreements.sql:1-12`
- **Impact**: Multiple federation handlers query by these columns. Full table scan on every federated request.

### M-36: Missing index on `vrp_leaves(commitment_hex)`
- **File**: `crates/annex-db/src/migrations/001_identity.sql:1-11`
- **Impact**: `get_path_for_commitment()` queries `WHERE commitment_hex = ?1` with no index. Full table scan.

### M-37: Missing index on `messages(expires_at)` for retention cleanup
- **File**: `crates/annex-db/src/migrations/010_messages.sql:14-15`
- **Impact**: Periodic retention task `DELETE ... WHERE expires_at < datetime('now')` does full table scan of all messages.

---

## LOW (28)

These cause minor issues, waste resources, or set bad precedents.

### L-01: `parse_transfer_scope` duplicated in two files
- **File**: `crates/annex-server/src/api_rtx.rs:549-556`, `crates/annex-server/src/api_federation.rs:868-875`
- **Impact**: Identical function in two modules. Divergence risk.

### L-02: `fetch_platform_identity` re-queries server ID from DB every call
- **File**: `crates/annex-server/src/api.rs:583-587`
- **Impact**: `SELECT id FROM servers LIMIT 1` on every call despite `state.server_id` being available. Wasteful.

### L-03: Policy `previous_status` hardcoded as "changed"
- **File**: `crates/annex-server/src/policy.rs:166`
- **Impact**: The actual previous status is available but not passed through. Audit log is less useful.

### L-04: Offered capabilities casing inconsistency
- **File**: `crates/annex-server/src/policy.rs:217-223` vs `api_vrp.rs:51-59`
- **Impact**: `"voice"` vs `"VOICE"`, `"federation"` vs `"FEDERATION"`. Case-sensitive matching could cause capability mismatch.

### L-05: Graph profile endpoint uses unauthenticated custom header
- **File**: `crates/annex-server/src/api_graph.rs:78-90`
- **Impact**: `X-Annex-Viewer` header is unauthenticated. Anyone can claim any identity.

### L-06: Channel handler errors not logged before returning 500
- **File**: `crates/annex-server/src/api_channels.rs:68-81,113,155`
- **Impact**: Multiple error paths map to `StatusCode::INTERNAL_SERVER_ERROR` with no `tracing::error!`. Makes debugging impossible.

### L-07: `delete_channel()` does not clean up related rows
- **File**: `crates/annex-channels/src/lib.rs:242-248`
- **Impact**: Only deletes from `channels`. Related `messages`, `channel_members`, `graph_edges` are orphaned (and FK violations will prevent the delete from succeeding anyway -- see H-26, H-27).

### L-08: `delete_expired_messages` has no batch size limit
- **File**: `crates/annex-channels/src/lib.rs:528-534`
- **Impact**: Deletes ALL expired messages in one statement. After years of accumulation, this locks the DB for an extended period.

### L-09: No upper bound on `EventFilter.limit`
- **File**: `crates/annex-observe/src/store.rs:156`
- **Impact**: Caller can set `limit = i64::MAX`, loading all events into memory.

### L-10: Merkle tree `restore` only warns on root mismatch
- **File**: `crates/annex-identity/src/merkle.rs:188-237`
- **Impact**: If computed root != stored root (DB corruption), logs a warning but returns the inconsistent tree. System silently operates with wrong Merkle state.

### L-11: Default `busy_timeout_ms` of 5000ms may be too short
- **File**: `crates/annex-db/src/pool.rs:21`
- **Impact**: Under heavy write contention, 5-second busy timeout causes spurious `SQLITE_BUSY` errors.

### L-12: Default pool max size of 8 may be too small
- **File**: `crates/annex-db/src/pool.rs:22`
- **Impact**: With `spawn_blocking` per request, only 8 concurrent DB requests can be in flight. WebSocket message volume can easily exceed this.

### L-13: `LIKE '%Federated%'` for federation scope filtering
- **File**: `crates/annex-channels/src/lib.rs:151-167`
- **Impact**: Fragile pattern match on JSON serialization format. Would match `"NotFederated"` if such a variant existed.

### L-14: `list_channels()` has no LIMIT
- **File**: `crates/annex-channels/src/lib.rs:129-143`
- **Impact**: Returns all channels for a server. Unbounded result set.

### L-15: `list_members()` has no LIMIT
- **File**: `crates/annex-channels/src/lib.rs:379-393`
- **Impact**: Returns all members of a channel. Unbounded result set.

### L-16: `check_reputation_score()` loads full handshake history
- **File**: `crates/annex-vrp/src/reputation.rs:50-53`
- **Impact**: No LIMIT on handshake history query. Exponential decay means only recent entries matter, but all are loaded.

### L-17: `get_agents_handler` returns all agents with no LIMIT
- **File**: `crates/annex-server/src/api_observe.rs:411-416`
- **Impact**: No pagination. Thousands of agents produce an enormous JSON response.

### L-18: Pruning interval clamped to max 60 seconds
- **File**: `crates/annex-server/src/background.rs:24`
- **Impact**: For large thresholds (e.g., 24 hours), pruning checks every 60 seconds unnecessarily.

### L-19: `FederationError` maps client errors to 500
- **File**: `crates/annex-server/src/api_federation.rs:56-73`
- **Impact**: `ZkVerification`, `IdentityDerivation` and other client-caused errors return 500 instead of 4xx.

### L-20: WAL mode PRAGMA return value not checked
- **File**: `crates/annex-db/src/pool.rs:56-61`
- **Impact**: If WAL mode cannot be set (e.g., `:memory:` DB), the PRAGMA silently does nothing. Connection operates in DELETE journal mode with different concurrency characteristics.

### L-21: No UNIQUE constraint on `graph_edges` to prevent duplicates
- **File**: `crates/annex-db/src/migrations/013_graph_edges.sql:1-9`
- **Impact**: Duplicate edges between same nodes with same kind. BFS visits same neighbor multiple times.

### L-22: `participant_type` stored as TEXT with no CHECK constraint
- **File**: `crates/annex-db/src/migrations/004_platform_identity.sql:1-15`
- **Impact**: No constraint on valid values. A typo stores invalid data.

### L-23: `LiveKitConfig` stores `api_secret` as plain `String` with `Debug`/`Serialize` derives
- **File**: `crates/annex-voice/src/config.rs:1-22`
- **Impact**: Secret appears in debug prints and JSON serialization. No redaction.

### L-24: Division by zero-adjacent in TTS speed calculation
- **File**: `crates/annex-voice/src/tts.rs:88`
- **Impact**: `speed = 0.001` produces `length_scale = 1000.0`, potentially causing piper to hang.

### L-25: Config env vars use inconsistent parsing
- **File**: `crates/annex-server/src/config.rs:340-375`
- **Impact**: `ANNEX_DB_PATH`, `ANNEX_LOG_LEVEL`, and voice path vars use raw `env::var` instead of `parse_env_var`, skipping `NotUnicode` error handling.

### L-26: Missing FK on `server_policy_versions.server_id` CASCADE
- **File**: `crates/annex-db/src/migrations/006_server_policy_versions.sql:7`
- **Impact**: Deleting a server leaves orphaned policy versions.

### L-27: Missing FK on `public_event_log.server_id`
- **File**: `crates/annex-db/src/migrations/022_public_event_log.sql:8-16`
- **Impact**: No referential integrity. Orphaned events possible.

### L-28: `vrp_roots` active flag update not scoped to server
- **File**: `crates/annex-identity/src/merkle.rs:267-269`
- **Impact**: `UPDATE vrp_roots SET active = 0 WHERE active = 1` affects all servers sharing the DB.

---

## NITPICK (11)

Style issues, redundancy, or theoretical concerns.

### N-01: Mixed `std::sync` and `tokio::sync` lock primitives in `AppState`
- **File**: `crates/annex-server/src/lib.rs:31`
- **Impact**: `std::sync::Mutex` for merkle_tree, `std::sync::RwLock` for policy/voice_sessions, `tokio::sync::RwLock` in ConnectionManager. Confusing and error-prone.

### N-02: `Extension(Arc::new(state))` instead of `.with_state()`
- **File**: `crates/annex-server/src/lib.rs:254`
- **Impact**: Older Axum pattern. Functionally correct but less idiomatic.

### N-03: `DuplicateNullifier` error variant reused for duplicate commitment
- **File**: `crates/annex-identity/src/registry.rs:94`
- **Impact**: Semantically confusing. Should have a `DuplicateCommitment` variant.

### N-04: `u128` for `created_at` timestamp in RTX types
- **File**: `crates/annex-rtx/src/types.rs:47`
- **Impact**: JSON serialization may lose precision beyond `2^53`. `u64` is more conventional for millisecond timestamps.

### N-05: Duplicate AppState boilerplate across ~20 test files
- **File**: `crates/annex-server/tests/*.rs`
- **Impact**: ~20 lines of identical setup code per file. No shared test helper module.

### N-06: Debug `println!` left in test code
- **File**: `crates/annex-server/tests/api_ws.rs:154`, `crates/annex-server/tests/api_sse.rs:100`
- **Impact**: Noisy test output.

### N-07: `std::mem::forget(db_file)` leaks temp files in tests
- **File**: `crates/annex-server/tests/ws_error_handling.rs:28`
- **Impact**: Temp files accumulate across test runs.

### N-08: Config tests use process-global `set_var`
- **File**: `crates/annex-server/src/config.rs:383-636`
- **Impact**: `std::env::set_var` is process-global and unsafe in Rust 2024 edition. Cross-test interference risk.

### N-09: Performance test uses hard assertion on timing
- **File**: `crates/annex-identity/tests/perf_merkle.rs:49`
- **Impact**: `assert!(single_op_ms < 50.0)` fails on slow CI runners or debug builds.

### N-10: `VisibilityLevel::Self_` naming
- **File**: `crates/annex-types/src/lib.rs:168-170`
- **Impact**: Trailing underscore to avoid keyword. `SelfNode` or `Own` would be clearer.

### N-11: `create_channel` does not return the created channel
- **File**: `crates/annex-channels/src/lib.rs:83-111`
- **Impact**: Returns `Ok(())`. Caller must make a second query to get the channel's `id` or `created_at`. Inconsistent with `create_message` which uses `RETURNING`.

---

## Test Coverage Gaps

These functionalities have **zero or minimal test coverage**:

| Area | File | Impact |
|------|------|--------|
| WebSocket VoiceIntent handler | `api_ws.rs:466-661` | ~200 lines of TTS, voice client, TOCTOU protection -- untested |
| Policy re-evaluation | `policy.rs:16-201` | Multi-agent alignment recalculation -- untested |
| Channel join alignment restrictions | `api_channels.rs:248-298` | Agent alignment checks for Partial/Conflict agents -- untested |
| Federation message relay | `api_federation.rs:99-215` | Outbound relay to peers -- untested |
| Background pruning task | `background.rs:17-81` | Node pruning loop -- untested |
| Rate limiter overflow | `middleware.rs:125` | 10K entry clear behavior -- untested |
| `find_commitment_for_pseudonym` | `api_federation.rs:217-260` | O(N*M) brute-force lookup -- untested |
| Bearer token auth path | `middleware.rs:30-79` | `Authorization: Bearer` header path -- untested |
| History cursor pagination | `api_channels.rs:164-202` | `before` parameter -- untested |
| Concurrency scenarios | (all) | Zero concurrency tests for any race condition, deadlock, or TOCTOU |

---

## Top 10 Systemic Risks for 100-Year Operation

1. **Unbounded growth with no backpressure**: WebSocket channels, broadcast channels, rate limiter HashMap, BFS traversal, and multiple query endpoints have no upper bounds. Over time, any of these will cause OOM.

2. **Progressive performance degradation**: `find_commitment_for_pseudonym` (O(N*M)), missing indexes on 6 tables, and full-table-scan queries will slow down proportionally to data volume. After years of operation, critical paths become unusable.

3. **Silent background task death**: Retention and pruning tasks are fire-and-forget. If either panics, the functionality permanently stops. Messages accumulate forever; inactive nodes are never cleaned up.

4. **Transaction gaps in 8 write paths**: RTX publish, RTX federation receive, federation attestation, VRP handshake, policy re-evaluation (agents), policy re-evaluation (federation), federation agreement creation, and channel update all perform multiple writes without transaction boundaries. Any crash mid-operation leaves inconsistent state with no self-healing.

5. **Signature ambiguity in 4 federation signing paths**: Field concatenation without delimiters in message relay, RTX relay, attestation, and bundle signing. Theoretical forgery vector that becomes exploitable as federation relationships age.

6. **No timeout on 5 external I/O paths**: 3 reqwest HTTP clients (federation relay, RTX relay, attestation), 1 STT subprocess, 1 TTS subprocess. Any of these can hang forever, tying up tokio tasks and eventually exhausting the runtime.

7. **Cryptographic weaknesses**: Secret key modular reduction collapses identities. Missing G1/G2 curve validation enables proof forgery. Uppercase/lowercase hex mismatch blocks legitimate users.

8. **Authentication is a placeholder**: Pseudonym-as-bearer-token means anyone who can read a message history can impersonate any user. No real auth mechanism exists.

9. **Merkle tree / DB divergence**: Two separate code paths can permanently desynchronize the in-memory Merkle tree from the database, with the system only logging a warning on restart rather than failing.

10. **No graceful degradation**: Missing fallbacks (policy deserialization failure blocks all messages, pool exhaustion returns Ok(0), missing vkey panics at startup), missing circuit breakers, and missing health checks mean any single subsystem failure cascades to total system failure.
