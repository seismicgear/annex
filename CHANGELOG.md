# Changelog

All notable changes to Annex are recorded here, in the format of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

There was no changelog before `0.1.0`. `release_v0.1.md` is the historical
record of what was prepared in February 2026 and never tagged; it carries a
supersession banner and is kept for provenance rather than maintained.

## [Unreleased]

### Fixed

- **Live captions transcribed nothing, in every deployment, and always had.**
  Three independent causes, each sufficient: the SFU tap handed whisper.cpp
  headerless 48 kHz PCM, which its loader rejects before reading a sample (it
  requires a 16 kHz 16-bit WAV container); the tap loop spawned one whisper
  process per 20 ms RTP packet, over a window shorter than whisper returns
  anything for; and the Docker image installed whisper.cpp v1.7.4's
  `build/bin/main`, which in that tag is the deprecation stub. `stt_ready`
  reported `true` for the stub because it only tested `is_file()`, and the
  per-frame failure was logged at DEBUG. Audio is now band-limited, resampled
  and wrapped (`annex-voice::audio`), buffered per `(channel, speaker)` and
  flushed at 2 s or 700 ms of quiet, and the build refuses a binary that
  prints a deprecation notice. `SttReadiness` / `stt_detail` name which of the
  four causes a server has, and the caption strip renders it.
- **`agent_min_alignment_score` shipped at a value no genuine peer could
  reach.** 0.8 against a raw cosine, where the pinned model's unrelated pairs
  top out at 0.5134 and the lexicon's at 0.3060 — above both separating bands,
  so the only agents ever admitted on the semantic path were those matching by
  anchor hash. Scores are normalised against the loaded scorer's measured noise
  floor and the default is 0.06 on that scale; migrations 046 and 047 carry
  stored thresholds and stored verdicts across.
- **An agent swept to `Conflict` kept working.** The sweep set
  `agent_registrations.active = 0` and closed one socket;
  `platform_identities.active` and `channel_members` were untouched, so the
  agent's session token still verified and a reconnect restored everything.
  Alignment is now checked at action time (`services/agent_policy.rs`) for
  send, edit, delete, voice media, VoiceIntent and channel creation, and a
  `Conflict` verdict bumps the token epoch in the same transaction that
  deactivates the registration.
- **The WebSocket send handler reported every refusal as "internal error".** A
  non-member, an alignment refusal, an oversized body and a tripped storage
  gate all came back identically, and all were logged at ERROR. Four of the
  five are the caller's doing.
- **Multi-hop RTX relay did not exist**, and ROADMAP Phase 9 recorded the two
  guards for it as complete. `relay_rtx_bundles` reset `relay_path` to
  `vec![local_public_url]` on every send, so the list could never hold more than
  one entry and the cycle check could only detect a cycle back to this server;
  `receive_federated_rtx` stored the bundle, fanned it out locally and stopped,
  so nothing re-relayed. Now: a signed hop chain where each hop covers the
  previous hop's digest, an origin attestation over a scope-invariant content
  digest plus a reasoning-chain commitment (so a relayer may strip a reasoning
  chain for policy and cannot add one), a TTL from `rtx_max_hops` bounded by
  `RTX_HOP_CEILING`, and loop prevention that can see more than one hop. See
  `docs/protocol/rtx-relay.md`.
- **The RTX relay's SSRF gate ignored `allow_private_peer_addresses`** while the
  message relay honoured it, so an operator who set that flag — documented as
  what makes two servers on a LAN, two containers on a Compose network, and
  peers across a VPN possible — got messages relayed and RTX bundles silently
  dropped at the same peer. The comment beside the call site claimed to mirror
  the message path, which is how the divergence survived.
- **`zk/scripts/test-proofs.js` had been failing since the challenge landed**
  — it built its v2 witness without the `challenge` public input — while
  `release-gates.md` and `invariants.md` quoted "16/16 must pass". It now also
  asserts the challenge binding: a proof presented against a different
  challenge is rejected, and a second challenge needs a second proof.
- **`scripts/verify-production-rejects-dev-fixtures.sh` ran in no workflow.**
  The globbed harness step matches `scripts/tests/*.test.sh`; it is
  `scripts/*.sh`. Twelve assertions about whether a release can ship
  dev-fixture ZK keys, invoked by hand. It is an explicit step in CI and in
  `test-all.sh` now, and asserts both callers exist.
- **`scripts/tests/ceremony-verifier.test.sh` corrupted the repository.** Its
  `restore()` deleted the backup as it copied, so every mutation after the
  first stayed on disk; a run left the tracked ceremony transcript carrying a
  `1111…` beacon signature, which failed `verify-ceremony.js` and the
  production ZK gate with it. Four of its six negative cases also addressed
  `phase1.beacon`, where there is no beacon, so they tested nothing while
  failing loudly. It now restores after every mutation and asserts the
  transcript is byte-identical to how it found it.

### Changed

- `[profile.dev.package."*"] debug = 0`. A complete
  `cargo test --workspace --exclude annex-desktop` needed 12.6 GB of test
  binaries inside a 19 GB `target/` and died in the linker with "No space left
  on device"; dependency line tables were the bulk of it and nobody steps
  through `webrtc-rs` from a failing Annex test. Now 7.8 GB and 11 GB.
  Workspace crates keep `line-tables-only`.

## [0.1.0] — unreleased

The first version intended to be tagged. Everything below `0.1.0` was
development; no `v*` tag has ever existed in this repository.

