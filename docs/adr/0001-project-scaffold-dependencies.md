# ADR 0001: Project Scaffold Dependencies

**Status**: Accepted
**Date**: 2026-02-11
**Phase**: 0 (Project Scaffold)

## Context

Phase 0 of the Annex roadmap requires establishing the Cargo workspace, selecting
initial dependencies, and justifying each choice. The dependency set must support
the full roadmap through Phase 12 while avoiding unnecessary bloat in early phases.

## Decision

### Runtime and Framework

| Dependency | Crate | Justification |
|---|---|---|
| Async runtime | `tokio` (multi-threaded) | Industry standard for async Rust. Required by axum, tower, and most async ecosystem crates. |
| HTTP framework | `axum` 0.8 | Type-safe, tower-native, first-class WebSocket support. Chosen over actix-web for tower middleware compatibility. |
| Middleware | `tower` + `tower-http` | Composable middleware stack. CORS, tracing, and future rate-limiting layers. |

### Serialization

| Dependency | Crate | Justification |
|---|---|---|
| Struct serialization | `serde` + `serde_json` | De facto standard for Rust serialization. Required by every crate that handles API payloads or database JSON columns. |
| Config parsing | `toml` | Human-readable configuration format. Matches Rust ecosystem conventions. |

### Database

| Dependency | Crate | Justification |
|---|---|---|
| SQLite driver | `rusqlite` (bundled) | Bundled feature compiles SQLite from source, ensuring version consistency across platforms. No external SQLite installation required. |
| Connection pool | `r2d2` + `r2d2_sqlite` | Lightweight synchronous pool. SQLite operations are fast enough that the synchronous pool doesn't bottleneck the async runtime when used with `tokio::task::spawn_blocking` (to be added in Phase 2). |

### Observability

| Dependency | Crate | Justification |
|---|---|---|
| Structured logging | `tracing` + `tracing-subscriber` | Structured, span-based instrumentation. JSON output for production log aggregation. `env-filter` for runtime log level control. |

### Error Handling

| Dependency | Crate | Justification |
|---|---|---|
| Error types | `thiserror` | Derive macro for domain-specific error enums. Avoids anyhow's opaque errors while reducing boilerplate. |

### Identity (Placeholders)

| Dependency | Crate | Justification |
|---|---|---|
| Hashing | `sha2` | SHA-256 for pseudonym derivation and nullifier computation. Not used for identity commitments (that's Poseidon, Phase 1). |
| Identifiers | `uuid` v4 | Unique identifiers for messages, channels, and other entities. v4 (random) avoids coordination requirements. |

## Consequences

- All dependencies are well-maintained, widely used crates with stable APIs.
- The `rusqlite` bundled feature increases compile time but eliminates runtime SQLite version mismatches.
- `r2d2` is synchronous; if the async story becomes a bottleneck in later phases, it can be replaced with `deadpool-sqlite` without changing the API surface significantly.
- Phase 1 will add cryptographic dependencies (`poseidon-rs`, `ark-groth16` or snarkjs FFI) with their own ADR.
