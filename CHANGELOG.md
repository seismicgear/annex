# Changelog

All notable changes to Annex are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

---

## [Unreleased]

### Fixed

- **Startup: the ZK-proof screen no longer looks frozen.** A first-run Groth16
  proof can take 30–60s; the registration/proving screen previously showed only
  a static label. It now has an animated spinner plus a live elapsed-time hint
  (respects reduced-motion). (AUDIT P4-UX-1)
- **Desktop host-mode voice now works out of the box.** `start_local_webrtc`
  previously tried to download a `webrtc-server` binary from a GitHub release
  URL that does not exist (404), and that failure disabled voice on every
  default desktop host install. Annex's SFU is embedded in-process, so the
  download was never needed — host mode now simply enables the in-process SFU
  via a loopback `[webrtc]` override (dead download machinery removed).
  (AUDIT P4-VOICE-2)

- **Desktop (all platforms): packaged app now locates its bundled ZK
  verification key.** Tauri stores `bundle.resources` under a mangled
  `<resource_root>/_up_/_up_/…` path that is not beside the executable; the
  desktop app previously only probed exe-relative/dev paths, so an installed
  deb/AppImage/NSIS/.app could not find `membership_vkey.json` and — with the
  default `enforce_zk_proofs = true` — refused to start the embedded server.
  Resolution now covers the real per-platform resource roots (Linux
  `…/lib/Annex`, Windows beside-exe, macOS `Contents/Resources`). Piper and
  voice-model resolution were fixed the same way. (AUDIT FINDING-038)
- **Desktop: clean shutdown.** The app now kills the spawned `webrtc-server`
  child and releases the Annex router public-endpoint session on exit, instead
  of orphaning the process (holding its port) and leaving a public tunnel
  advertised for a dead local server. (AUDIT FINDING-039)

### Changed

- **Federation: agreement TTL is now enforced.** `expire_stale_agreements` was
  implemented and tested but never called; a new hourly background task now
  deactivates federation agreements whose `updated_at` is older than
  `federation.agreement_ttl_days` (default 30; 0 disables). Re-handshakes and
  policy re-evaluations refresh `updated_at`, so only silent peers are reaped.
- **VRP: `alignment_score` now reports the real measured similarity.** It was
  hardcoded to 1.0/0.5/0.0 from the discrete status (a "placeholder"); the
  validation report now carries the actual anchor similarity (exact match,
  prohibited-action divergence, or the bag-of-words cosine), independent of the
  final verdict. (AUDIT P4-VRP-2)
- **VRP: longitudinal reputation now gates alignment outcomes.** Previously the
  handshake verdict was computed and reputation was read afterward and ignored
  (so an agent with a long history of Conflict was treated like a fresh one).
  The server now reads reputation from prior history before recording the
  current outcome and downgrades the alignment one step when a peer's sustained
  Partial/Conflict history has driven its reputation below the neutral band.
  (AUDIT P4-VRP-1)

### Security

- **Identity: the three advertised privacy circuits now exist and are enforced
  in zero knowledge.** `channel_eligibility`, `link_pseudonyms`, and
  `federation_attestation` previously did not exist — the README sold
  capability/linkage/attestation privacy that had no cryptographic backing
  (channel gating read plaintext role flags; cross-server attestation was a
  metadata record). All three are now real Circom/Groth16 circuits over the
  existing Poseidon(BN254) tree, with domain-separated nullifiers, verified by
  new `POST /api/zk/{channel-eligibility,link-pseudonyms,federation-attestation}`
  endpoints that bind the role/topic/context and root before accepting a proof.
  The client generates them and the desktop bundle ships the keys. Proven by 18
  new circuit assertions and four router integration tests driving real proofs.
  (Trusted setup is still the dev-fixture; a multi-party ceremony is the
  remaining production step.) (AUDIT P4-ID-1)
- **Identity: the client now uses secret-derived (v2) nullifiers by default,
  closing the disclosed nullifier-linkability hole.** The v1 per-topic
  nullifier was `sha256(commitment + topic)` — derivable by anyone holding the
  public Merkle leaf, so a registry snapshot exposed every topic pseudonym. The
  client now generates a v2 Groth16 proof whose nullifier is
  `Poseidon(sk, topicHash, 1)` computed INSIDE the circuit, so the commitment no
  longer reveals the pseudonym. The server accepts both v1 and v2 by default
  (`security.enabled_zk_versions = ["v1","v2"]`) for migration; the desktop
  bundles the v2 verification key. (AUDIT FINDING-003 / P4-ID-2)
- **Federation: RTX relay signatures now bind bundle content.** The relay
  envelope signature previously covered only metadata, so a relaying or
  man-in-the-middle peer could rewrite a relayed bundle's content, tags, author,
  or timestamp without invalidating it. The signing payload now includes a
  length-prefixed SHA-256 hash of the bundle content, which the receiver
  recomputes from the bundle it received — any tampering fails verification.
  (Per-agent author-signature verification remains future work.) (AUDIT P4-FED-1)
- **Server: refuse to demote the last active moderator.** `PATCH
  /api/admin/members/{id}/capabilities` now returns `409 Conflict` rather than
  letting a moderator strip moderation from every admin and lock the server
  out of administrative control. (AUDIT FINDING-040)

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
- Tauri desktop app with auto-start server, Annex router public endpoint, zero-click startup
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

- Federation: RTX multi-hop chains lack origin validation; policy changes trigger local re-evaluation but don't proactively notify peers; agreement lifecycle lacks manual revocation/expiration
- Agent VRP: semantic alignment uses bag-of-words similarity (Partial tier reachable); ZK proof enforcement at channel access is opt-in (`enforce_zk_proofs` config)
- Voice: agent voice connects to LiveKit rooms; Bark TTS uses Python subprocess; System TTS uses platform-native commands
- See [release_v0.1.md](release_v0.1.md) for complete details