### Zero-knowledge identity

- **Trusted setup ceremony.** `zk/scripts/ceremony.js` produces the Groth16
  proving and verification keys a release ships: phase 1, a phase-2
  contribution per circuit, and finalisation with a **drand round committed to
  before that round exists** — published together with the hashes of the
  pre-beacon artifacts, so nobody, including whoever ran it, could know the
  beacon while choosing a contribution.

  It is a **single-operator** ceremony and says so: `ceremony.type` is
  `single-contributor-beacon`, never `mpc`. Independent participants are
  strictly stronger and remain the target; `--contributors N` and
  `--ptau <file> --ptau-sha256 <hex>` (which adopts a real perpetual Powers of
  Tau, hash-checked then `powersoftau verify`-ed) are the upgrade path, and
  neither changes anything downstream.

- **Provenance is checked, not asserted.** `verify-ceremony.js` runs
  `snarkjs zkey verify` over r1cs → ptau → every contribution → beacon,
  re-derives the verification key from the proving key rather than trusting the
  shipped one, and checks the transcript's drand round against what was
  published. `verify-artifacts.js` gains `--all`, an allowlist of ceremony
  types, and a requirement that any non-fixture claim name a transcript that
  exists on disk. `install-ceremony.js` re-hashes every copy it makes.

### Security

- **`ANNEX_ENFORCE_ZK_PROOFS=false` no longer starts a production server.** It
  was the only dangerous setting with no production gate, and what it disables
  is authentication: the server then accepts a raw pseudonym as a Bearer token
  and as a WebSocket query parameter, against pseudonyms it serves publicly.
  There is no escape-hatch variable; a deployment that wants proofs off runs a
  dev profile and says so.

- **The production posture defaults to ON.** Four gates each read
  `ANNEX_BUILD_PROFILE` themselves and returned success when it was unset, so
  the whole posture was opt-in through a variable nothing in `deploy.sh`,
  `deploy.ps1` or the operator documentation ever set — and a typo in it was
  indistinguishable from a dev profile. The profile now comes from the binary:
  a release build is `production`, a debug build is `dev`, and an unrecognised
  value falls back to the compiled default rather than to "off".

- **A third profile, `desktop`.** The desktop app embeds the server on loopback
  for one person; holding it to the multi-tenant gates would demand an explicit
  CORS origin list and refuse to start. It keeps the gates that protect a
  shipped binary — artifact provenance, signing-key strength and persistence —
  and drops the ones that only matter when strangers can reach the port.

### Testing and CI

- **Two suites that ran nowhere are wired in.** `api/signal.test.mjs` — 41
  tests over the federation signaling relay, including the canonical signing
  string `annex-federation` must match byte for byte — was named by no
  workflow, script or doc, which is how the two implementations came to
  disagree about whether `rendezvous_tag` is part of that string. And
  `zk/scripts/test-proofs.js`, a gate `release-gates.md` describes in detail,
  ran in no workflow: CI's ZK step covered `verify-artifacts.js` alone.

- **The UI audit is green and order-independent.** Surfaces that post during
  capture write to a scratch channel instead of the fixture channel every other
  surface photographs; pictures clip to the element under test; retries are off
  for the audit project, because against shared server state a retry is a
  second write rather than a second sample. One genuine failure previously
  cascaded into 53. Two manifest guards keep it from returning.

- **CI uploads the evidence it was already producing.** Playwright writes the
  masked, clipped actual and a pixel diff to `client/e2e-results/` on every
  mismatch; the workflow uploaded only an unmasked full-page shot that cannot
  be compared to a baseline.

- `scripts/verify-production-rejects-dev-fixtures.sh` tests the gate rather
  than the tree, and immediately found two real holes: the release workflow
  gated one circuit of six, and nothing stopped the dev-ceremony bypass being
  set in it.

- `cargo-deny`, Dependabot across four ecosystems, and guards for ROADMAP
  self-consistency and version agreement.

### Documentation

- `RELEASE_READINESS.md` no longer instructs the reader to discount red CI
  checks; that paragraph described infrastructure that was fixed at run 749,
  and a real failure was sitting on `main` while it stood.
- `ROADMAP.md` agreed with itself. Five phases were marked PARTIAL in the
  summary and COMPLETE in their own sections.
- `deployment.md` said message content is stored in plaintext and that signing
  keys live in the database. Both were false, and the second means an operator
  following it backed up the database and not the key.
- `AGENTS.md` promised three `VrpCapabilitySharingContract` fields the protocol
  does not have.

### Known limitations

Carried forward deliberately, and tracked in `ROADMAP.md`:

- The trusted setup is single-operator, not multi-party.
- VRP semantic alignment scores with a pinned static embedding table
  (`potion-base-2M`, 7.5 MB), not a learned contextual model. A production
  server refuses to start without it; dev and desktop fall back to the concept
  lexicon and carry `lexicon-v1` as their scorer fingerprint so a peer can see
  which instrument produced a verdict.
- RTX cross-server delivery is single-hop.
- Whisper STT needs an operator-supplied model; none is bundled.
  `scripts/setup-stt.sh` installs a digest-pinned one.
- The server speaks HTTP only and expects a TLS-terminating reverse proxy.
- Desktop installers are not OS-code-signed; SmartScreen and Gatekeeper will
  warn. Verify downloads against the published SHA-256 checksums.

[Unreleased]: https://github.com/seismicgear/annex/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/seismicgear/annex/releases/tag/v0.1.0
