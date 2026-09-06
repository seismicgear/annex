# Annex Architecture Map

A repo-specific map of crates, their runtime responsibilities, and the boundaries between them. The intended reader is a coding agent that has just been handed a task and needs to know which file to edit and which not to.

The Cargo workspace lives at `Cargo.toml` (workspace root). Members are listed
under `[workspace] members`. The Tauri shell `annex-desktop` is excluded from
default `cargo test` invocations because its WebKit/GTK build deps aren't
present in lightweight CI lanes — see `scripts/test-all.sh` and the
`--exclude annex-desktop` convention in CI.

## Process model

- **Standalone server** — `cargo run -p annex-server` runs `crates/annex-server/src/main.rs`, which loads `config.toml`, calls `prepare_server()` → `app(state)` from `lib.rs`, binds an Axum HTTP server, and serves the API + WebSocket.
- **Embedded server (Host mode)** — `crates/annex-desktop/src/main.rs` reuses the same `prepare_server()` / `app()` entrypoints inside the Tauri process and exposes them on `127.0.0.1:<random>`.
- **Client mode (no embedded server)** — Tauri webview connects directly to a remote HTTPS server URL.
- **Frontend** — `client/` is a Vite + React 19 SPA. The same bundle ships standalone (web), bundled into Tauri (desktop), and is served by the Axum server in dev. State lives in Zustand stores.

## Runtime boundaries

```
┌────────────────────────────────────────────────────────────────────────┐
│                         User device (Tauri process)                    │
│                                                                        │
│  ┌─────────────────┐     ┌──────────────────────────────────────────┐  │
│  │ Webview         │ ←→  │ Embedded annex-server (Host) or          │  │
│  │ (client/ build) │     │ no server (Client mode → remote URL)     │  │
│  └─────────────────┘     └──────────────────────────────────────────┘  │
│         ↑                          ↑                                   │
│         │ Tauri commands           │ Axum (HTTP + WS)                  │
│         │ (deep-link, startup)     │                                   │
└─────────┼──────────────────────────┼───────────────────────────────────┘
          │                          │
          │                          │ HTTPS / WSS
          │                          ▼
          │              ┌────────────────────────┐
          │              │ Other Annex servers    │
          │              │ (federation peers)     │
          │              └────────────────────────┘
          │
          ▼
   keychain (OS keyring) + AppData/local files
```

## Crates

### `annex-server` (`crates/annex-server/`)

**Role**: HTTP + WebSocket service. Owns routing, middleware, request
validation, and orchestration across every other crate.

Source layout (one file per concern; small modules preferred):
- `main.rs` — standalone server entry; reads `config.toml`, calls `prepare_server`.
- `lib.rs` — `app(state) -> Router`, `prepare_server(cfg)`, `AppState`. Builds the Merkle tree from DB, loads the Groth16 vkey (or `generate_dummy_vkey()` if missing — dev only, see invariants), wires Axum routes.
- `api*.rs` — one module per API surface: `api.rs` (root + identity), `api_admin.rs`, `api_agent.rs`, `api_channels.rs`, `api_federation.rs`, `api_graph.rs`, `api_invite.rs`, `api_link_preview.rs`, `api_observe.rs`, `api_rtx.rs`, `api_sse.rs`, `api_upload.rs`, `api_usernames.rs`, `api_vrp.rs`, `api_ws.rs`.
- `middleware.rs` — auth + ZK gating: parses `pseudonym`, validates session token, validates membership proof against `state.merkle_tree.root_hex()` and `state.vkey`, enforces `state.enforce_zk_proofs`.
- `policy.rs` — server policy snapshot, role gating, channel ACL evaluation.
- `retention.rs` + `background.rs` — message retention worker, presence sweep.
- `config.rs` — `Config`, `ConfigError`, `load_config`, env-var overrides, slug auto-persist.

Tests:
- `--lib` unit tests for config, middleware, CORS, etc.
- `tests/api_*.rs` integration tests use `tower::ServiceExt::oneshot()` against an in-memory SQLite (see `tests/common/mod.rs`); a few use a real `TcpListener` for WS (see `tests/ws_*.rs`).

### `annex-identity` (`crates/annex-identity/`)

**Role**: Identity + ZKP primitives. Pure, no I/O except DB.

