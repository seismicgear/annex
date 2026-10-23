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
3. v2 federated message envelopes are freshness-checked against `Config::federation::{freshness_window_seconds, future_skew_seconds}` on the live receive path. Catch-up (implemented — ADR-0014) writes through `delivery_mode = 'catchup'` to bypass the live window.
4. SQLite errors of class `SQLITE_FULL` / `SQLITE_IOERR` trip the storage gate. While `Degraded`, the auth middleware returns HTTP 507 on POST/PUT/PATCH/DELETE. Reads continue. Operator restarts the process (or the future admin-clear endpoint lands) to clear the gate.
5. `public_event_log` rows now carry `prev_hash` + `event_hash`. A tamper to one row is detectable by `verify_event_log_chain`.
6. The migration runner verifies SHA-256 of every embedded migration against the recorded hash on every boot. An edit to a committed migration after deploy is now a startup error rather than silently accepted.
7. Periodic SQLite maintenance (`wal_checkpoint(TRUNCATE)` + `ANALYZE`, optional `VACUUM`) runs every `maintenance_interval_hours` when `maintenance_enabled = true`.

## Remaining gaps (intentionally deferred)

These were either out of scope for this pass or have a concrete prerequisite that needs its own scoped task:

| Gap | Status | Why deferred | Next step |
| --- | --- | --- | --- |
| **Federation catch-up endpoint** | Implemented (ADR-0014 amendment) | Signed nonce-bound batches over durable outbox sequence; cursor and deletion markers persist across restart | Run real peer handshake, attestation, join, partition and restart acceptance |
| **Federated redaction tombstones** | **Landed 2026-06-10** (ADR-0011 amendment) | — | `FederatedRedactionEnvelope` + `POST /api/federation/redactions`; WS delete on FEDERATED channels enqueues signed tombstones via the outbox; origin-receipt + sender/moderation authority checks; receipt-ledger idempotent |
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

## Annex 0.1.3 native qualification handoff — 2026-10-20

**Not synchronized to KI AI Congress / Constellation.** The authorized connector
returned `SCOPE_UNAVAILABLE`; no external handoff was saved and unchanged access
failures are not retried. This existing repository record is the fallback.

