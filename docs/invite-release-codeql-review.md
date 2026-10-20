# Invite release CodeQL review

## October 20 commercial-candidate review

PR #339 retains both scanner languages and the security-and-quality query set.
The initial new Windows/Linux acceptance checkouts produced two high cache
poisoning findings and two unsafe-checkout findings. Those workflows now read
only inert version JSON at the authenticated build's commit; they never check
out or execute code selected by `subject_commit`. The harness comes from the
workflow's own checkout. Automatic package-manager caching is explicitly off.
Artifact run, source, successful preflight/native job, immutable archive digest,
package identity and installed-executable digest checks remain mandatory.

CI, package proof and release now default to `contents: read`. Only the existing
publication jobs retain explicit write scopes. Mutable third-party rust-cache
and Docker login references are replaced by verified upstream commit pins.
The unused acceptance-script imports/assignment reported in #59, #60 and #62
are removed. These changes do not change product UI or authorization policy.

The remaining concrete data-flow findings were reviewed against the current
source; the scanner remains enabled and no path/query exclusion was added:

| Alerts | Exact disposition and supporting boundary |
| --- | --- |
| #93 | False positive: choosing a v1 slug or v2 tag selects the appropriate mandatory drain verifier, never whether authentication occurs. Both production branches require authorized key ownership, a fresh bound signature and replay rejection. The production profile comes from process configuration, not the request. All 60 signaling tests passed, including wrong-recipient, missing/invalid signature, downgrade and replay rejection. |
| #128, #129 | False positives: the Promise reference comparison deliberately clears only the key-generation operation still owned by this invocation. The operation is awaited immediately before `finally`; comparing its resolved values would break the concurrency guard. |
| #58 | False positive: `proof.worker.ts` is a dedicated module Worker created by `lib/zk.ts` using a local URL. Its creator/worker channel is not a Window cross-origin message listener. The proof/capability authority stays with the existing verifier. |
| #132, #133 | Intentional public downloads: beacon and desktop-resource URLs are read from the checked-in public manifests/configuration. These GET requests contain no private file body. Desktop resource size/digest and beacon chain/key/signature checks remain enforced. |
| #127 | Intentional evidence write: the draft uploader saves the returned public release ID and asset digests to the fixed `release-draft-upload.json` path. Response data cannot choose that path or execute code. |
| #106–#109 | Intentional isolated acceptance proxy: it binds only `127.0.0.1:3480`, forwards only to `127.0.0.1:3080` and validates the fixed test hostname. TLS key material configures TLS locally; it is not forwarded in HTTP bodies. Fixed evidence basenames record response status and disposable test invitations, not auth/session request bodies. Private fixture observations are excluded from public deliverables. |

These are source-specific dispositions, not a general security certification.
Reassess if the production-profile boundary, verifier selection, worker creation,
fixed paths, local proxy binding, or manifest trust changes.

## Historical October 15 review

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
