# Contributing to Annex

Contributions are welcome. This document explains how to build, test, and submit changes.

---

## Prerequisites

| Tool | Version | Notes |
|------|---------|-------|
| Rust | 1.85+ | Install from [rustup.rs](https://rustup.rs) |
| Node.js | 22+ | For client builds |
| sqlite3 | Any | For database seeding (optional — deploy scripts handle this) |

## Build

### Server

```bash
cargo build --release -p annex-server
```

### Client

```bash
cd client
npm install
npm run build
```

### Desktop (Tauri)

```bash
cd client
npm run tauri build
```

### Full workspace

```bash
cargo build --workspace
```

## Test

```bash
cargo test --workspace
```

Some tests require ZK trusted setup keys in `zk/keys/`. Tests that depend on these keys are skipped when the keys are absent. This is expected in most development environments.

To run client tests:

```bash
cd client
npm test
```

## Code Style

### Rust

- Run `cargo fmt` before committing
- Run `cargo clippy` and resolve all warnings
- No `unsafe` without justification and a safety comment
- No `unwrap()` in production code paths — use proper error handling

### TypeScript / React

- Project ESLint and Prettier configs are in `client/`
- Run `npm run lint` to check

## Architecture

The workspace is organized into focused crates:

```
crates/
├── annex-server      # HTTP/WebSocket server, route handlers, middleware
├── annex-identity    # ZKP identity, Poseidon hashing, Merkle trees
├── annex-vrp         # Value Resonance Protocol, trust negotiation
├── annex-graph       # Social/presence graph, pseudonym profiles
├── annex-channels    # Channel management, message storage
├── annex-voice       # LiveKit integration, Piper TTS, Whisper STT
├── annex-federation  # Server-to-server protocol, attestations
├── annex-rtx         # Recursive Thought Exchange transport
├── annex-observe     # Observability, event streams, public APIs
├── annex-db          # Database pool, migrations, connection management
├── annex-types       # Shared types and data structures
└── annex-desktop     # Tauri desktop application
```

Architecture decision records live in `docs/adr/`. Read these before proposing changes to foundational subsystems.

## Submitting a Pull Request

1. Fork the repository and create a branch from `main`
2. Make your changes with clear, descriptive commits
3. Ensure `cargo fmt`, `cargo clippy`, and `cargo test --workspace` pass
4. Open a PR with:
   - A description of what changed and why
   - A test plan (how you verified the change works)
   - Note any database migration changes
   - Note any configuration changes
5. Reference a GitHub issue if one exists

### What gets extra review

Changes to these subsystems are security-critical and receive additional scrutiny:

- ZK circuits (`zk/circuits/`)
- VRP trust negotiation (`crates/annex-vrp/`)
- Federation protocol (`crates/annex-federation/`)
- Authentication and authorization middleware (`crates/annex-server/src/middleware.rs`)
- Cryptographic operations (signing, hashing, proof verification)
- Database migrations (`crates/annex-db/`)

If your PR touches these areas, flag it in the description.

### What will get rejected

- Changes that violate [FOUNDATIONS.md](FOUNDATIONS.md) — these are non-negotiable
- Surveillance, telemetry, or behavioral tracking
- Mandatory identity disclosure mechanisms
- Centralized control or kill-switch mechanisms
- Changes that weaken ZK proof guarantees or VRP trust evaluation

Read FOUNDATIONS.md before proposing architectural changes. If a proposal conflicts with those principles, it will be rejected regardless of technical merit.

## Database Migrations

Migrations live in `crates/annex-db/` and run automatically on server startup. If your change requires a schema change:

- Add a new migration (never modify existing ones)
- Migrations must be idempotent (`CREATE TABLE IF NOT EXISTS`, `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`)
- Test with a fresh database and with an existing database from the previous version

## License

By contributing, you agree that your contributions are licensed under the [Annex Noncommercial + Protocol-Integrity License](LICENSE.md). This is not an MIT/Apache project — read the license before contributing.

---

Questions? Open a discussion or email contact@montgomerykuykendall.com.
