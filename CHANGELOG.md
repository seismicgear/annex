# Changelog

All notable changes to Annex are recorded here, in the format of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

There was no changelog before `0.1.0`. `release_v0.1.md` is the historical
record of what was prepared in February 2026 and never tagged; it carries a
supersession banner and is kept for provenance rather than maintained.

## [Unreleased]

Nothing yet.

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
- VRP semantic alignment is a curated concept lexicon, not a learned model, and
  it decides Aligned/Partial/Conflict for every handshake.
- The agent capability contract is enforced at channel join, not at action time.
- RTX cross-server delivery is single-hop.
- Whisper STT needs an operator-supplied model; none is bundled.
- The server speaks HTTP only and expects a TLS-terminating reverse proxy.
- Desktop installers are not OS-code-signed; SmartScreen and Gatekeeper will
  warn. Verify downloads against the published SHA-256 checksums.

[Unreleased]: https://github.com/seismicgear/annex/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/seismicgear/annex/releases/tag/v0.1.0
