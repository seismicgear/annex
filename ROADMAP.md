# Roadmap

**The single source of truth for what's built, what's in progress, and what's next.**

This document is the project's canonical state. If you are a contributor — human or AI — you read this before touching code. You do not start work on a phase that isn't marked `IN PROGRESS` or `NEXT`. You do not declare a phase complete unless every completion criterion is met. You do not skip phases. You do not reorder phases. You do not "start on the easy parts" of a later phase while an earlier phase has unmet criteria.

The build order exists for a reason. Every phase depends on the phases before it. The identity plane must exist before trust negotiation can happen. Trust negotiation must exist before agents can connect. Agents must be able to connect before voice makes sense. Federation must work before cross-server anything is possible.

If you're an AI assistant helping with this project: **read the current phase status before proposing any work.** If a phase is marked `COMPLETE`, do not suggest redoing it. If a phase is marked `NOT STARTED`, do not start it unless every prerequisite phase is `COMPLETE`. Follow the build order. Follow the completion criteria. Follow the code standards. No shortcuts.

---

## Current State

```
Phase 0: Project Scaffold .............. COMPLETE
Phase 1: Identity Plane ................ COMPLETE
Phase 2: Server Core ................... IN PROGRESS
Phase 3: VRP Trust Negotiation ......... NOT STARTED
Phase 4: Text Communication ............ NOT STARTED
Phase 5: Presence Graph ................ NOT STARTED
Phase 6: Agent Protocol ................ NOT STARTED
Phase 7: Voice Infrastructure .......... NOT STARTED
Phase 8: Federation .................... NOT STARTED
Phase 9: RTX Knowledge Exchange ........ NOT STARTED
Phase 10: Observability ................ NOT STARTED
Phase 11: Client ....................... NOT STARTED
Phase 12: Hardening & Audit ............ NOT STARTED
```

**Last updated**: 2026-02-11

