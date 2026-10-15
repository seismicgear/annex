# v0.1.0 public-readiness verification — October 15, 2026

Scope: the application/docs repository, `seismicgear/monolith-annex`, and
<https://monolithannex.com>. The owner's request authorizes publication and
merging the open PRs. Original v0.1.0 artifacts remain unchanged; post-tag source
updates are explicitly unreleased.

## Implementation review

Claims were checked against server routes/startup, desktop configuration and
resource preparation, client identity/invite flows, device certificates,
membership manifests/epochs, private attachments, federation authority/outbox,
relay startup, and speech-provider readiness. The [release ledger](RELEASE_READINESS.md)
separates the signed build and final-source CI from actual native acceptance.

Documentation now distinguishes implemented mechanisms, observed acceptance,
known limitations, and the preserved architectural vision. The roadmap's
unsupported external-review/performance/soak completion claims are reopened.
The website explains joining, local hosting, remote HTTPS, OS trust prompts,
external agent runtimes, and per-channel encryption.

## Checks performed during this pass

- Client: **928 tests / 106 files passed**, plus lint and production TypeScript/Vite
  build after the compatible client patch updates.
- Proofs: all pinned production artifacts verified and installed; real proof
  round trips and tamper/domain-separation checks: **57 passed, zero failed**.
- Rust: strict Clippy across all non-desktop workspace targets passed. Native
  desktop and remaining integration gates are recorded by the pull request's CI;
  a local Linux environment does not qualify Windows or macOS.
- Roadmap: **16 consistency checks passed**; phase summaries and detailed status
  agree, and unfinished criteria remain present.
- Supply chain: exact GLib/Ark backports verified against Cargo metadata; the
  three remaining maintenance dispositions retain reviewed versions, sources,
  checksums, features, parents, and expiration. The WebRTC migration eliminates
  bincode, so its obsolete exception was removed.
- Published server archive: downloaded, SHA-256 matched the release, extracted
  with original resources, and `--check` passed. The path performs migrations,
  creates an initial channel/signing key, and exits. Missing web/speech payloads
  are now documented accurately.
- Website: invite parser/handler, signed signaling, pinned release catalog, CSP
  nonces, HEAD/404/method behavior tests passed. Browser checks passed at **320,
  390, 768, and 1440 px**, with no horizontal overflow or axe WCAG A/AA findings;
  keyboard skip link, FAQ, destination preservation, and help were exercised.
  Desktop and phone screenshots were visually reviewed.
- **All six installer URLs and SHA256SUMS returned HTTP 200** after redirects.
  Link availability is not a claim that each native installer was executed here.

The website has repeatable browser checks in its CI and supports rerunning them
against the deployed URL. Merge/build/deployment receipts belong to GitHub and
the existing Vercel integration; final production acceptance must inspect the
actual deployed pages, not merely the preview or commit.

## Dependency PR decisions

[Dependency integration](dependency-upgrades.md) records each open PR's result.
TypeScript 7 and Arkworks 0.6 are explicitly deferred at compatible versions;
merging their histories does not claim those proposed versions were adopted.
No security gate or published ceremony was weakened. Other compatible upgrades
and repository handoff instructions are incorporated.

## License

The owner requested commercial use/modification with a separate reseller/paid
hosting agreement and retained IP ownership. Community License 2.0 implements
that policy. The restriction means it remains source available, not OSI open
source. No copyright assignment or rights to unrelated KI projects are granted.
Contributor ownership and the need for separate broader commercial rights are
explicit. Published binaries receive an additional grant only for rights KI can
license; third-party terms remain intact.

## Remaining work and owner

Maintainer-owned release qualification remains: complete native Windows/Mac
invite/join/message journeys, installed updater acceptance, all speech-provider
combinations, broader network/scale/soak trials, independent cryptographic review,
and a broader alignment corpus. Observable acceptance is a source/artifact-bound
receipt for each path, including failures and skips. The public website does not
claim these are complete.

## Congress handoff

**Not synchronized to KI AI Congress / Constellation.** The connector's orientation
and bounded Annex graph search returned `SOURCE_READ_TIMEOUT`, SQLSTATE `57014`,
phase `nodes`. Its health check reported degraded source reads/worker queue.
The exact repository resolution also returned no acquired repository identity;
that is not evidence that the project does not exist. Repository-document search
found the marketing record but did not establish an existing writable project
thread. No competing tracker or out-of-scope thread was created. This source-linked
repository record preserves the handoff; retry synchronization after the service
and project/thread identity are available. No synchronization success is claimed.
