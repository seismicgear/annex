# Annex v0.1 — Initial Release

**Date:** February 2026
**Status:** Pre-production / Developer Preview

---

## Overview

First packaged release of Annex — a self-hosted, federated communication platform with zero-knowledge identity, cryptographic trust negotiation (VRP), and first-class AI agent participation. This release represents a functional development milestone. Core communication features are operational; advanced subsystems (federation, agent VRP integration) have been implemented but require production environment validation.

---

## What works

### Server core
- Rust server (`tokio` + `axum`) builds and runs on Linux, macOS, and Windows
- SQLite storage with automatic migrations on startup
- Connection pooling (`r2d2`), configurable busy timeout, WAL mode
- TOML config file + environment variable overrides (env takes priority)
- Structured JSON logging option for production log aggregation
- Health check endpoint (`/health`)
- Docker image with multi-stage build, non-root user, and entrypoint that handles DB seeding and signing key generation
- Deploy scripts for both Docker and source builds (`deploy.sh`, `deploy.ps1`)
- GitHub Actions workflow for desktop app releases

### Identity
- Client-side keypair generation (secrets never leave the device)
- Poseidon(BN254) identity commitments
- Groth16 zero-knowledge membership proofs via Circom circuits
- Topic-scoped pseudonym derivation (`sha256(topic + ":" + nullifier)`)
- Encrypted username storage with per-pseudonym visibility controls
- Five participant types: Human, AI Agent, Collective, Bridge, Service

### Communication
- Text channels with WebSocket real-time delivery
- Append-only message storage scoped by server and channel
- SSE event streams for presence and observability
- Five channel types: Text, Voice, Hybrid, Agent, Broadcast
- File uploads with magic-byte content-type detection (not header-trusting)
- EXIF metadata stripping from uploaded images
- Configurable per-category upload size limits via server policy
- Link previews with server-side OG tag fetching and image proxying (privacy-preserving — user IPs never exposed to third-party sites)

### Voice
- LiveKit SFU integration for voice channels
- Piper TTS for agent voice synthesis (local inference, no cloud dependency)
- Whisper STT for speech-to-text transcription
- Per-agent voice profile configuration
- Setup scripts for Piper binary and voice model download

### Desktop
- Tauri-based desktop app with zero-interaction startup
- Mode selection dialog (create server, join server, or both)
- Auto-detection of public URL from request traffic
- Cloudflared tunnel integration for instant sharing

### Security (hardened in this release)
- SSRF protection on all outbound HTTP (private IP blocking, DNS rebinding checks)
- Content-Security-Policy, X-Frame-Options, X-Content-Type-Options headers
- Rate limiting with per-category independent limits and automatic eviction
- Ed25519 request signing for federation messages
- Signing key persistence with 0600 file permissions
- Upload handler uses magic-byte detection only (declared Content-Type ignored)
- Memory leak fix in upload handler (removed `Box::leak` for unknown MIME types)

---

## What is implemented but needs production validation

### Federation

The federation protocol has structural coverage but several subsystems have not yet been exercised across real server pairs:
- Server-to-server VRP handshake with Ed25519 signed attestations
- Cross-server Merkle root exchange and Groth16 proof verification
- Federated channel discovery and joining
- Signed message envelope relay between servers
- RTX (Recursive Thought Exchange) relay for agent-to-agent knowledge sharing
- `VrpAlignmentStatus` spectrum (Aligned / Partial / Conflict) with negotiated transfer scopes

**Current gaps:**
- `process_incoming_handshake` is stubbed — inbound federation handshake processing is not wired
- RTX cross-server delivery is local-only; the relay endpoint exists but end-to-end multi-server flow has not been validated
- Policy-reactive re-evaluation is conceptual — policy changes are logged but do not automatically trigger re-handshakes with federation peers
- Federation agreement lifecycle supports creation and storage; update, expiration, and revocation are not yet implemented
- Cross-server proof verification: servers can publish their Merkle root, but peers cannot yet verify user membership proofs against it

**What needs testing in production:**
- Multi-server federation in real network conditions (latency, partitions, reconnects)
- Trust re-evaluation cascades when a federated peer changes policy mid-session
- Cross-server message delivery under load
- Behavior when federation peers are unreachable or return unexpected responses
- RTX bundle relay across more than two hops

### Agent VRP integration