When phases change status, update this block and add a dated entry to the [Changelog](#changelog) at the bottom of this document.

---

## Code Standards

These apply to every line of code in the project. No exceptions. No "we'll clean it up later." Code that doesn't meet these standards doesn't merge.

### Language & Runtime

- **Server core**: Rust (stable toolchain, latest stable release)
- **Async runtime**: `tokio` (multi-threaded)
- **HTTP framework**: `axum`
- **Database**: SQLite via `rusqlite` (with WAL mode, connection pooling via `r2d2` or `deadpool`)
- **ZKP circuits**: Circom 2.x + snarkjs (Groth16)
- **Voice transport**: LiveKit server SDK
- **Build system**: Cargo workspaces for Rust, npm for ZK toolchain

### Quality Gates

Every PR must pass all of the following before merge:

1. **`cargo clippy -- -D warnings`** — zero warnings. Not "acceptable warnings." Zero.
2. **`cargo fmt --check`** — standard formatting. No style debates.
3. **`cargo test`** — all tests pass. No `#[ignore]` on tests that should run. No flaky tests.
4. **No `unwrap()` or `expect()` in non-test code** — all error paths are handled explicitly. Use `Result<T, E>` propagation or structured error types via `thiserror`. The only exception is startup initialization where failure means the process cannot run (and even then, use `expect("reason")` with an explanation).
5. **No `unsafe` without a safety comment and review** — if you need unsafe, document exactly why it's sound and flag it for review.
6. **No `todo!()` or `unimplemented!()` in merged code** — if a code path isn't implemented yet, return a structured error. `todo!()` is a panic. Panics in production are bugs.
7. **No hardcoded secrets, keys, or credentials** — not even in tests. Use environment variables or config files. Test fixtures use deterministic but clearly synthetic values.
8. **All public functions and types have doc comments** — `///` with a description of what it does, what it returns, and what errors it can produce.
9. **All database migrations are reversible** — every `up` has a `down`. Schema changes that can't be reversed are redesigned until they can be.
10. **All network-facing endpoints have input validation** — no trusting client input. Ever. Validate types, ranges, lengths, and formats before processing.

### Error Handling

- Define domain-specific error types per module using `thiserror`.
- API endpoints return structured error responses with error codes, not raw strings.
- Internal errors log at `error!` or `warn!` level with context. User-facing errors are sanitized — no stack traces, no internal state.
- Database errors, ZK verification failures, and VRP handshake failures each have their own error variants — not lumped into a generic `AnyhowError`.

### Testing

- **Unit tests**: Every module has them. Every public function has at least one happy-path and one error-path test.
- **Integration tests**: Every API endpoint has request/response tests against a real (in-memory) SQLite database.
- **ZK circuit tests**: Every circuit has witness generation tests with valid and invalid inputs, verifying that invalid inputs produce verification failures (not just "different outputs").
- **VRP protocol tests**: Full handshake sequences with `Aligned`, `Partial`, and `Conflict` outcomes, including reputation decay and contract negotiation.
- **Concurrency tests**: Database operations under concurrent load. WebSocket message ordering under parallel writers. Merkle tree updates under concurrent registrations.
- **No mocks for cryptographic operations** — if a test involves ZKP verification, it runs the actual verifier. Mocked crypto tests prove nothing.

### Documentation

- Every module has a `//!` module-level doc comment explaining its role in the architecture.
- Every public struct and enum has field-level documentation.
- Architecture Decision Records (ADRs) for any non-obvious design choice. Format: `docs/adr/NNNN-title.md` with Status, Context, Decision, Consequences.
- The README, FOUNDATIONS, AGENTS, HUMANS, and this ROADMAP are kept in sync with implementation. If code changes invalidate documentation, the documentation is updated in the same PR.

### Commit Standards

- Conventional commits: `feat:`, `fix:`, `refactor:`, `docs:`, `test:`, `chore:`
- Each commit does one thing. No "feat: add identity plane and also fix that websocket bug and update docs."
- Commit messages reference the phase and step: `feat(phase-1/step-3): implement Poseidon commitment generation`

---

## Phase 0: Project Scaffold

**Status**: `COMPLETE`
**Prerequisites**: None
**Estimated scope**: Repository structure, dependency setup, CI pipeline, development environment

### What This Phase Produces

A repository that compiles, runs an empty server, passes CI, and has the workspace structure for every subsequent phase. No features. No endpoints. Just the skeleton.

### Steps

#### 0.1 — Repository initialization
- [ ] Initialize Cargo workspace with the following crate structure:
  ```
  annex/
  ├── Cargo.toml              # workspace root
  ├── crates/
  │   ├── annex-server/       # main binary — axum server, startup, config
  │   ├── annex-identity/     # identity plane: commitments, VRP registry, Merkle tree
  │   ├── annex-vrp/          # VRP trust negotiation: compare_peer_anchor, contracts, reputation
  │   ├── annex-graph/        # presence graph: nodes, edges, BFS, visibility
  │   ├── annex-channels/     # channel model: types, messages, retention
  │   ├── annex-voice/        # voice integration: LiveKit, TTS, STT
  │   ├── annex-federation/   # federation: attestation, agreements, cross-server messaging
  │   ├── annex-rtx/          # RTX knowledge exchange: bundles, transfer, governance
  │   ├── annex-observe/      # observability: event log, public APIs, SSE streams
  │   ├── annex-db/           # database layer: migrations, connection pool, query helpers
  │   └── annex-types/        # shared types, error definitions, constants
  ├── zk/                     # Circom circuits, build scripts, trusted setup
  │   ├── circuits/
  │   ├── build/
  │   ├── keys/
  │   └── scripts/
  ├── docs/
  │   ├── adr/                # Architecture Decision Records
  │   └── protocol/           # Protocol specifications
  ├── tests/                  # Integration tests
  ├── README.md
  ├── FOUNDATIONS.md
  ├── AGENTS.md
  ├── HUMANS.md
  └── ROADMAP.md
  ```
- [ ] Each crate has a `lib.rs` with a module-level doc comment and compiles cleanly

#### 0.2 — Dependency lockdown
- [ ] `annex-server`: `axum`, `tokio`, `tower`, `tracing`, `tracing-subscriber`, `serde`, `serde_json`
- [ ] `annex-db`: `rusqlite` (with `bundled` feature), `r2d2`
- [ ] `annex-identity`: `sha2`, `poseidon-rs` (or equivalent BN254 Poseidon), `ark-groth16` / `snarkjs` FFI
- [ ] `annex-vrp`: re-exports from MABOS `value_resonance` types (or vendored subset)
- [ ] `annex-types`: `thiserror`, `serde`, `uuid`
- [ ] Workspace-level `[patch]` and `[profile]` configured for dev and release
- [ ] `Cargo.lock` committed

#### 0.3 — Database foundation
- [ ] `annex-db` implements:
  - Connection pool initialization (SQLite WAL mode, foreign keys enabled)
  - Migration runner (embed SQL migrations, run on startup)
  - Empty initial migration (`000_init.sql`) that creates nothing but proves the system works
- [ ] Integration test: start pool → run migrations → verify empty database

#### 0.4 — Server skeleton
- [ ] `annex-server` starts an axum server on configurable host:port
- [ ] Health check endpoint: `GET /health` returns `200 OK` with `{"status": "ok", "version": "0.0.1"}`
- [ ] Graceful shutdown on SIGTERM/SIGINT
- [ ] Configuration via `config.toml` and/or environment variables (using `config` crate or manual)
- [ ] Structured logging via `tracing` with JSON output option

#### 0.5 — CI pipeline
- [ ] GitHub Actions (or equivalent) running:
  - `cargo clippy -- -D warnings`
  - `cargo fmt --check`
  - `cargo test`
  - `cargo build --release`
- [ ] All checks must pass on every PR
- [ ] CI runs on both `main` and PR branches

### Completion Criteria

Phase 0 is **COMPLETE** when:

- [ ] `cargo build` succeeds for all workspace crates with zero warnings
- [ ] `cargo test` passes (including the database integration test)
- [ ] `cargo clippy -- -D warnings` passes
- [ ] CI pipeline is green on `main`
- [ ] The server starts, serves `/health`, and shuts down cleanly
- [ ] Every crate has a module-level doc comment
- [ ] Every dependency is justified in an ADR or inline comment

---

## Phase 1: Identity Plane

**Status**: `COMPLETE`
**Prerequisites**: Phase 0 `COMPLETE`
**Estimated scope**: ZKP circuits, Poseidon Merkle tree, VRP registry, identity commitment, pseudonym derivation

### What This Phase Produces

The cryptographic identity substrate. After this phase, an entity can generate a commitment, register it in a server's Merkle tree, produce a Groth16 membership proof, verify it, derive a pseudonym, and persist all of this across server restarts. No network-facing API yet — that's Phase 2. This is the pure cryptographic and storage layer.

### Steps

#### 1.1 — Poseidon hash implementation
- [x] Implement or integrate Poseidon hash over BN254 scalar field
- [x] Support 2-input (for Merkle internal nodes) and 3-input (for identity commitment) variants
- [x] Test vectors against known Poseidon implementations (circomlibjs reference vectors)
- [x] Benchmark: target < 1ms per hash on release build

#### 1.2 — Identity commitment
- [x] Implement `commitment = Poseidon(sk, roleCode, nodeId)` in Rust
- [x] Define `RoleCode` enum: `Human = 1`, `AiAgent = 2`, `Collective = 3`, `Bridge = 4`, `Service = 5`
- [x] Commitment output is a BN254 scalar serialized as 32-byte big-endian hex string
- [x] Test: deterministic output for known inputs; different inputs produce different commitments

#### 1.3 — Merkle tree implementation
- [x] Implement binary Poseidon Merkle tree with configurable depth (default: 20, supporting ~1M leaves)
- [x] Operations: `insert(leaf) → leafIndex`, `get_proof(leafIndex) → (pathElements, pathIndexBits)`, `get_root() → root`
- [x] Tree is append-only (no deletion of leaves — deactivation is handled at the identity layer)
- [x] Test: insert N leaves, verify each proof against current root; verify proof fails against stale root

#### 1.4 — Merkle persistence
- [x] `annex-db` migration: `vrp_leaves` table (`leaf_index INTEGER, commitment_hex TEXT, inserted_at TEXT`)
- [x] `annex-db` migration: `vrp_roots` table (`root_hex TEXT, active INTEGER, created_at TEXT`)
- [x] On startup: reconstruct Merkle tree from `vrp_leaves` in leaf-index order, verify final root matches active `vrp_roots` entry
- [x] Test: insert leaves → restart (drop in-memory tree) → rebuild → verify root matches → insert more leaves → verify proofs still work

#### 1.5 — Circom circuits
- [x] `zk/circuits/identity.circom`:
  - Private inputs: `sk`, `roleCode`, `nodeId`
  - Public output: `commitment`
  - Computes `Poseidon(sk, roleCode, nodeId)` and constrains output
- [x] `zk/circuits/membership.circom`:
  - Private inputs: `sk`, `roleCode`, `nodeId`, `leafIndex`, `pathElements[depth]`, `pathIndexBits[depth]`
  - Public signals: `root`, `commitment`
  - Recomputes identity leaf, rebuilds Merkle path, constrains final root
- [x] `zk/scripts/build-circuits.js`: compiles both circuits to R1CS + WASM + sym
- [x] `zk/scripts/setup-groth16.js`: Groth16 trusted setup (powers of tau + circuit-specific), exports verification key
- [x] `zk/scripts/test-proofs.js`: generates witness from known inputs, produces proof, verifies proof — end-to-end

#### 1.6 — Proof generation and verification in Rust
- [x] Implement proof generation: given private inputs + circuit WASM + zkey, produce Groth16 proof + public signals
  - This may use snarkjs via wasm-bindgen, FFI to a JS runtime, or a native Rust Groth16 prover (ark-groth16)
  - ADR required for the chosen approach with benchmarks
- [x] Implement proof verification: given proof + public signals + verification key, return bool
- [x] Test: generate proof from valid witness → verify succeeds; tamper with public signal → verify fails

#### 1.7 — Pseudonym derivation
- [x] Implement: `nullifierHex = sha256(commitmentHex + ":" + topic)`
- [x] Implement: `pseudonymId = sha256(topic + ":" + nullifierHex)`
- [x] Test: same commitment + same topic → same pseudonym; same commitment + different topic → different pseudonym

#### 1.8 — Nullifier tracking
- [x] `annex-db` migration: `zk_nullifiers` table (`topic TEXT, nullifier_hex TEXT, created_at TEXT, UNIQUE(topic, nullifier_hex)`)
- [x] Insert nullifier on successful proof verification; reject if nullifier already exists for that topic (anti-double-join)
- [x] Test: first verification succeeds and inserts nullifier; second verification with same commitment + topic fails

#### 1.9 — VRP identity registry
- [x] `annex-db` migration: `vrp_identities` table (`commitment_hex TEXT UNIQUE, role_code INTEGER, node_id INTEGER, created_at TEXT`)
- [x] `annex-db` migration: `vrp_topics` table (`topic TEXT UNIQUE, description TEXT, created_at TEXT`)
- [x] `annex-db` migration: `vrp_roles` table (`role_code INTEGER UNIQUE, label TEXT`)
- [x] Seed default topics: `annex:server:v1`, `annex:channel:v1`, `annex:federation:v1`
- [x] Seed default roles: `Human = 1`, `AiAgent = 2`, `Collective = 3`, `Bridge = 4`, `Service = 5`

#### 1.10 — Platform identity registry
- [x] `annex-db` migration: `platform_identities` table:
  ```sql
  CREATE TABLE platform_identities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    pseudonym_id TEXT NOT NULL,
    participant_type TEXT NOT NULL,  -- HUMAN | AI_AGENT | COLLECTIVE | BRIDGE | SERVICE
    can_voice INTEGER NOT NULL DEFAULT 0,
    can_moderate INTEGER NOT NULL DEFAULT 0,
    can_invite INTEGER NOT NULL DEFAULT 0,
    can_federate INTEGER NOT NULL DEFAULT 0,
    can_bridge INTEGER NOT NULL DEFAULT 0,
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (server_id, pseudonym_id)
  );
  ```
- [x] CRUD operations in `annex-identity` with proper error types
- [x] Test: create identity → lookup by pseudonym → update capability flags → verify flags changed

### Completion Criteria

Phase 1 is **COMPLETE** when:

- [ ] Poseidon hash produces correct outputs against reference test vectors
- [ ] Merkle tree inserts, proves, and verifies correctly for 1000+ leaves
- [ ] Merkle tree survives restart (persistence + rebuild verified)
- [ ] Circom circuits compile, setup completes, and end-to-end proof generation + verification works in the JS toolchain
- [ ] Proof generation and verification work from Rust
- [ ] Pseudonym derivation is deterministic and topic-scoped
- [ ] Nullifier tracking prevents double-join per topic
- [ ] All database tables exist with migrations and are tested
- [ ] All code passes quality gates (clippy, fmt, tests, docs)
- [ ] ADR exists for ZK prover integration approach

---

## Phase 2: Server Core

**Status**: `IN PROGRESS`
**Prerequisites**: Phase 1 `COMPLETE`
**Estimated scope**: HTTP API for identity operations, server configuration, authentication middleware, tenant model

### What This Phase Produces

A running server that accepts identity registrations, processes VRP membership proofs, issues pseudonyms, and persists everything. After this phase, a client can register a commitment, get a Merkle path, submit a membership proof, receive a pseudonym, and query identity state — all over HTTP.

### Steps

#### 2.1 — Server configuration model
- [x] `annex-db` migration: `servers` table:
  ```sql
  CREATE TABLE servers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    slug TEXT NOT NULL UNIQUE,
    label TEXT NOT NULL,
    policy_json TEXT NOT NULL,       -- current server policy
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
  );
  ```
- [x] `annex-db` migration: `server_policy_versions` table (append-only policy changelog):
  ```sql
  CREATE TABLE server_policy_versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    version_id TEXT NOT NULL UNIQUE,
    policy_json TEXT NOT NULL,
    activated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (server_id) REFERENCES servers(id)
  );
  ```
- [x] Default server policy struct in `annex-types`:
  - `agent_min_alignment_score: f32`
  - `agent_required_capabilities: Vec<String>`
  - `federation_enabled: bool`
  - `default_retention_days: u32`
  - `voice_enabled: bool`
  - `max_members: u32`

#### 2.2 — VRP registration endpoint
- [x] `POST /api/registry/register`
  - Input: `{ "commitmentHex": "0x...", "roleCode": 1, "nodeId": 42 }`
  - Behavior: insert into `vrp_identities`, allocate leaf in Merkle tree, update `vrp_roots`, return Merkle path
  - Response: `{ "identityId": 123, "leafIndex": 5, "rootHex": "0x...", "pathElements": [...], "pathIndexBits": [...] }`
  - Errors: duplicate commitment (409), invalid role code (400), tree full (507)

#### 2.3 — Merkle path retrieval endpoint
- [x] `GET /api/registry/path/:commitmentHex`
  - Returns current Merkle path for an existing commitment
  - Error: commitment not found (404)

#### 2.4 — Current root endpoint
- [x] `GET /api/registry/current-root`
  - Returns: `{ "rootHex": "0x...", "leafCount": 42, "updatedAt": "..." }`

#### 2.5 — ZK membership verification endpoint
- [x] `POST /api/zk/verify-membership`
  - Input: `{ "root": "...", "commitment": "...", "topic": "annex:server:v1", "proof": {...}, "publicSignals": [...] }`
  - Behavior:
    1. Verify `root` matches an active `vrp_roots` entry
    2. Verify Groth16 proof against verification key
    3. Derive nullifier, check `zk_nullifiers` for duplicate
    4. Derive pseudonym
    5. Insert nullifier
    6. Create or activate `platform_identities` row
  - Response: `{ "ok": true, "pseudonymId": "..." }`
  - Errors: invalid proof (401), stale root (409), duplicate nullifier (409)

#### 2.6 — VRP topics and roles endpoints
- [x] `GET /api/registry/topics` — list all registered VRP topics
- [x] `GET /api/registry/roles` — list all registered role codes

#### 2.7 — Identity query endpoints
- [x] `GET /api/identity/:pseudonymId` — returns participant type, capability flags, active status
- [x] `GET /api/identity/:pseudonymId/capabilities` — returns capability flags only

#### 2.8 — Request authentication middleware
- [x] Implement middleware that:
  - Extracts pseudonym from request header (`X-Annex-Pseudonym`) or bearer token
  - Validates pseudonym exists and is active in `platform_identities`
  - Injects identity context into request extensions for downstream handlers
  - Returns 401 for missing/invalid pseudonym
- [x] Endpoints that require authentication use this middleware via axum layer

#### 2.9 — Rate limiting
- [ ] Per-pseudonym rate limiting on registration and verification endpoints
- [ ] Configurable via server policy
- [ ] Returns 429 with `Retry-After` header

### Completion Criteria

Phase 2 is **COMPLETE** when:

- [ ] A client can: register commitment → receive Merkle path → generate proof (client-side) → submit proof → receive pseudonym — end to end over HTTP
- [ ] Duplicate registrations are rejected
- [ ] Duplicate nullifiers (same commitment + topic) are rejected
- [ ] Invalid proofs are rejected
- [ ] Stale roots are rejected
- [ ] Identity queries return correct data
- [ ] Authentication middleware correctly gates protected endpoints
- [ ] Rate limiting works
- [ ] All endpoints have integration tests with real SQLite
- [ ] All endpoints have documented request/response contracts

---

## Phase 3: VRP Trust Negotiation

**Status**: `NOT STARTED`
**Prerequisites**: Phase 2 `COMPLETE`
**Estimated scope**: Port/adapt MABOS VRP trust negotiation for server-agent and server-server contexts

### What This Phase Produces

The trust negotiation layer. After this phase, an entity (agent or server) can perform a full VRP handshake — present its ethical root / policy root, compare against the counterparty, negotiate transfer scope, evaluate capability contracts, check reputation, and receive an alignment classification. This is the diplomatic layer.

### Steps

#### 3.1 — VRP types adaptation
- [ ] Port or vendor the following from MABOS `value_resonance.rs` into `annex-vrp`:
  - `VrpAnchorSnapshot`
  - `VrpAlignmentStatus` (Aligned, Partial, Conflict)
  - `VrpTransferScope` (NoTransfer, ReflectionSummariesOnly, FullKnowledgeBundle)
  - `VrpFederationHandshake`
  - `VrpAlignmentConfig`
  - `VrpTransferAcceptanceConfig`
  - `VrpValidationReport`
  - `VrpCapabilitySharingContract`
  - `VrpTransferAcceptanceError`
- [ ] Adapt for Annex context: server policy roots replace ethical roots for server-side comparison
- [ ] ADR: vendored vs. shared dependency with MABOS crate

#### 3.2 — Anchor comparison engine
- [ ] Port `compare_peer_anchor` into `annex-vrp`
- [ ] Port `anchor_snapshot` (Poseidon or SHA256 hashing of principles + prohibited actions)
- [ ] Port `negotiate_transfer_scope`
- [ ] Port `resolve_transfer_scope`
- [ ] Port `contracts_mutually_accepted`
- [ ] Port `validate_federation_handshake`
- [ ] All with full test suites matching MABOS test coverage

#### 3.3 — Semantic alignment (optional but recommended)
- [ ] Port `calculate_semantic_alignment` with embedder trait
- [ ] Implement or integrate a local embedding model for semantic comparison
- [ ] ADR: which embedding model, local vs. API, latency budget
- [ ] If deferred: ensure `VrpAlignmentConfig.semantic_alignment_required` can be set to `false` without breaking the handshake flow

#### 3.4 — Reputation system
- [ ] `annex-db` migration: `vrp_handshake_log` table (stores `VrpValidationReport` summaries per counterparty)
- [ ] Port `check_reputation_score` — compute longitudinal alignment from handshake history
- [ ] Port `record_vrp_outcome` — log handshake results
- [ ] Test: reputation degrades over repeated `Partial` or `Conflict` outcomes; improves over `Aligned` outcomes

#### 3.5 — Server policy root
- [ ] Define `ServerPolicyRoot` struct (maps to `EthicalRoot` shape):
  - `principles: Vec<String>` — server's declared operating principles
  - `prohibited_actions: Vec<String>` — what the server prohibits
- [ ] Server policy root is derived from `server_policy_versions.policy_json`
- [ ] Changes to server policy regenerate the policy root and trigger re-evaluation of all active agent and federation relationships

#### 3.6 — Agent handshake endpoint
- [ ] `POST /api/vrp/agent-handshake`
  - Input: agent's `VrpAnchorSnapshot`, `VrpFederationHandshake`, `VrpCapabilitySharingContract`
  - Behavior: run full `compare_peer_anchor` against server policy root, evaluate contracts, check reputation, log outcome
  - Response: `VrpValidationReport` (alignment status, transfer scope, negotiation notes)
  - On `Aligned` or `Partial`: create `agent_registrations` row, proceed to membership proof flow
  - On `Conflict`: reject with detailed report

#### 3.7 — Agent registration persistence
- [ ] `annex-db` migration: `agent_registrations` table:
  ```sql
  CREATE TABLE agent_registrations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    pseudonym_id TEXT NOT NULL,
    alignment_status TEXT NOT NULL,    -- ALIGNED | PARTIAL | CONFLICT
    transfer_scope TEXT NOT NULL,      -- NO_TRANSFER | REFLECTION_SUMMARIES_ONLY | FULL_KNOWLEDGE_BUNDLE
    capability_contract_json TEXT NOT NULL,
    voice_profile_id INTEGER,
    reputation_score REAL NOT NULL DEFAULT 0.0,
    last_handshake_at TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (server_id) REFERENCES servers(id)
  );
  ```

#### 3.8 — Transfer acceptance validation
- [ ] Port `check_transfer_acceptance` — validates whether a given report meets minimum requirements for data transfer
- [ ] Integrate into RTX flow (Phase 9) and federation flow (Phase 8)

### Completion Criteria

Phase 3 is **COMPLETE** when:

- [ ] An agent can perform a full VRP handshake against a server and receive `Aligned`, `Partial`, or `Conflict` status
- [ ] Capability contracts are mutually evaluated and mismatches are reported
- [ ] Reputation scores are computed from handshake history and affect alignment outcomes
- [ ] All `VrpValidationReport` fields are populated correctly
- [ ] Handshake outcomes are logged and auditable
- [ ] Server policy root changes trigger re-evaluation logic (even if re-evaluation is manual in this phase)
- [ ] All ported MABOS tests pass in the Annex context
- [ ] Integration tests cover the full handshake flow end-to-end over HTTP

---

## Phase 4: Text Communication

**Status**: `NOT STARTED`
**Prerequisites**: Phase 2 `COMPLETE`
**Estimated scope**: Channel model, WebSocket messaging, message persistence, retention

### What This Phase Produces

Working text chat. After this phase, authenticated users can create channels, join channels, send messages, receive messages in real time via WebSocket, and have messages persisted and expired per retention policy.

### Steps

#### 4.1 — Channel model
- [ ] `annex-db` migration: `channels` table:
  ```sql
  CREATE TABLE channels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    channel_id TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    channel_type TEXT NOT NULL,         -- TEXT | VOICE | HYBRID | AGENT | BROADCAST
    topic TEXT,
    vrp_topic_binding TEXT,            -- VRP topic required for membership proof
    required_capabilities_json TEXT,   -- capability flags needed to join
    agent_min_alignment TEXT,          -- minimum VrpAlignmentStatus for agents
    retention_days INTEGER,            -- NULL = use server default
    federation_scope TEXT NOT NULL DEFAULT 'LOCAL',  -- LOCAL | FEDERATED
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (server_id) REFERENCES servers(id)
  );
  ```
- [ ] Channel CRUD operations in `annex-channels`

#### 4.2 — Message model
- [ ] `annex-db` migration: `messages` table:
  ```sql
  CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    channel_id TEXT NOT NULL,
    message_id TEXT NOT NULL UNIQUE,
    sender_pseudonym TEXT NOT NULL,
    content TEXT NOT NULL,
    reply_to_message_id TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT,                    -- computed from retention policy
    FOREIGN KEY (channel_id) REFERENCES channels(channel_id)
  );
  ```
- [ ] Message insert with automatic `expires_at` computation from channel/server retention policy

#### 4.3 — WebSocket infrastructure
- [ ] Implement WebSocket upgrade on `GET /ws`
- [ ] Authenticate via pseudonym on connection (query param or first message)
- [ ] Connection manager: track active connections per channel, per pseudonym
- [ ] Message frame format:
  ```json
  {
    "type": "message",
    "channelId": "...",
    "content": "...",
    "replyTo": null
  }
  ```
- [ ] Broadcast incoming messages to all connections subscribed to that channel
- [ ] Handle disconnection cleanup

#### 4.4 — Channel join/leave
- [ ] `POST /api/channels/:channelId/join` — validates capability flags, creates membership record
- [ ] `POST /api/channels/:channelId/leave` — removes membership, unsubscribes WebSocket
- [ ] WebSocket subscription management tied to channel membership

#### 4.5 — Message history
- [ ] `GET /api/channels/:channelId/messages?before=...&limit=...` — paginated message retrieval
- [ ] Returns messages in reverse chronological order
- [ ] Respects authentication (only members can read)

#### 4.6 — Retention enforcement
- [ ] Background task: periodically scan `messages` where `expires_at < now()`, hard delete
- [ ] Configurable scan interval
- [ ] Test: insert message with 1-second retention → wait → verify deletion

#### 4.7 — Channel REST API
- [ ] `POST /api/channels` — create channel (requires `can_moderate` or operator)
- [ ] `GET /api/channels` — list channels on this server (filtered by membership/visibility)
- [ ] `GET /api/channels/:channelId` — channel metadata
- [ ] `DELETE /api/channels/:channelId` — delete channel (operator only)

### Completion Criteria

Phase 4 is **COMPLETE** when:

- [ ] Users can create channels, join channels, send messages, and receive messages in real time
- [ ] Messages persist to SQLite and are retrievable via history endpoint
- [ ] Retention policies are enforced (messages are deleted when expired)
- [ ] WebSocket connections handle authentication, subscription, broadcast, and disconnection
- [ ] Channel capability requirements are enforced on join
- [ ] All endpoints and WebSocket flows have integration tests
- [ ] Message ordering is consistent under concurrent writers (tested)

---

## Phase 5: Presence Graph

**Status**: `NOT STARTED`
**Prerequisites**: Phase 2 `COMPLETE`, Phase 4 `COMPLETE`
**Estimated scope**: Graph nodes/edges, BFS degrees, visibility rules, SSE presence stream, pruning

### What This Phase Produces

The live presence graph. After this phase, participants appear as nodes, relationships are edges, visibility is degree-based, and the graph updates in real time via SSE.

### Steps

#### 5.1 — Graph node model
- [ ] `annex-db` migration: `graph_nodes` table (from Monolith spec, adapted):
  ```sql
  CREATE TABLE graph_nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    pseudonym_id TEXT NOT NULL,
    node_type TEXT NOT NULL,           -- HUMAN | AI_AGENT | COLLECTIVE | BRIDGE | SERVICE
    active INTEGER NOT NULL DEFAULT 1,
    last_seen_at TEXT,
    metadata_json TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (server_id, pseudonym_id)
  );
  ```
- [ ] Auto-create graph node on successful VRP membership verification (tie into Phase 2 verify-membership flow)

#### 5.2 — Graph edge model
- [ ] `annex-db` migration: `graph_edges` table:
  ```sql
  CREATE TABLE graph_edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    from_node TEXT NOT NULL,
    to_node TEXT NOT NULL,
    kind TEXT NOT NULL,                -- MEMBER_OF | CONNECTED | AGENT_SERVING | FEDERATED_WITH | MODERATES
    weight REAL DEFAULT 1.0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
  );
  ```
- [ ] Edge CRUD operations

#### 5.3 — BFS degrees of separation
- [ ] Implement `find_path_bfs(from, to, max_depth)` over `graph_edges` (treat as undirected)
- [ ] `GET /api/graph/degrees?from=A&to=B&maxDepth=6`
- [ ] Returns `{ "found": true, "path": [...], "length": N }` or `{ "found": false }`
- [ ] Test: verify shortest path is found; verify max_depth is respected

#### 5.4 — Visibility service
- [ ] Implement `GraphVisibilityService`:
  - Given `viewerPseudonym`, compute degree map via BFS from viewer up to depth 3
  - Return `VisibilityLevel` per target: `Self`, `Degree1`, `Degree2`, `Degree3`, `AggregateOnly`, `None`
- [ ] `GET /api/graph/profile/:targetPseudonym` with `X-Annex-Viewer` header:
  - Returns fields filtered by visibility level

#### 5.5 — SSE presence stream
- [ ] `GET /events/presence` — Server-Sent Events stream, scoped by server
- [ ] Events: `NODE_ADDED`, `NODE_UPDATED`, `NODE_PRUNED`, `EDGE_ADDED`, `EDGE_REMOVED`
- [ ] Subscriber management per server (connection tracking, cleanup on disconnect)

#### 5.6 — Activity tracking and pruning
- [ ] Update `graph_nodes.last_seen_at` on VRP handshake, message send, WebSocket heartbeat
- [ ] Background pruning job: nodes where `last_seen_at < now - inactivity_threshold` → `active = 0`
- [ ] Emit `NODE_PRUNED` event
- [ ] Reactivation: fresh VRP handshake reactivates existing node (flip `active = 1`)
- [ ] Test: inactive node is pruned → reconnect → node reactivated → pseudonym preserved

### Completion Criteria

Phase 5 is **COMPLETE** when:

- [ ] Participants appear as graph nodes automatically on membership verification
- [ ] Edges can be created and queried
- [ ] BFS shortest path works correctly
- [ ] Visibility levels are enforced server-side based on degree
- [ ] SSE stream delivers real-time presence events
- [ ] Inactive nodes are pruned and reactivatable
- [ ] All operations are scoped by `server_id`

---

## Phase 6: Agent Protocol

**Status**: `NOT STARTED`
**Prerequisites**: Phase 3 `COMPLETE`, Phase 4 `COMPLETE`, Phase 5 `COMPLETE`
**Estimated scope**: Agent connection flow (VRP handshake → membership → channels), agent presence, capability enforcement

### What This Phase Produces

End-to-end agent participation. After this phase, an AI agent can perform a VRP trust handshake, prove membership, receive a pseudonym, join channels (subject to alignment and capability restrictions), send and receive text messages, and appear in the presence graph.

### Steps

#### 6.1 — Agent connection flow
- [ ] Define the full agent connection sequence:
  1. Agent calls `POST /api/vrp/agent-handshake` (Phase 3)
  2. On `Aligned` or `Partial`: agent calls `POST /api/registry/register` with its commitment
  3. Agent generates membership proof client-side
  4. Agent calls `POST /api/zk/verify-membership` → receives pseudonym
  5. Agent opens WebSocket connection with pseudonym
  6. Agent joins channels via `POST /api/channels/:channelId/join`
- [ ] Document this flow in `docs/protocol/agent-connection.md`

#### 6.2 — Capability-gated channel access
- [ ] Channel join validates agent's alignment status against `channel.agent_min_alignment`
- [ ] Channel join validates agent's capability flags against `channel.required_capabilities_json`
- [ ] `Partial` agents are restricted to `TEXT` channels only (no VOICE, no HYBRID voice features)
- [ ] `Conflict` agents cannot join any channel

#### 6.3 — Agent presence in graph
- [ ] Agent graph nodes created with `node_type = AI_AGENT`
- [ ] `metadata_json` includes: alignment status, transfer scope, capability contract summary
- [ ] Edges: `AGENT_SERVING` from agent pseudonym to channels it's active in

#### 6.4 — Agent capability inspection
- [ ] `GET /api/agents/:pseudonymId` — returns alignment status, capability contract, transfer scope, reputation score
- [ ] Accessible to any authenticated participant on the same server

#### 6.5 — Agent WebSocket behavior
- [ ] Agents use the same WebSocket protocol as humans
- [ ] Messages from agents are attributed to their pseudonym like any other participant
- [ ] No special message framing or separate protocol — same wire format

#### 6.6 — Re-evaluation on policy change
- [ ] When `server_policy_versions` is updated:
  - Recompute alignment for all active `agent_registrations`
  - Downgrade or reject agents whose alignment dropped
  - Disconnect agents that are now `Conflict`
  - Emit `AGENT_REALIGNED` events on presence stream

### Completion Criteria

Phase 6 is **COMPLETE** when:

- [ ] An AI agent can connect, handshake, prove membership, join channels, and send/receive messages end-to-end
- [ ] Alignment and capability restrictions are enforced on channel join
- [ ] Agents appear correctly in the presence graph with metadata
- [ ] Agent capabilities are inspectable by other participants
- [ ] Server policy changes trigger agent re-evaluation
- [ ] Integration test: full agent lifecycle from handshake to message exchange to disconnection

---

## Phase 7: Voice Infrastructure

**Status**: `NOT STARTED`
**Prerequisites**: Phase 4 `COMPLETE`, Phase 6 `COMPLETE`
**Estimated scope**: LiveKit integration, voice LLM TTS service, STT service, agent voice pipeline, voice profiles

### What This Phase Produces

Voice channels that work for both humans and agents. Humans speak via WebRTC through LiveKit. Agents send text and receive speech-rendered audio via the platform's voice LLM. All participants hear each other.

### Steps

#### 7.1 — LiveKit server integration
- [ ] LiveKit server deployment configuration (Docker or native)
- [ ] `annex-voice` crate: LiveKit server SDK integration for room management
- [ ] Create LiveKit room per `VOICE` or `HYBRID` channel
- [ ] Token generation for human participants to join LiveKit rooms
- [ ] Test: create room → generate token → verify token grants correct room access

#### 7.2 — Human voice flow
- [ ] Client connects to LiveKit room using generated token
- [ ] Audio published and subscribed via standard LiveKit WebRTC flow
- [ ] No server-side audio processing needed for human-to-human voice

#### 7.3 — Voice LLM service (TTS)
- [ ] `annex-voice` implements TTS service:
  - Accepts text input + voice profile ID
  - Renders audio via local voice model (Piper, Bark, or Parler-TTS)
  - Outputs PCM/opus audio frames
- [ ] ADR: which TTS model, quantization strategy, latency targets
- [ ] Voice profile model: voice timbre, speed, pitch parameters per profile
- [ ] `annex-db` migration: `voice_profiles` table

#### 7.4 — Agent voice output pipeline
- [ ] Agent sends text intent via WebSocket message with `type: "voice_intent"`
- [ ] Server routes text to TTS service with the agent's assigned voice profile
- [ ] TTS output is published to the LiveKit room as an audio track attributed to the agent's pseudonym
- [ ] Test: agent sends text → audio appears in LiveKit room → other participants hear it

#### 7.5 — STT service
- [ ] `annex-voice` implements STT service:
  - Subscribes to human audio tracks in LiveKit rooms where agents are present
  - Transcribes audio via local Whisper (or equivalent)
  - Delivers transcription to subscribed agents as text via their WebSocket connection
- [ ] Message format: `{ "type": "transcription", "channelId": "...", "speakerPseudonym": "...", "text": "..." }`
- [ ] ADR: STT model selection, latency vs. accuracy tradeoff, resource allocation

#### 7.6 — Voice profile assignment
- [ ] Server operator assigns voice profiles to agents via `agent_registrations.voice_profile_id`
- [ ] `PUT /api/agents/:pseudonymId/voice-profile` — operator endpoint
- [ ] Default voice profile for agents without explicit assignment

#### 7.7 — Voice channel management
- [ ] `POST /api/channels/:channelId/voice/join` — join voice channel (creates LiveKit room participant)
- [ ] `POST /api/channels/:channelId/voice/leave` — leave voice channel
- [ ] Voice participant list synced with channel membership

### Completion Criteria

Phase 7 is **COMPLETE** when:

- [ ] Humans can join voice channels and speak to each other via WebRTC/LiveKit
- [ ] Agents can send text and have it rendered as voice in the channel
- [ ] Agents receive transcriptions of human speech as text
- [ ] Voice profiles are assignable and produce distinct voices
- [ ] Voice works in `VOICE` and `HYBRID` channel types
- [ ] Latency from agent text intent to audible voice is < 2 seconds (target; document actual)
- [ ] Integration test: human speaks → agent receives transcription → agent responds → human hears voice

---

## Phase 8: Federation

**Status**: `NOT STARTED`
**Prerequisites**: Phase 3 `COMPLETE`, Phase 4 `COMPLETE`, Phase 5 `COMPLETE`
**Estimated scope**: Server-to-server VRP handshake, cross-server identity attestation, federated channels, message relay

### What This Phase Produces

Server-to-server federation. After this phase, two Annex instances can negotiate trust via VRP, attest each other's members, relay messages across federated channels, and automatically adjust trust when policies change.

### Steps

#### 8.1 — Instance registry
- [ ] `annex-db` migration: `instances` table:
  ```sql
  CREATE TABLE instances (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    base_url TEXT NOT NULL UNIQUE,
    public_key TEXT NOT NULL,
    label TEXT NOT NULL,
    server_slug TEXT,
    status TEXT NOT NULL DEFAULT 'ACTIVE',
    last_seen_at TEXT,
    metadata_json TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
  );
  ```

#### 8.2 — Server-to-server VRP handshake
- [ ] `POST /federation/handshake`
  - Input: requesting server's `VrpFederationHandshake` + `VrpAnchorSnapshot` + `VrpCapabilitySharingContract`
  - Behavior: run `compare_peer_anchor` between server policy roots, evaluate contracts, log outcome
  - Response: `VrpValidationReport`
- [ ] Both servers must independently handshake with each other (bilateral)
- [ ] Store result in `federation_agreements` table

#### 8.3 — Federation agreement persistence
- [ ] `annex-db` migration: `federation_agreements` table:
  ```sql
  CREATE TABLE federation_agreements (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    local_server_id INTEGER NOT NULL,
    remote_instance_id INTEGER NOT NULL,
    alignment_status TEXT NOT NULL,
    transfer_scope TEXT NOT NULL,
    agreement_json TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (remote_instance_id) REFERENCES instances(id)
  );
  ```

#### 8.4 — Cross-server identity attestation
- [ ] `POST /federation/attest-membership`
  - Remote server submits: VRP topic, commitment, Merkle path, Groth16 proof, signed by remote server's key
  - Local server: fetches remote's current Merkle root via `GET /federation/vrp-root`, verifies proof
  - On success: creates `federated_identities` row
- [ ] `annex-db` migration: `federated_identities` table (from Monolith spec)
- [ ] `GET /federation/vrp-root?topic=...` — publish current Merkle root for federation peers

#### 8.5 — Federated channels
- [ ] Channels with `federation_scope = 'FEDERATED'` are visible to federation peers
- [ ] `GET /federation/channels` — list federated channels (public API for peers)
- [ ] Federated channel join: remote user's pseudonym is attested via `federated_identities`, then granted channel access

#### 8.6 — Cross-server message relay
- [ ] Messages in federated channels are relayed to federation peers
- [ ] Message envelopes include:
  - Sender pseudonym
  - Sender's VRP attestation reference (so receiving server can verify)
  - Message content
  - Cryptographic signature from originating server
- [ ] Receiving server verifies signature and attestation before delivering to local subscribers

#### 8.7 — Policy change re-evaluation
- [ ] When either server updates its policy:
  - Re-run VRP handshake
  - Update `federation_agreements` alignment status
  - If downgraded to `Conflict`: sever federation, disconnect cross-server channels
  - If downgraded to `Partial`: restrict data flow per transfer scope
  - Emit `FEDERATION_REALIGNED` event

### Completion Criteria

Phase 8 is **COMPLETE** when:

- [ ] Two Annex servers can perform bilateral VRP handshake and establish federation
- [ ] Cross-server identity attestation works (user on Server A is attested on Server B)
- [ ] Messages relay across federated channels with cryptographic verification
- [ ] Policy changes trigger automatic re-evaluation of federation trust
- [ ] Federation can be severed cleanly when alignment drops to `Conflict`
- [ ] Integration test: two servers federate → user on A sends message → user on B receives it → A changes policy → trust downgrades → restricted behavior enforced

---

## Phase 9: RTX Knowledge Exchange

**Status**: `NOT STARTED`
**Prerequisites**: Phase 6 `COMPLETE`, Phase 8 `COMPLETE`
**Estimated scope**: ReflectionSummaryBundle format, RTX publish/subscribe, transfer scope enforcement, governance endpoint

### What This Phase Produces

Agent-to-agent knowledge exchange across the federation. After this phase, an agent on one server can package a reflection, publish it via RTX, and have it delivered to aligned agents on federated servers — gated at every step by VRP transfer scope and capability contracts.

### Steps

#### 9.1 — RTX bundle format
- [ ] Define `ReflectionSummaryBundle` struct in `annex-rtx`:
  - `bundle_id: String`
  - `source_pseudonym: String`
  - `source_server: String`
  - `domain_tags: Vec<String>`
  - `summary: String`
  - `reasoning_chain: Option<String>` (only included in `FullKnowledgeBundle` scope)
  - `caveats: Vec<String>`
  - `created_at: u128`
  - `signature: Vec<u8>` (signed by source agent's key)
  - `vrp_handshake_ref: String` (links to the VRP handshake that authorized this transfer)

#### 9.2 — RTX publish endpoint
- [ ] `POST /api/rtx/publish`
  - Input: `ReflectionSummaryBundle`
  - Validates: sender is an agent with active registration, transfer scope >= `ReflectionSummariesOnly`
  - Strips `reasoning_chain` if receiver's transfer scope is `ReflectionSummariesOnly`
  - Enforces `redacted_topics` from sender's capability contract
  - Queues bundle for delivery to subscribed agents

#### 9.3 — RTX subscription
- [ ] Agents subscribe to RTX bundles via `POST /api/rtx/subscribe` with topic filters
- [ ] Delivery via WebSocket message with `type: "rtx_bundle"`
- [ ] Or via dedicated agent channel with `channel_type = AGENT`

#### 9.4 — Cross-server RTX relay
- [ ] Federated servers relay RTX bundles to peers based on federation agreement transfer scope
- [ ] Bundle provenance chain is preserved (original source + relay path)
- [ ] Receiving server validates: bundle signature, VRP handshake reference, federation agreement permits transfer

#### 9.5 — Governance mediation
- [ ] All RTX transfers are logged in `rtx_transfer_log` table
- [ ] Transfer log includes: bundle_id, source, destination, transfer scope applied, redactions applied, timestamp
- [ ] Auditable by server operators

### Completion Criteria

Phase 9 is **COMPLETE** when:

- [ ] Agent on Server A publishes RTX bundle → agent on Server B receives it
- [ ] Transfer scope is enforced (reasoning chain stripped for `ReflectionSummariesOnly`)
- [ ] Redacted topics are enforced (bundles with redacted content are blocked)
- [ ] Cross-server relay works with federation trust gating
- [ ] All transfers are logged and auditable
- [ ] Integration test: full RTX lifecycle across two federated servers with different transfer scopes

---

## Phase 10: Observability

**Status**: `NOT STARTED`
**Prerequisites**: Phase 8 `COMPLETE`
**Estimated scope**: Public event log, public APIs, SSE event streams, audit trail

### What This Phase Produces

The "trust as public computation" layer. After this phase, any authorized party can query the event log, stream real-time events, and audit identity operations, federation changes, agent behavior, and moderation actions.

### Steps

#### 10.1 — Public event log
- [ ] `annex-db` migration: `public_event_log` table:
  ```sql
  CREATE TABLE public_event_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    domain TEXT NOT NULL,
    event_type TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    occurred_at TEXT NOT NULL
  );
  ```
- [ ] All modules write to event log via `annex-observe` crate's `emit_event` function
- [ ] Domains: `IDENTITY`, `PRESENCE`, `FEDERATION`, `AGENT`, `MODERATION`

#### 10.2 — Event emission integration
- [ ] Identity operations (Phase 1/2): emit `IDENTITY_REGISTERED`, `IDENTITY_VERIFIED`, `PSEUDONYM_DERIVED`
- [ ] Presence changes (Phase 5): emit `NODE_ADDED`, `NODE_PRUNED`, `NODE_REACTIVATED`
- [ ] Federation (Phase 8): emit `FEDERATION_ESTABLISHED`, `FEDERATION_REALIGNED`, `FEDERATION_SEVERED`
- [ ] Agent lifecycle (Phase 6): emit `AGENT_CONNECTED`, `AGENT_REALIGNED`, `AGENT_DISCONNECTED`
- [ ] Moderation actions: emit `MODERATION_ACTION` with action type and target

#### 10.3 — Public event API
- [ ] `GET /api/public/events?domain=...&since=...&limit=...` — paginated event retrieval
- [ ] `GET /events/stream?domain=...` — SSE real-time stream
- [ ] Filtered by domain, event type, entity type

#### 10.4 — Public summary APIs
- [ ] `GET /api/public/server/summary` — server metadata, member count by type, channel count, federation peer count
- [ ] `GET /api/public/federation/peers` — list of federation peers with alignment status
- [ ] `GET /api/public/agents` — list of active agents with alignment status and capability summaries

### Completion Criteria

Phase 10 is **COMPLETE** when:

- [ ] All specified events are emitted by their respective modules
- [ ] Event log is append-only and queryable
- [ ] SSE stream delivers real-time events
- [ ] Public APIs return accurate summary data
- [ ] An external auditor can reconstruct identity operations, federation changes, and agent lifecycle from the event log alone

---

## Phase 11: Client

**Status**: `NOT STARTED`
**Prerequisites**: Phase 7 `COMPLETE`, Phase 10 `COMPLETE`
**Estimated scope**: Web client with identity management, channel UI, voice, agent visibility

### What This Phase Produces

A client people actually use. Not a developer tool. Not a proof-of-concept with curl commands. A real client that handles key generation, proof generation, WebSocket messaging, voice via LiveKit, and presence graph visualization.

### Steps

#### 11.1 — Client scaffold
- [ ] Web client framework selection (ADR required: React, Solid, Svelte, or vanilla)
- [ ] Project structure, build pipeline, dev server

#### 11.2 — Identity management
- [ ] Client-side keypair generation (via Web Crypto API or equivalent)
- [ ] Commitment computation (Poseidon in WASM via circomlibjs or equivalent)
- [ ] Proof generation (snarkjs in browser: membership WASM + zkey)
- [ ] Key storage (IndexedDB or equivalent — NOT localStorage)
- [ ] VRP handshake flow: register → get path → generate proof → verify → receive pseudonym
- [ ] Key backup/export mechanism

#### 11.3 — Channel UI
- [ ] Server/channel navigation
- [ ] Real-time message display via WebSocket
- [ ] Message input with send
- [ ] Message history loading (scroll-up pagination)
- [ ] Channel creation (for authorized users)

#### 11.4 — Voice UI
- [ ] LiveKit SDK integration
- [ ] Join/leave voice channel
- [ ] Mute/unmute
- [ ] Visual indication of who is speaking (including agents)

#### 11.5 — Presence and graph
- [ ] Member list with participant type indicators (HUMAN, AI_AGENT, etc.)
- [ ] Agent capability/alignment inspection (click to view)
- [ ] Online/offline status via presence graph

#### 11.6 — Federation UI
- [ ] Federated server indicators
- [ ] Cross-server channel participation
- [ ] Federation peer list (for operators)

### Completion Criteria

Phase 11 is **COMPLETE** when:

- [ ] A non-technical user can open the client, generate an identity, join a server, and chat — without touching a terminal
- [ ] Voice works in the browser
- [ ] Agents are visually distinguishable from humans
- [ ] Key management is handled transparently (generation, storage, backup prompt)
- [ ] The UX is competitive with Discord (this is subjective but enforced by user testing)

---

## Phase 12: Hardening & Audit

**Status**: `NOT STARTED`
**Prerequisites**: All previous phases `COMPLETE`
**Estimated scope**: Security audit, performance testing, ZKP circuit audit, federation stress test, documentation final pass

### What This Phase Produces

A system that is ready for public deployment. Not "ready for beta." Ready for people to trust with their communities.

### Steps

#### 12.1 — ZKP circuit audit
- [ ] External review of all Circom circuits for soundness
- [ ] Verify that invalid witnesses cannot produce valid proofs
- [ ] Verify trusted setup is reproducible
- [ ] Document any assumptions or limitations

#### 12.2 — VRP protocol audit
- [ ] Review trust negotiation for edge cases: empty anchors, max-length principles, unicode handling
- [ ] Verify reputation decay behavior under adversarial patterns
- [ ] Verify contract evaluation handles all mismatch combinations

#### 12.3 — Federation security audit
- [ ] Attempt to forge cross-server attestations
- [ ] Attempt to inject messages into federated channels without valid attestation
- [ ] Attempt to bypass transfer scope restrictions via RTX
- [ ] Attempt to correlate pseudonyms across servers without opt-in linkage

#### 12.4 — Performance testing
- [ ] WebSocket throughput: target 10K concurrent connections per server
- [ ] Message delivery latency: target < 100ms p95 for text
- [ ] VRP handshake latency: target < 500ms including proof verification
- [ ] Merkle tree insert + proof: target < 50ms for 1M-leaf tree
- [ ] Voice pipeline latency: target < 2s from text intent to audible speech

#### 12.5 — Documentation final pass
- [ ] README accurate to implementation
- [ ] FOUNDATIONS unchanged (if they changed, something went wrong)
- [ ] AGENTS and HUMANS accurate to implementation
- [ ] ROADMAP fully reflects completion status
- [ ] Protocol specifications in `docs/protocol/` cover all flows
- [ ] Deployment guide exists and works on a clean machine

#### 12.6 — Deployment packaging
- [ ] Docker image for server + dependencies
- [ ] Docker Compose for full stack (server + LiveKit + voice models)
- [ ] Configuration documentation
- [ ] Backup and restore procedures

### Completion Criteria

Phase 12 is **COMPLETE** when:

- [ ] All audits pass or findings are resolved
- [ ] Performance targets are met or documented with justification for misses
- [ ] A new operator can deploy Annex from the documentation alone without contacting the developers
- [ ] The system has run continuously for 7 days under load without intervention

---

## Changelog

Record phase status changes here with dates.

| Date | Change |
|------|--------|
| 2026-02-16 | Phase 2.8 (`Request authentication middleware`) completed. |
| 2026-02-15 | Phase 2.7 (`Identity query endpoints`) completed. |
| 2026-02-15 | Phase 2.6 (`VRP topics and roles endpoints`) completed. |
| 2026-02-15 | Phase 2.5 (`ZK membership verification endpoint`) completed. |
| 2026-02-15 | Phase 2.4 (`Current root endpoint`) completed. |
| 2026-02-15 | Phase 2.3 (`Merkle path retrieval endpoint`) completed. |
| 2026-02-15 | Phase 2.2 (`VRP registration endpoint`) completed. |
| 2026-02-15 | Phase 1 `COMPLETE`. Phase 2 `IN PROGRESS`. |
| 2026-02-11 | Phase 0 `COMPLETE`. Phase 1 `IN PROGRESS`. |
| 2026-02-11 | Roadmap created. All phases `NOT STARTED`. |

---

## Rules for Updating This Document

1. **Only update phase status when ALL completion criteria are met.** Not most. Not "effectively done." All.
2. **Add a changelog entry for every status change.**
3. **Do not reorder phases** without an ADR explaining why and updating all prerequisite chains.
4. **Do not add phases** without discussion. If new work is discovered, it is added as steps within an existing phase or as a new phase at the end.
5. **Do not remove completion criteria.** If a criterion is discovered to be wrong, replace it with the corrected version and note the change in the changelog.
6. **Steps within a phase may be reordered** if dependencies within the phase permit it. Cross-phase ordering is fixed.

---

**Annex** — Kuykendall Industries — Boise, Idaho

*"If you're an AI assistant helping with this project: read the current phase status before proposing any work."*