Owner: current Annex release assignment, branch
`fix/annex-commercial-release-20261020`, [draft PR #339](https://github.com/seismicgear/annex/pull/339).
The exact candidate source is `c85f710c8893b7bf24cd4056dd19c2c0fa6a9506`.
Subsequent commits through `d6f5920` change qualification, documentation, and
release assembly; they do not change the candidate application.

Implemented and tested: all six native installer formats launch, complete their
recorded native journeys, and match their artifact inventory.
Windows EXE/MSI public-0.1.2 upgrades passed
[39447739096](https://github.com/seismicgear/annex/actions/runs/39447739096);
Linux DEB/AppImage and DEB uninstall/reinstall recovery passed
[39453972417](https://github.com/seismicgear/annex/actions/runs/39453972417).
Both Mac DMG invite journeys passed
[39446503370](https://github.com/seismicgear/annex/actions/runs/39446503370);
ARM and Intel native hosting/reconnection passed
[39447072157](https://github.com/seismicgear/annex/actions/runs/39447072157)
and [39455236396](https://github.com/seismicgear/annex/actions/runs/39455236396).

The release producer now preserves installer format in updater feeds and signs
the exact DEB payload using the existing release authority. Eighteen component
tests passed; authenticated reassembly
[39458858799](https://github.com/seismicgear/annex/actions/runs/39458858799)
passed without rebuilding any native payload. Original build
[39442643771](https://github.com/seismicgear/annex/actions/runs/39442643771)
passed native and assembly gates, but GitHub draft upload failed HTTP 403.
The original assembly was downloaded, API-digest verified, and all 54 checksums
verified locally. Delivery must use the new metadata, including the DEB signature.

Actual in-app updater acceptance remains underway. Linux already rejected a
stalled transfer and corrupted payload, verified the valid signature, and replaced
the AppImage; its observer incorrectly compared the mounted ELF with Array's
whole-AppImage hash. The observer now derives the ELF hash from the authenticated
candidate. That correction and separate MSI/DEB update paths require successful
native reruns before any updater completion claim.

Integration work is submitted in [Array #10](https://github.com/seismicgear/Array/pull/10)
(175 local native desktop tests passed, one platform skip) and
[KI Store #370](https://github.com/seismicgear/kuykendall-industries/pull/370)
(147 local tests passed). Their CI jobs did not start because Actions budgets
prevented execution. No production Store policy, database, offer, or publication
was changed. Credentials for Windows publisher signing and Apple Developer ID/
notarization are absent; Array's production trust/publisher policies and the
live Store/customer acceptance remain separate gates. These are tested candidate
packages, not a signed commercial release.

Next acceptance: finish native updater fault/retry/replacement/restart/recovery
on every format, acquire the authenticated metadata, verify unchanged installer
hashes, deliver installers and evidence, and update this record with exact results.

### October 20 native updater follow-through

**Not synchronized to KI AI Congress / Constellation:** the existing connector
attempt returned `SCOPE_UNAVAILABLE`; no unchanged retry or successful external
write is claimed.

Linux DEB and AppImage native updater journeys passed in
[39460419192](https://github.com/seismicgear/annex/actions/runs/39460419192) and
[39460424908](https://github.com/seismicgear/annex/actions/runs/39460424908).
Windows ordinary HTTPS trust passed in
[39466145151](https://github.com/seismicgear/annex/actions/runs/39466145151);
full MSI/NSIS updater acceptance remains separate.

Mac updater screenshots revealed a real startup error before the updater:
the compiled build-checkout path shadowed installed keys, which macOS's sandbox
refused to read. Commit `87550094627d8c84ca001bbcb3623f09f7ea0794` prioritizes
bundled verification and speech resources. All 67 native desktop tests passed
(one ignored). It also routes observed permission clicks through XCTest instead
of raw events requiring an additional Accessibility grant. New native build
[39467676029](https://github.com/seismicgear/annex/actions/runs/39467676029)
must supply replacement packages and new source-bound native receipts before
delivery. The c85 packages are superseded; their previous successful journeys
remain historical evidence, not qualification of the changed source.

The c85 Windows NSIS in-app updater passed
[39466983287](https://github.com/seismicgear/annex/actions/runs/39466983287):
real stalled/corrupted downloads, verified retry, installed hash, automatic
restart, and ordinary founder/message recovery. Complete c85 draft recovery
[39466157468](https://github.com/seismicgear/annex/actions/runs/39466157468)
passed; all 55 remote asset digests match local files. Draft ID `418644113`
is explicitly labeled superseded and remains unpublished.

The repaired 87550094 Apple Silicon DMG passed installed Host and recovery in
[39471087867](https://github.com/seismicgear/annex/actions/runs/39471087867).
Windows's initial blank WebView page caused an observer race; the test now waits
for the actual Annex title and retains failure screenshots. Qualification
revision `5d6bd731` also waits for source-matching successful native build gates
instead of treating an unfinished package as a failed app. Six guard tests cover
pending jobs, failures, cancellations, deadlines, and wrong source/workflow.
No signature, sandbox, TLS or artifact verification gate was relaxed.

The replacement native build `39467676029` has now passed all six installer
formats, server tarball, server image and preflight jobs. Windows EXE/MSI
public-0.1.2 upgrades and forced-stop recovery passed
[39473252886](https://github.com/seismicgear/annex/actions/runs/39473252886);
both verify the actual 87550094 installed executable and saved founder/message.
Intel Host/reopen acceptance passed
[39472567707](https://github.com/seismicgear/annex/actions/runs/39472567707),
completing that journey on both Mac architectures.

Mac updater run `39470725215` reached the actual Updates dialog, but its observer
incorrectly expected the version label and number in one accessibility node.
The original screenshot and accessibility tree show separate sibling nodes
and an ordinary Annex local-network prompt. Qualification revision `8a0db083`
checks the exact label/value pair inside that dialog and handles the observed
consent prompt while waiting. The replacement run is `39474109144`; no app bytes
changed. Signed-bundle verification now also reads actual entitlements, requires
the shipped sandbox/network/media claims and hardened runtime, and rejects
debugger/runtime bypasses. Four portable negative/positive tests passed; native
execution of the strengthened gate remains part of the updater run.

Windows EXE and MSI ordinary-profile in-app updates passed runs `39471633279`
and `39472176568`. Linux AppImage update passed `39471081130`; the DEB test's
Send-button race was repaired by waiting for WebDriver's actual enabled state,
and its rerun `39475380938` passed. All four cases cover stalled/corrupt downloads,
verified retry, exact replacement, observed automatic restart and founder/message
retention. Linux observers reopened only after independently observing the real
restarted candidate; no interactive privilege-password acceptance is claimed.

The new macOS signature gate first rejected Apple's legitimate
`flags=0x10002(adhoc,runtime)` display because its parser expected `(runtime)`.
Commit `787326ee` reads the signed CodeDirectory's `CS_RUNTIME` bit. Six portable
tests now pass, and the parser accepts the actual signed ARM and Intel candidate
receipts. The strengthened native updater rerun is `39477741199`; this is a
verification change, not an application or entitlement change.

The 87550094 assembly and draft upload succeeded. All 55 local deliverables
(2,697,119,561 bytes) match draft `418685781` by exact size and SHA-256. The
authenticated assembly ZIP is artifact `12286754997`, SHA-256
`ed6f5372ca7f828cb0ca7a6a769e93ec89c8177600d04f1c2c489dfa9d5719f8`.
The candidate remains unpublished and its Windows installers remain unsigned.

Intel native invitation run `39472574363` passed, completing exact-DMG recipient
acceptance on both Mac architectures. Its original receipt confirms Safari cold
launch, explicit join, an independently received message and same-process warm
delivery. Native direct-transport acceptance also passed against actual Monolith
introduction and Chromium 141. The test page initially fell through Vite's SPA
index and navigated during the proof; a custom application mode and explicit
transport-module dependency entries fixed that harness failure. The successful
run verifies one introduction followed by direct messaging and 1 MiB transfer
after further browser introductions are blocked. Two real Whisper provider tests
passed with their required known-speech waveform; the initially selected media
fixture lacked the second expected phrase and is retained as a fixture-selection
failure. No provider assertion or application code was weakened.

The strengthened native signature gate passed on both Mac architectures. The
ARM updater test in `39477741199` stopped when local-network consent interrupted
its long synthesized message entry. Commit `1d410cb0` uses ordinary native Paste
and requires the complete visible composer value before Send. Its ARM rerun
`39481337225` passed that step, then captured consent consuming the Check action:
the Updates dialog remained idle and the independent HTTPS service received no
manifest request. Commit `1ced964e` repeats that idempotent check after granting
the observed consent, only when its actual native control is enabled.

The Intel job in `39477741199` reached the real manifest and payload download,
including the deliberately stalled first chunk, then its observer threw because
WebKit supplies a numeric accessibility value for a heading. Commit `6524abf8`
matches only string labels/values instead of applying `CONTAINS` to every value.
No update, installation, timeout or signature requirement was relaxed. Run
`39484798242`, which still contained that proven shared observer error, was
cancelled and superseded by both-architecture run `39485169930`. These failures
are retained as harness failures; they do not establish successful Mac updates.

Full CI `39477744324` passed server checks, frontend tests, both required desktop
builds, desktop install/launch/uninstall, both server smoke lanes, the UI audit
and supply-chain checks. The deferred Mac CI job was skipped; actual Mac package
acceptance remains separately recorded. All application paths compared with
87550094 are unchanged. The delivery's eleven passing native journeys have
authenticated original archive acquisition records and explicit source checks.

### Newly disclosed TLS advisories supersede the 87550094 packages

The later supply-chain job `118843594831` in `39485170900` correctly failed
on RUSTSEC-2026-0381/0382. A fresh complete-lockfile audit additionally found
RUSTSEC-2026-0383. The compatible repair selects `rustls-webpki 0.103.16` and
both root-data packages at `1.0.10`. The separate DTLS regression lockfile also
uses the repaired verifier; its exact-byte provenance digest was updated for
that dependency-only change. Both root-data LICENSE files match the previously
shipped text byte-for-byte, and the license allowances remain exact-version
exceptions. No vulnerability ignore was introduced.

The fresh full-lockfile audit now reports zero vulnerabilities, with the existing
maintenance notices still separate. Draft `418685781` was explicitly marked
**SUPERSEDED — DO NOT DISTRIBUTE** and remains unpublished. The previously linked
local installers likewise require replacement; functional passes do not waive the
new vulnerability findings. Current Mac runs on those old bytes are diagnostic
only. The security-fixed candidate requires rebuilt native packages and fresh
source-bound installation/updater verification before delivery.

### macOS updater installation defect and repair

Not synchronized to KI AI Congress / Constellation: the connector remains
`SCOPE_UNAVAILABLE`. Current owner: Annex commercial-release work.

The security-fixed `1afe0088` passed Windows EXE/MSI installer and updater
journeys, but ARM job `118857362386` in run `39489162027` failed the actual app
replacement after its three transfer/signature stages passed. The captured
native Updates dialog reports the installer failure and retains version 0.1.2.
This is a product failure, separate from the earlier XCTest observation defect.

The repair implements a bundle-scoped XPC installation service with message
identity authentication, a separately pinned public key, bounded archive
validation, strict staged code verification and atomic exchange. The main
application's sandbox and UI remain intact. Five portable Rust boundary tests,
strict Clippy and seven signed-entitlement tests passed locally. Native XPC,
replacement, restart and recovery remain unqualified until the newly added
native checks and full packaged updater journeys execute. No commercial
readiness or deployment is claimed by this source change.

### October 23 qualification continuation

Not synchronized to KI AI Congress / Constellation: the previous connector
response was `SCOPE_UNAVAILABLE`. Current owner: Annex commercial-release work.

Native XPC CI passed on both the original implementation and `e76bfeea`.
Release run `39958646388` then stopped at the newly published ctutils advisory
RUSTSEC-2026-0385. The production lockfile now selects upstream `ctutils 0.4.4`
without suppressing the advisory. All older candidate packages are superseded.

The mobile delete-confirm screenshot baseline had captured opacity-zero
actions. Its replacement is the inspected actual image from the e76bfeea UI
audit, which shows the intended armed Confirm control. The capture hook now
explicitly establishes hover and asserts opacity; no product UI or tolerance
was changed. Fresh CI, package builds and exact-package native updater checks
must complete before delivery.
