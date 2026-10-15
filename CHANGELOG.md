# Changelog

Notable changes follow [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Both v0.1.0 and its separate v0.1.1 packaging correction were published on
October 15, 2026. Their tags and installer bytes remain unchanged.

## [Unreleased]

- Add source/asset-bound macOS public-invite qualification using actual DMGs,
  XCTest, and independent destination observations; add immutable build recovery
  and draft-upload receipts. These harness improvements do not replace installers.
- Restore the owner's original website design while preserving accurate content,
  downloads, installation help, licensing explanations, and functional fixes.
- Integrate compatible CI action updates and record explicitly deferred dependency
  proposals during packaging qualification. Tighten the upload test's URL-origin
  assertion and pin the native acceptance cache action.

## [0.1.1] — 2026-10-15

Published from `c4513be6f301e49511e219c9c969d5eefd7e109b` as a separate macOS
packaging correction. The original v0.1.0 installers remain unchanged.

- Seal the complete macOS app with explicit ad-hoc signing, sign embedded Whisper
  before recording resource provenance, and strictly verify the bundle and each
  embedded Mach-O file before release upload.
- Include Annex Community License 2.0, updated onboarding/architecture/privacy
  documentation, CONTRIBUTING.md, and the reviewed dependency changes from #321.
- Preserve incomplete roadmap requirements, including independent cryptographic
  review, a multi-contributor ceremony, broader network/load qualification, and
  installed updater acceptance.
- Apple Silicon and Intel published installers completed the native public-invite,
  explicit join, independent message, and warm-delivery checks in
  [run 38814938771](https://github.com/seismicgear/annex/actions/runs/38814938771).
  Developer ID/notarization and ordinary browser-download/Gatekeeper acceptance
  remain unavailable or unqualified; these are separate from ad-hoc integrity.

## [0.1.0] — 2026-10-15

### Identity and security

- Challenge-bound zero-knowledge membership, proof renewal against the current
  published root, device linking by encrypted transfer code or scannable QR,
  recovery, session revocation, and explicit connection takeover.
- Production builds require the six pinned ZK artifact manifests and their
  ceremony provenance. Verification follows each proving-key chain, derives
  its verification key, and verifies the committed drand beacon. The existing
  ceremony is explicitly single-contributor, not a multi-party ceremony.
- Authenticated encryption at rest refuses malformed marked ciphertext
  instead of treating corruption as plaintext. Current permissions and token
  epochs are enforced on active connections as well as new requests.
- Device certificates, signed channel membership manifests and compare-and-swap
  epoch changes bind encrypted-channel access to admitted devices.
  Historical sealed keys remain distinct from permission to obtain new keys
  or fetch current protected attachments.

### Channels, messages and federation

- Text, voice, hybrid, broadcast and agent channel actions include server-side
  publication permissions, edit history, deletion and retention handling.
  Message edits/deletions and their federation outbox entries commit together.
- Federation supports authenticated HTTP and WebRTC data-channel delivery,
  durable retries, receipt-based deduplication and operator recovery. Ordered
  channel authority events support discovery, explicit fingerprint pinning,
  membership changes, history adoption and resumable reconciliation.
- RTX supports signed multi-hop forwarding with a bounded hop count, loop
  refusal and authenticated origin/hop metadata. Negotiated domain, size,
  retention, redaction and transfer-scope restrictions are enforced; full
  knowledge transfer requires explicit policy permission. Accepted handshake
  references bind authors to the actual negotiated agreement.
- Ordinary and encrypted attachments have staged ownership, explicit abort
  reservations, transactional message associations and durable cleanup.
  Federated transfers verify bounded chunks, content hashes and current
  read permits; encrypted names and keys remain within encrypted descriptors.
- Read-time expiry checks refuse unswept expired content. Reconciliation
  preserves immutable retained attachment metadata and associations and rolls
  back on conflicts rather than silently replacing local history.
- Browser recovery handles storage-open failures, reconnects, server
  switching, interrupted uploads and bounded typed proof renewal. Channel
  access, federation outbox and agent voice controls expose actionable errors.

### Voice, video and speech

- Human and agent audio use room-scoped media lifecycle and caption handling.
  Camera and screen-share tracks remain distinct; leave/rejoin, voice
  permission removal and session revocation close the affected media.
- SRTP sequence continuity, publisher/receiver teardown, source attribution,
  TTS pacing and sample-rate conversion were repaired. Piper speed control
  uses the actual measured speech path; parameter extremes remain subject to
  separate provider qualification.
- Desktop packages prepare and verify pinned Piper voices, the Whisper
  `base.en` model, a real native `whisper-cli`, and the alignment model.
  Speech readiness checks the actual executable/model contract. Server
  operators can configure their speech providers independently.
- Optional Bark and system providers retain their own executable/model
  prerequisites; configuring a profile does not prove that provider works on
  every target platform.

### Desktop invitations and release packaging

- Public invitations carry a validated HTTPS origin and an unchanged bearer
  code through `monolithannex.com` into `annex://invite`. Native cold/warm
  delivery retains pending invitations until acknowledged by the application;
  accepting an invitation remains an explicit user action.
- Public-share validation rejects incompatible origin shapes and private
  literal addresses before a share link is persisted. Direct native HTTPS
  links retain their separately defined private-server behavior.
- Isolated native profiles separate server data, browser storage, credentials
  and single-instance identity. Installed deep-link registration is tested
  separately from that isolated mode. Uninstall defaults to retaining data;
  explicit removal checks paths, reparse points and deletion outcomes.
- Release preflight checks version/CHANGELOG agreement, production proof
  provenance, resource safety and updater-key availability. Windows x64,
  Linux x64, macOS Apple Silicon and macOS Intel builds are all required.
- Release assembly verifies the actual Tauri 2 updater payload signatures
  and their authenticated comments, gives installers architecture-specific
  names, and creates source-bound download metadata. Final checksums include
  the installers, updater payloads, metadata, SBOMs and proof provenance.
- The workflow also builds a server tarball and container image and runs the
  real startup validator. A workflow definition is not evidence that an image
  or installer has been published; the actual release job must complete.

### Qualification and remaining boundaries

- All four native release targets, artifact/signature checks, server archive,
  and container jobs passed. Release-source CI and CodeQL passed.
- Final-source browser verification recorded 13 functional tests, 6 group-call
  tests, and 426 UI checks across 105 surfaces with zero findings.
- The published Linux DEB completed an app-created public invitation through the
  OS handler, explicit join, and independently received messaging. The published
  AppImage launched from a persistent path with spaces and received OS invitations.
- Full Windows/macOS native invite-to-message acceptance and installed updater
  acceptance remain open. Earlier Windows Sandbox acceptance was blocked by
  WebView2 startup. Optional speech/provider and broad network/load/soak
  qualification remain incomplete.
- Windows has no Authenticode signing; macOS is ad-hoc signed without Developer ID
  or notarization. Updater signatures and checksums are separate.
- Public hosting requires reachable HTTPS. No working automatic public tunnel
  is claimed. The trusted setup remains single-contributor with a beacon;
  alignment thresholds are calibrated on a small corpus.
- Exact source-bound receipts: [Release readiness](docs/RELEASE_READINESS.md).

[Unreleased]: https://github.com/seismicgear/annex/commits/main
[0.1.0]: https://github.com/seismicgear/annex/releases/tag/v0.1.0
