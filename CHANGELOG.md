# Changelog

All notable changes to Annex are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

---

## [0.1.0] — 2026-02-24

First packaged release. Developer preview — not all features are production-ready. See [release_v0.1.md](release_v0.1.md) for the full release notes.

### Added

- Rust server (`tokio` + `axum`) with SQLite storage and automatic migrations
- Self-sovereign ZKP identity: Poseidon(BN254) commitments, Groth16 membership proofs, topic-scoped pseudonyms
- Text channels with WebSocket delivery, append-only message storage, message edit/delete
- Voice channels via LiveKit SFU with Piper TTS and Whisper STT
- Five channel types: Text, Voice, Hybrid, Agent, Broadcast
- Five participant types: Human, AI Agent, Collective, Bridge, Service
- File uploads with magic-byte content-type detection and EXIF metadata stripping
- Link previews with privacy-preserving server-side image proxy
- Federation protocol: VRP handshake, Merkle root exchange, signed message relay, RTX transport
- Agent framework: VRP handshake, alignment classification, capability contracts, voice profiles
- Observability: append-only event log, SSE streaming, public summary APIs
- Tauri desktop app with auto-start server, cloudflared tunnel, zero-click startup
- Docker image with multi-stage build and non-root user
- Deploy scripts for Linux/macOS (`deploy.sh`) and Windows (`deploy.ps1`)
- TOML config file + environment variable overrides
- GitHub Actions workflow for desktop app releases

### Security

- SSRF protection: private IP blocking + DNS rebinding checks on all outbound HTTP
- Content-Security-Policy, X-Frame-Options, X-Content-Type-Options headers
- Configurable CORS (restrictive by default)
- Rate limiting with periodic cleanup and automatic eviction
- Ed25519 request signing for federation messages
- Signing key auto-persistence with 0600 file permissions
- Upload handler uses magic-byte detection only (declared Content-Type ignored)
- Memory leak fix in upload handler (removed `Box::leak` for unknown MIME types)
- Docker credentials moved to environment variables

### Known Gaps

- Federation: `process_incoming_handshake` is stubbed; RTX cross-server delivery is local-only; policy changes don't trigger automatic re-handshakes
- Agent VRP: semantic alignment defaults to Conflict (no real embedder); ZK proofs load but aren't enforced at channel access points
- Voice: agent voice pipeline is file-based, not real-time WebRTC; Bark/System TTS return "not implemented"
- See [release_v0.1.md](release_v0.1.md) for complete details