- `commitment.rs` — `Poseidon(sk, roleCode, nodeId)` over BN254. Uses `light-poseidon`.
- `merkle.rs` — append-only Poseidon Merkle tree (depth = `merkle_tree_depth`, default 20). Loads from `identities` table on boot; verifies persisted root against recomputed root and panics on mismatch (`MerkleRootMismatch`).
- `nullifier.rs` — `zk_nullifiers (topic, nullifier_hex, pseudonym_id, commitment_hex)`; `insert_nullifier` is the only enforced double-join boundary.
- `zk.rs` — Arkworks Groth16 over BN254. `parse_proof`, `parse_public_signals`, `parse_verification_key`, `verify_proof`. Also exports `generate_dummy_vkey()` — **dev fallback only**, never to be used as a production substitute.
- `registry.rs` — DB-backed identity table reads + `RegistrationResult`, `get_path_for_commitment`.
- `platform.rs` — `PlatformIdentity` (cross-server identity continuity).
- `poseidon.rs` — light-poseidon adapter pinned to BN254 / `Fr`.

ZK artifact paths are documented in `zk-merkle-production.md`.

### `annex-db` (`crates/annex-db/`)

**Role**: SQLite pool + numbered migrations.

- `pool.rs` — `r2d2 + r2d2_sqlite`. Configures WAL mode, busy timeout from `Config::database`, foreign keys.
- `migrations/` — numbered `NNN_*.sql` files (currently `000_init.sql` through `033_server_public_url.sql`). **Append-only.** A new migration is always a new file; never edit a committed migration. See `invariants.md` § SQLite migrations.
- `migrations.rs` — runs migrations on pool init.
- `lib.rs` — `create_pool(path, busy_timeout, max_size)`, `apply_migrations`. In-memory mode used by tests via `:memory:`.

### `annex-channels` (`crates/annex-channels/`)

**Role**: Channel CRUD, channel membership, message persistence helpers.

- `lib.rs` — single module; channel records, messages, ACL helpers. Wraps `annex-db` queries for the API layer.

### `annex-federation` (`crates/annex-federation/`)

**Role**: Server-to-server trust + signaling.

- `handshake.rs` — VRP handshake exchange, `compare_peer_anchor` driver, `contracts_mutually_accepted()`.
- `signal.rs` — federation signal envelope + Ed25519 signature verification. **Signature verification cannot be bypassed** — see invariants.
- `transport.rs` — outbound HTTPS transport (via `reqwest`), retries.
- `db.rs` — federation_agreements + federated_identities reads/writes.
- `types.rs` — message types crossing the federation boundary.

### `annex-rtx` (`crates/annex-rtx/`)

**Role**: Agent ↔ server knowledge exchange protocol (RTX).

- `types.rs` — `KnowledgeBundle`, `TransferScope`, `RtxRequest`/`RtxResponse`.
- `validation.rs` — bundle structure + capability gating.
- `error.rs` — typed errors.

The server side lives in `annex-server/src/api_rtx.rs`.

### `annex-voice` (`crates/annex-voice/`)

**Role**: Voice channel orchestration, TTS/STT integration, voice profile metadata.

- `service.rs` — voice session lifecycle (per-server).
- `tts.rs` — Piper TTS bridge (calls bundled `assets/piper/piper` binary; voices in `assets/voices/`).
- `stt.rs` — STT bridge (whisper.cpp by default; configurable via `Config::voice`).
- `agent.rs` — voice profile selection for agent personas.
- `config.rs` — voice-specific config knobs (also reflected in `annex-server::config::ServerConfig` env overrides).

WebRTC media plane is hosted by `annex-server` (it embeds `webrtc-rs`), and its
signalling rides the app's own `/ws` WebSocket rather than a separate signalling
port. LiveKit references in older docs are stale — nothing in the tree links,
dials or spawns a LiveKit server.

### `annex-graph` + `annex-observe` (`crates/annex-graph/`, `crates/annex-observe/`)

- `annex-graph` — `graph_nodes` / `graph_edges` representation of the presence + agent graph.
- `annex-observe` — append-only event log + `EventPayload` types; consumed by `api_observe.rs` and SSE.

### `annex-vrp` (`crates/annex-vrp/`)

**Role**: Value Resonance Protocol — the trust negotiation core.

- `types.rs` — `VrpAnchorSnapshot`, `VrpRoleEntry`, `VrpTopic`, `Capabilities`, `EthicalRoot`, `VrpCapabilitySharingContract`.
- `semantic.rs` — anchor comparison; embedding-equivalence stub.
- `reputation.rs` — alignment-status computation.
- `server_root.rs` — server-side anchor root resolution.

### `annex-types` (`crates/annex-types/`)

**Role**: Cross-crate types with no dependencies on the rest. `PresenceEvent`,
voice DTOs, `policy.rs` shared types. Add types here when more than one crate
must agree on the wire shape.

