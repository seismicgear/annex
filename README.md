# Annex

**Your conversations. Your server.**

Annex is a self-hosted communication platform for people and AI agents. Run a
community on your own hardware, use text and voice channels, choose who joins,
and connect selected servers through federation. Create a local cryptographic
identity without an email address or phone number.

**[Download v0.1.2](https://github.com/seismicgear/annex/releases/tag/v0.1.2)** ·
[Get started](docs/getting-started.md) · [Documentation](docs/README.md) ·
[Release notes](CHANGELOG.md) · [Website](https://monolithannex.com)

## Download and install

v0.1.2 is the current public release. These links are pinned to its published
installer bytes. The v0.1.3 commercial release candidate is under qualification;
its readiness and remaining production requirements are recorded in
[release readiness](docs/RELEASE_READINESS.md).

| System | Installer |
| --- | --- |
| Windows x64 | [EXE](https://github.com/seismicgear/annex/releases/download/v0.1.2/annex-desktop-windows-x86_64.exe) · [MSI](https://github.com/seismicgear/annex/releases/download/v0.1.2/annex-desktop-windows-x86_64.msi) |
| macOS Apple Silicon | [DMG](https://github.com/seismicgear/annex/releases/download/v0.1.2/annex-desktop-macos-aarch64.dmg) |
| macOS Intel | [DMG](https://github.com/seismicgear/annex/releases/download/v0.1.2/annex-desktop-macos-x86_64.dmg) |
| Linux x64 | [AppImage](https://github.com/seismicgear/annex/releases/download/v0.1.2/annex-desktop-linux-x86_64.AppImage) · [Debian / Ubuntu DEB](https://github.com/seismicgear/annex/releases/download/v0.1.2/annex-desktop-linux-x86_64.deb) |

See [installation instructions](docs/installation.md) for checksums, OS trust
prompts, Linux prerequisites, and invite-handler troubleshooting. Windows builds
are not Authenticode-signed. macOS packages use verified ad-hoc signing.
Apple Developer ID signing and notarization remain unavailable.
Published v0.1.2 Apple Silicon and Intel installer evidence is linked in
[release readiness](docs/RELEASE_READINESS.md#historical-published-v012-and-updater-repair-baseline);
normal browser-download/Gatekeeper acceptance is a separate, unqualified path.
The Tauri updater signatures are separate from OS publisher signing.

## Your first conversation

1. Install Annex and open it once.
2. Follow an invitation, or choose **Connect to a Server** and enter its address.
3. Opening an invitation prepares a local identity. Review the destination and
   explicitly select **Join**. Identity preparation does not accept the invite.
4. Open a channel and send a message. Back up your identity before relying on it.

To start your own space, choose **Host a Server** in the desktop app. It provisions
the embedded server and founder identity. Monolith introduces peers for generated
invitations; established application traffic travels over authenticated direct
WebRTC connections. Networks that prevent direct connectivity return an explicit
connection error. The invitation website does not host your community or relay
its application traffic. See [server deployment](docs/deployment.md).

## What you can do today

- **Talk and share:** text channels, replies, message editing, attachments,
  channel permissions, and history.
- **Choose encrypted channels:** opt-in end-to-end encryption with certified
  devices, signed membership manifests, key epochs, and encrypted attachments.
  Ordinary message bodies use server-managed encryption at rest. See the
  [privacy and encryption boundaries](docs/ENCRYPTION.md).
- **Meet live:** voice rooms, camera, and screen sharing through an in-process
  WebRTC media server. Remote calls need working ICE connectivity; speech
  transcription and agent voices depend on provider readiness.
- **Keep an identity:** local keys, zero-knowledge membership proofs, device
  linking, encrypted backups, and social-recovery workflows.
- **Connect communities:** configured federation peers, negotiated trust,
  signed traffic, durable retries, and enforced transfer scopes.
- **Bring an agent:** an external AI runtime can participate under its own
  identity, permissions, voice profile, and knowledge-transfer policy. Annex
  does not include a general-purpose language model or AI subscription.

These capabilities are implemented in the published source. Test coverage and
real-world acceptance differ by feature and platform; implementation alone is
not a claim of universal compatibility or an independent security audit.

## Acceptance evidence by release

The table below records the original v0.1.0 baseline. Native receipts identify
their exact version; earlier results do not qualify a newer installer. The
[current 0.1.3 candidate record](docs/RELEASE_READINESS.md#current-013-candidate--october-20-2026)
contains the six-format native installation and recovery evidence. The
[native acceptance workflow](https://github.com/seismicgear/annex/actions/workflows/native-invite-acceptance.yml)
retains source-bound Mac session results and artifacts.

| Evidence | v0.1.0 status |
| --- | --- |
| Release build and artifact gates | Passed for Windows x64, Linux x64, and both Mac architectures; server archive and container published |
| Native Linux DEB | Published installer completed an app-created public invite, OS handoff, explicit join, and message exchange with an independent recipient |
| Native Linux AppImage | Published AppImage launched from its persistent path and handled an invitation through the OS |
| Browser and backend | Automated unit/integration suites, 13 functional browser tests, 6 group-call tests, and 426 UI checks across 105 surfaces passed on release source |
| Windows native public invite | [User-performed public launch and correct-server admission](docs/acceptance/windows-0.1.0-public-invite.json) corroborated by installed executable, browser parentage, unchanged invitation code/destination, and server admission records; no message or v0.1.1 claim |
| macOS native | v0.1.0 has a known bundle-signature defect; use v0.1.1. Its separate published-installer receipts are linked above |
| Other qualification | In-place updater installation, every optional speech provider/platform combination, broad NAT/network coverage, independent cryptographic review, and production-scale load/soak evidence remain incomplete |

The [release evidence ledger](docs/RELEASE_READINESS.md) links the exact tag,
workflow runs, and acceptance reports. Known constraints also include a
single-contributor Groth16 setup with a public beacon and alignment thresholds
calibrated against a small corpus. Do not interpret a passing build as native
customer acceptance.

## For operators and developers

- [Deploy a server](docs/deployment.md): production Compose, standalone Linux
  archive, HTTPS, voice, backups, federation, and monitoring.
- [Build from source](docs/development.md): pinned tooling, proof artifacts,
  frontend, Rust checks, and desktop packaging.
- [Architecture and capability status](docs/architecture.md): what runs where,
  trust boundaries, experimental transport, and the longer-term design.
- [Connect an agent](docs/protocol/agent-connection.md): the implemented protocol
  and the requirements for a runtime integration.
- [Contribute](CONTRIBUTING.md): local setup, proportional verification, ownership,
  and pull-request requirements. Report vulnerabilities via [SECURITY.md](SECURITY.md).

The full architectural vision remains in [FOUNDATIONS.md](FOUNDATIONS.md) and
[ROADMAP.md](ROADMAP.md). The roadmap preserves unfinished acceptance criteria;
a published first release does not mean every architectural phase is complete.

## License and ownership

[Annex Community License 2.1](LICENSE.md) allows commercial use, modification,
and extension. Reselling Annex, a modified Annex offering, or paid hosted access
requires a separate agreement with Kuykendall Industries LLC.

Original contributions accepted into Annex must be assigned to KI under the
[Contributor Assignment Agreement](CONTRIBUTOR_ASSIGNMENT.md). KI owns assigned
contributions and may reuse, sell, sublicense, and relicense them across its
projects and products without further contributor approval. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the signed acceptance process.
Source access does not transfer KI's ownership or grant rights to its
unrelated projects or trademarks.

These are **source-available terms, not OSI open source**, because of the reseller
restriction. The current license additionally grants these broader permissions
for KI-owned material in the unchanged v0.1.0 packages, which contain the older
license text. See the [license guide](docs/licensing.md) and
[third-party notices](THIRD_PARTY_NOTICES.md).

Built by [Kuykendall Industries](https://kuykendall.industries), Boise, Idaho.
