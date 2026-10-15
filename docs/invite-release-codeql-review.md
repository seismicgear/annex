# Invite release CodeQL review

Reviewed on 2026-10-15 against PR #319, analysis run 38701869467 and
source 36f9cd737f87603a739d287289a1beef102f3ac9. The scanner remains enabled
with its existing security-and-quality queries and policy. This review is
limited to the following individual alerts; it does not dismiss other findings.

## Repairs

- #103: replace separate path inspection/read with a single opened handle,
  descriptor-bound inspection and read, no-follow where supported, and file
  identity checks. Actual path-substitution regressions cover this boundary.
- #101 and #102: pin the newly introduced cache actions to upstream commit
  `6323deb102c322ba6fcbdcafc7e3dddab59af2b6` (v2.9.2).
- #99: remove the always-true test cleanup condition; storage restoration still
  runs after preparation fails or succeeds.
- #61: remove the unused `capabilityArtifactsPresent` helper.

## Exact false-positive dispositions

The live scripts are operator-invoked acceptance programs. Their explicit
session/fixture inputs select disposable test servers and evidence directories;
they are not public request handlers. Their real authenticated network requests
and retained observations are necessary parts of the tests.

| Alert | Boundary and reason |
| --- | --- |
| #92 | `prepare-desktop-assets.mjs`: the download URL comes from the checked-in asset manifest. HTTPS and pinned size/SHA-256 are verified before downloaded bytes become resources. No private file is uploaded. |
| #90 | `live/protocol.mjs`: the explicit test-session URL selects the server for the operator-requested acceptance operation. No remote request supplies the destination. |
| #91 | `live/protocol.mjs`: the request body contains the intended test operation and cryptographic protocol fields. Signing secrets stay local. |
| #86 | `live/manifest.mjs`: the destination is the explicit isolated-session input, not an attacker-controlled server request. |
| #87 | `live/manifest.mjs`: sending the disposable identity's real session credential to its selected server is intentional authentication. |
| #88 | `live/manifest.mjs`: the body contains the deliberate manifest protocol operation, not the raw private fixture. |
| #85 | `live/federation-ui.mjs`: the outage health probe requires two captured processes with verified executable hashes and URLs restricted to `127.0.0.1`. |
| #89 | `live/encrypted-authority.mjs`: uploads freshly generated disposable ciphertext through the selected peer's actual authenticated attachment endpoint. |
| #84 | `live/channel-authority.mjs`: uploads generated test attachment bytes to the selected disposable peer after genuine credential renewal. |
| #96 | `live/image-privacy.mjs`: persists cleaned image bytes after exact type/digest, metadata-removal and database assertions. The basename comes from the checked-in fixture manifest; the response cannot choose the destination. |
| #95 | `live/device-certificates.mjs`: persists public certificate/verifier JSON at the fixed evidence filename `device-certificate-public-fixture.json`. Network data cannot select the path. |
| #94 | `live/device-upgrade.mjs`: persists disposable migration state as JSON at the explicitly selected private fixture file. The returned message ID is data, not a path or executable input. |
| #97 | `membership-verification.worker.ts`: this is a dedicated Worker created with a local module URL by `membership-verification.ts`, not a Window cross-origin message listener. Messages use its private creator/worker channel. |
| #98 | `encrypted-uploads.ts`: Promise object identity deliberately checks ownership before deleting an in-flight operation. Awaiting the compared values would remove that protection when an older operation finishes after replacement. |

Reassess these dispositions if a public entry point, remotely selected output
path, private-key serialization, worker communication model, or operation
ownership model changes. No scanner directories or query families are excluded.