### `annex-desktop` (`crates/annex-desktop/`)

**Role**: Tauri 2 shell. Single source file (`src/main.rs`).

- Reuses `annex_server::{config, init_tracing, prepare_server}` via dependency on `annex-server`.
- Tauri commands: `get_startup_mode`, `save_startup_mode`, `clear_startup_mode`, `start_embedded_server`, `reset_server_data`, `get_pending_invite`, `check_first_run_completed`, `mark_first_run_completed`, `get_public_endpoint` (router session), and the deep-link plugin.
- `tauri.conf.json` references `../../zk/keys/membership_vkey.json`, `../../assets/piper`, `../../assets/voices` as bundled resources.
- `build.rs` performs a soft WebKit version check; the actual GTK/PipeWire/WebKitGTK system deps must be installed before `cargo build -p annex-desktop`.
- Pre-existing compile/test caveats are listed in CLAUDE.md; `--exclude annex-desktop` is the convention everywhere except dedicated desktop CI lanes.

### `client/` (`client/`)

**Role**: Frontend SPA.

- Build: `vite build` produces `client/dist/`. The desktop pipeline copies it into `crates/annex-desktop/dist/` via `scripts/build-desktop.js`.
- State: Zustand stores in `client/src/stores/` — `identity.ts`, `channels.ts`, `servers.ts`, `voice.ts`, `usernames.ts`.
- API client: `client/src/lib/api.ts` (single source for HTTP). WebSocket client: `client/src/lib/ws.ts` (`AnnexWebSocket`).
- ZK proof: `client/src/lib/zk.ts` + `client/src/workers/proof.worker-*.ts`. The membership zkey + wasm are loaded from `client/public/zk/`.
- Tests: Vitest + RTL (`*.test.ts(x)`); Playwright E2E in `client/e2e/`. Server-side E2E uses `scripts/e2e-server.sh`.

### `zk/` (`zk/`)

**Role**: Circom circuits + Groth16 trusted setup tooling.

- `circuits/identity.circom` — `commitment = Poseidon(sk, roleCode, nodeId)`. 264 non-linear constraints.
- `circuits/membership.circom` — Membership circuit at `Membership(20)`: depth-20 Merkle inclusion + identity recomputation + leafIndex/pathIndexBits binding. Public outputs are `[root, commitment]`. 5,184 non-linear constraints.
- `scripts/build-circuits.js` — runs `circom`, emits `build/{name}.r1cs`, `build/{name}.sym`, `build/{name}_js/{name}.wasm`.
- `scripts/setup-groth16.js` — generates `pot14_*.ptau` (depth-14 powers of tau) and per-circuit zkeys + vkeys.
- `scripts/test-proofs.js` — smoke tests for identity + membership proofs (16 assertions).
- `bin/circom` — vendored circom binary used by build-circuits.js when system circom is absent.

Production artifact requirements are spelled out in `zk-merkle-production.md`.

## Where requests flow

For a typical "client wants to send a message" path:

1. `client/src/components/MessageInput.tsx` calls `client/src/lib/api.ts`'s send helper.
2. HTTP → `crates/annex-server/src/api_channels.rs`.
3. Middleware (`crates/annex-server/src/middleware.rs`) verifies session token; if `enforce_zk_proofs` is on, it also checks the membership proof against `state.merkle_tree.root_hex()` and rejects raw-pseudonym calls.
4. Channel ACL via `policy.rs` + `annex-channels::lib`.
5. Persistence via `annex-db` pool.
6. Broadcast to WS subscribers via `annex-server::api_ws`.

For "agent sends VRP handshake to a federated peer":

1. `crates/annex-server/src/api_federation.rs` builds an envelope.
2. `annex-federation::signal` signs with the server's Ed25519 key; recipient verifies.
3. `annex-federation::handshake` runs the VRP comparison; result lands in `federation_agreements`.

## Don't-touch surfaces

The following are wire-format-stable and require explicit task scope before
changing:
- WS frame shapes in `crates/annex-server/src/api_ws.rs` and the matching parser in `client/src/lib/ws.ts`.
- API JSON shapes under `crates/annex-server/src/api*.rs` and their consumers in `client/src/lib/api.ts`.
- Numbered SQL migrations in `crates/annex-db/src/migrations/`.
- Public ZK signal layout: `[root, commitment]` for membership; the order is consumed by `crates/annex-server/src/middleware.rs` and `client/src/lib/zk.ts`.
- The `annex://invite?server=…&code=…` deep-link grammar parsed in `crates/annex-desktop/src/main.rs::parse_deep_link_invite`.