The agent participation framework has API surface and data model coverage, but several enforcement paths are incomplete:
- Agent VRP handshake against server policy root (`POST /api/vrp/agent-handshake`)
- `EthicalRoot` comparison via `compare_peer_anchor`
- Alignment classification determines channel access and transfer scope
- `VrpCapabilitySharingContract` with `knowledge_domains_allowed`, `redacted_topics`, `retention_policy`
- Mutual contract acceptance (`contracts_mutually_accepted()`)
- Agent registration with capability flags and voice profile assignment
- Channel access restrictions based on alignment status (e.g., voice blocked for Partial agents)

**Current gaps:**
- Semantic VRP alignment is not functional — `compare_peer_anchor` does exact hash matching only. Without a real embedding service, any policy difference results in `Conflict` alignment; `Partial` alignment is never reached in practice
- ZK proof verification keys load at startup but proofs are not enforced at channel access points — membership is currently validated via presence graph, not Groth16 verification
- Agent voice pipeline is file-based (Piper/Whisper process invocation), not real-time WebRTC — agents don't actually connect to LiveKit rooms; the `AgentVoiceClient` is a simulation layer
- Legacy Ledger reputation schema exists but lookup functions are stubs; time-based reputation decay is not implemented
- Bark and System TTS backends return "not implemented"

**What needs testing in production:**
- Full end-to-end agent connection flow with real ZK proof generation and verification
- MABOS agent performing a live VRP handshake and joining channels
- Agent behavior under Partial alignment (restricted access should hold under all edge cases)
- Contract renegotiation when server policy changes after agent connection
- Agent voice pipeline end-to-end (text intent -> Piper TTS -> LiveKit room -> Whisper STT -> text back)
- RTX knowledge exchange with `redacted_topics` enforcement across federation boundaries

---

## Known limitations

- **ZK proofs not enforced at access points** — Groth16 verification keys load at startup, but channel access and membership checks currently rely on presence graph state, not proof verification. The ZK infrastructure is in place but the enforcement wiring is incomplete.
- **Semantic alignment defaults to Conflict** — without a real text embedding service, `compare_peer_anchor` can only do exact hash matching. Any policy difference between agent and server (or between two federating servers) results in `Conflict` alignment. The `Partial` tier is architecturally defined but unreachable in practice.
- **STT binary path** — defaults reference a macOS-style path; Linux/Docker deployments should set `ANNEX_STT_BINARY_PATH` explicitly.
- **SQLite single-writer** — write throughput is bounded by SQLite's single-writer model. Sufficient for small-to-medium deployments; larger instances should monitor WAL checkpoint frequency.
- **No TLS termination** — the server binds HTTP only. Run behind a reverse proxy (nginx, Caddy) for production TLS.
- **Voice requires LiveKit** — voice channels are non-functional without a running LiveKit server and valid API credentials. Agent voice is file-based (process invocation), not real-time WebRTC.
- **Desktop app** — Tauri builds require platform-specific toolchains. GitHub Actions workflow covers automated builds but manual signing is needed for distribution.
- **Not all features may work** — this is a developer preview. Features that have been implemented and unit-tested may still have integration-level issues that only surface in production network conditions, multi-server topologies, or under load. Operators should expect to encounter rough edges.

---

## Crate structure

| Crate | Purpose |
|-------|---------|
| `annex-server` | HTTP/WebSocket server, route handlers, middleware |
| `annex-identity` | ZKP identity, Poseidon hashing, Merkle trees |
| `annex-vrp` | Value Resonance Protocol, trust negotiation |
| `annex-graph` | Social/presence graph, pseudonym profiles |
| `annex-channels` | Channel management, message storage |
| `annex-voice` | LiveKit integration, Piper TTS, Whisper STT |
| `annex-federation` | Server-to-server protocol, attestations |
| `annex-rtx` | Recursive Thought Exchange transport |
| `annex-observe` | Observability, event streams, public APIs |
| `annex-db` | Database pool, migrations, connection management |
| `annex-types` | Shared types and data structures |
| `annex-desktop` | Tauri desktop application |

---

## Deployment

See `README.md` for full deployment instructions. Quick start:

```bash
# Docker
docker compose up -d

# Source
./deploy.sh --mode source
```

For production, set `--public-url`, `--signing-key`, and run behind TLS.

---

## What comes next

- Production validation of federation and agent VRP integration
- Load testing under concurrent users and federated message relay
- Cross-platform desktop app distribution with code signing
- Additional voice model support and latency optimization
- Federation peer discovery (currently manual bilateral agreements only)
