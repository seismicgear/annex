# Changelog

Notable changes follow [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`release_v0.1.md` preserves the historical February 2026 preparation notes.
Version 0.1.0 has not previously been published; the subsequent fixes belong
to this first release candidate, rather than a separate later release.

## [Unreleased]

The first release candidate is consolidated under 0.1.0 below.

## [0.1.0] — release candidate

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

This section describes the candidate source. It does **not** declare every
implemented repair qualified through the final installed application.

- Source-bound component suites and genuine browser sessions cover many
  identity, encryption, attachment, media and federation paths. Their receipts
  identify the candidate used; earlier passes are not silently transferred to
  later source revisions.
- The complete UI/native matrix on the final release source remains open.
  In particular, the installed OS-handler invitation journey and actual join
  of the intended server are pending until recorded against the installer.
  Public landing-page routing alone does not establish installed delivery.
- Cross-platform installer execution, updater installation, and optional
  speech-provider/parameter combinations require their own platform evidence.
  A build, signature check or bundled resource inventory does not replace it.
- Installers are not Windows Authenticode signed or Apple Developer ID signed
  and notarized. Tauri updater signatures and SHA-256 verification are separate
  from operating-system trust; platform warnings remain possible.
- The server's HTTP listener requires real TLS termination for public HTTPS.
  Automatic public traffic hosting is not established by the invitation
  website, and no working public tunnel service is claimed by this release.

[Unreleased]: https://github.com/seismicgear/annex/commits/main
[0.1.0]: https://github.com/seismicgear/annex/releases/tag/v0.1.0
