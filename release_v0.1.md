# Annex v0.1 — Initial Release

**Date:** February 2026
**Status:** Pre-production / Developer Preview — **historical record, superseded**

> **Read this before acting on anything below.** These are the notes for the
> February 2026 release, kept as a dated record. They are not a description of
> the current tree, and one difference matters enough to call out here rather
> than leave for the reader to discover:
>
> **Voice no longer uses LiveKit, and never requires it.** Every mention of
> LiveKit on this page — the feature list, the agent voice pipeline, the crate
> table, and especially the "Voice requires LiveKit" known limitation — is
> obsolete. Voice is a native WebRTC SFU compiled into the Annex binary
> (`crates/annex-voice/src/service.rs`, built on `webrtc-rs`), signalling over
> the app's own `/ws` WebSocket. There is no media server to install and no API
> credentials to obtain. Do not deploy a LiveKit server on the strength of this
> document.
>
> For current voice operation see [`docs/deployment.md`](docs/deployment.md).

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
- ~~LiveKit SFU integration for voice channels~~ → native in-process WebRTC SFU (`webrtc-rs`); see banner
- Piper TTS for agent voice synthesis (local inference, no cloud dependency)
- Whisper STT for speech-to-text transcription
- Per-agent voice profile configuration
- Setup scripts for Piper binary and voice model download

### Desktop
- Tauri-based desktop app with zero-interaction startup
- Mode selection dialog (create server, join server, or both)
- Auto-detection of public URL from request traffic
- Annex router integration for automatic public endpoint provisioning

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
- RTX cross-server delivery works single-hop; multi-hop chains lack origin validation and circular relay prevention
- Policy changes trigger local re-evaluation of federation agreements and agent alignment, but do not proactively notify federation peers or initiate re-handshakes
- Federation agreement lifecycle supports creation, storage, and policy-driven updates; manual revocation, expiration TTLs, and graceful deactivation are not yet implemented
- Cross-server proof verification: initial attestation (Groth16 proof against remote Merkle root) works; continuous verification (re-checking on root changes) and revocation are not yet implemented

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
- Semantic VRP alignment uses bag-of-words text similarity when original principle text is available; exact hash matching is used as a fast path. The `Partial` tier is reachable when principles overlap but are not identical
- ZK proof verification keys load at startup; enforcement at channel access points is configurable via `enforce_zk_proofs` (~~default: off for backward compatibility~~ — **it now defaults to `true`**; see `default_enforce_zk_proofs` in `crates/annex-server/src/config.rs`)
- Agent voice pipeline uses ~~LiveKit~~ the in-process SFU for real-time WebRTC room participation, with synthesized PCM written straight into the mixer; TTS (Piper, Bark, System) and STT (Whisper) are subprocess-based
- Reputation scoring (`check_reputation_score`) is implemented with algorithmic exponential decay over handshake history; wall-clock-based TTL decay is not yet implemented
- Bark TTS uses a Python subprocess wrapper; System TTS uses platform-native commands (espeak-ng on Linux, `say` on macOS)

**What needs testing in production:**
- Full end-to-end agent connection flow with real ZK proof generation and verification
- MABOS agent performing a live VRP handshake and joining channels
- Agent behavior under Partial alignment (restricted access should hold under all edge cases)
- Contract renegotiation when server policy changes after agent connection
- Agent voice pipeline end-to-end (text intent -> Piper TTS -> ~~LiveKit room~~ in-process SFU room -> Whisper STT -> text back)
- RTX knowledge exchange with `redacted_topics` enforcement across federation boundaries

---

## Known limitations

- ~~**ZK proof enforcement is opt-in**~~ — **no longer true.** `enforce_zk_proofs` now defaults to **`true`** (`crates/annex-server/src/config.rs::default_enforce_zk_proofs`), and a regression test asserts it. Clients submit proofs via the `x-annex-zk-proof` header. Do not read this line as licence to run with enforcement off.
- **Semantic alignment** — `compare_peer_anchor` uses bag-of-words text similarity for the `Partial` tier when original principle text is available; falls back to exact hash matching otherwise. Requires principle text in anchor snapshots (not just hashes) for partial alignment to activate.
- **STT binary path** — defaults to `assets/whisper/whisper` (platform-agnostic relative path). The Whisper binary is not bundled in Docker (unlike Piper TTS); Docker deployments should set `ANNEX_STT_BINARY_PATH` explicitly or use the provided Dockerfile which bundles it.
- **SQLite single-writer** — write throughput is bounded by SQLite's single-writer model. Sufficient for small-to-medium deployments; larger instances should monitor WAL checkpoint frequency.
- **No TLS termination** — the server binds HTTP only. Run behind a reverse proxy (nginx, Caddy) for production TLS.
- ~~**Voice requires LiveKit**~~ — **no longer true, and never do this.** Superseded by the native in-process SFU; see the banner at the top of this page. Voice channels require no external media server and no API credentials.
- **Desktop app** — Tauri builds require platform-specific toolchains. CORS origins for the Tauri webview are automatically configured. GitHub Actions workflow covers automated builds but manual signing is needed for distribution.
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
| `annex-voice` | ~~LiveKit integration~~ native WebRTC SFU (`webrtc-rs`), Piper TTS, Whisper STT |
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
