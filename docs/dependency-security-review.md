# Dependency security review — 2026-10-15

The release audit continues to deny unreviewed advisories, yanked packages,
unknown registries, unknown Git sources, and disallowed licenses. A successful
build alone is not security acceptance. This review distinguishes repaired
security defects from upstream maintenance notices; maintenance is not claimed
as a vulnerability fix.

## Release repair addendum — 2026-11-08

The managed Array release repair updates the active workspace and standalone
DTLS regression lockfiles. These changes address the following RustSec findings
without adding an advisory ignore or changing an existing maintenance disposition:

| Advisory | Repaired dependency or parent path |
| --- | --- |
| [RUSTSEC-2026-0385](https://rustsec.org/advisories/RUSTSEC-2026-0385.html) | ctutils 0.4.8 supplies the upstream constant-time mask and comparison repairs. |
| [RUSTSEC-2026-0382](https://rustsec.org/advisories/RUSTSEC-2026-0382.html) | rustls-webpki 0.103.16 rejects the distrusted certificate public keys. |
| [RUSTSEC-2026-0381](https://rustsec.org/advisories/RUSTSEC-2026-0381.html) | webpki-roots 1.0.10 removes the affected trust anchor. |
| [RUSTSEC-2026-0383](https://rustsec.org/advisories/RUSTSEC-2026-0383.html) | webpki-root-certs 1.0.10 repairs the additional root-certificate path reached through rustls-platform-verifier in the all-features graph. |
| [RUSTSEC-2026-0441](https://rustsec.org/advisories/RUSTSEC-2026-0441.html) | Hyper 1.9.0 removes its pin-utils dependency. The exact webrtc-util 0.9.0 source is retained with nix default features disabled and its required net feature enabled, removing the unused AIO path to pin-utils. |
| [RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html) | DTLS uses p256/p384 0.14.0, elliptic-curve 0.14.3 and crypto-bigint 0.7.10. No affected crypto-bigint 0.5 arithmetic is copied or relabeled. |

The curve-parent migration also uses sec1 0.8.1 and the corresponding maintained
ECDH APIs. P-256, P-384 and X25519 remain supported, with the original wire
identifiers and uncompressed NIST public-key encoding. X25519 keeps its existing
rand_core 0.6 generator. P-256/P-384 use the new parents' fallible system-random
generation API; failure returns a DTLS error before a keypair is accepted.
Shared hybrid-array and typenum patch updates satisfy the new parents' bounds.

The existing mandatory `supported_group_regression` filter now also checks
mutual ECDH and independent agreement with ring for all three groups, expected
public/shared-secret lengths, invalid SEC1 points, and every private-key/group
mismatch. The existing ClientHello/GREASE and unsupported-group rejection tests
remain in that same gate. Source reconstruction records every adapted file and
the regression manifests/locks. Resolved-graph verification additionally requires
the Unix network features and rejects AIO or any remaining pin-utils package.

Both certificate-data packages have individual license exceptions limited to
exactly 1.0.10. Their unchanged CDLA-Permissive-2.0 text is bundled in
`THIRD_PARTY_NOTICES.md`. The original four maintenance dispositions below retain
their exact package, parent, feature and checksum scope and 2027-01-13 expiry.

The `Cargo.lock` files inside published vendor archives remain byte-for-byte
upstream inventory. They do not resolve the application or the standalone DTLS
regression harness; those use the two updated active lockfiles. Keeping archive
inventory intact preserves the existing source-custody proof.

Upstream repair references: [ctutils](https://github.com/RustCrypto/utils/pull/1569),
[webpki](https://github.com/rustls/webpki/pull/546),
[root certificates](https://github.com/rustls/webpki-roots/pull/133),
[Hyper](https://github.com/hyperium/hyper/pull/4023),
[nix feature definitions](https://docs.rs/crate/nix/0.26.4/features),
[crypto-bigint](https://github.com/RustCrypto/crypto-bigint/pull/1365).

Verification on November 8: locked all-features Cargo metadata resolves 785
packages; exact-source/resolved-path and all four maintenance-disposition checks
pass. Cargo-deny 0.18.6 reports `advisories ok, bans ok, licenses ok, sources ok`
against the official RustSec database at
`309392b18f6b17022f82c701f300433126901068`. The database was freshly fetched with
the Git CLI after the embedded transport failed, then the unchanged repository
policy was checked against that cache. The database contains no separate
webrtc-util advisory hidden by its path dependency.

All five mandatory DTLS supported-group tests pass, including the three new
cryptographic behavior tests. All 49 focused Node tests pass across source
custody, maintenance policy, managed release and desktop staging. Formatting
checks pass for the touched Rust files. An independent nix-only compile check
also passes; live interface enumeration is unavailable in this environment
because both the unchanged system `getifaddrs` call and its Rust wrapper return
EPERM.

These dependency checks do not establish signed, installed native-app acceptance.

## Concrete defects

Compatible lockfile updates repair the reported anyhow, crossbeam-epoch,
event-listener, h2, quick-xml, rand 0.8/0.9, rustls and rustls-webpki defects.
Spin 0.9.9 replaces the yanked 0.9.8. Tauri 2.12.4 and its official plugin family
remove the older rand 0.7 / kuchikiki chain and the unmaintained UNIC crates.
The JavaScript API is aligned to 2.12.4; the official pinned CLI remains 2.11.2,
whose source checks Rust/API and Rust/plugin-JavaScript major/minor agreement,
not equality between the CLI and framework version.

The GTK3-compatible GLib 0.18.5 mutability fix and the Ark 0.5.1 tracing adapter
are retained as exact upstream-source backports. See `../vendor/README.md`.
Versions are not relabeled. Source and resolved dependency provenance plus
optimized behavioral regression tests run before the audit in CI and release
preflight. Cargo-deny does not match registry advisories to path packages, so no
GLib advisory ignore is added. The separate exact-source and resolved-path gate is
mandatory: the unpatched registry source must never be accepted.

## Maintenance findings requiring individual disposition

These are upstream maintenance-status notices. None of the four retained advisories
identifies an exploitable code defect. Any disposition is limited to the exact
package version, direct parent versions, enabled features, and review period;
new advisories remain blocking. Review expires 2027-01-13.

| Advisory and package | Exact direct parents | Exposure and replacement assessment |
| --- | --- | --- |
| [RUSTSEC-2024-0388](https://rustsec.org/advisories/RUSTSEC-2024-0388.html), derivative 2.2.0 | ark-crypto-primitives 0.5.0 | Procedural derive macro executed at compilation; not a runtime parser. Enabled feature is `use_core`. Latest ark-crypto-primitives 0.6.0 also depends on derivative ^2. A maintained derive replacement requires upstream-equivalent generated trait implementations and proof-system regression tests. |
| [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436.html), paste 1.0.15 | ark-ff 0.5.0; tokenizers 0.23.2 | Procedural token-concatenation macro executed at compilation; no enabled features. Tokenizers 0.23.2 is the current stable release and retains paste. `pastey` is a maintained candidate, requiring parent manifest changes and macro expansion compatibility tests. An Ark family change also affects proof interoperability and cannot be presented as a trivial package substitution. |
| [RUSTSEC-2024-0370](https://rustsec.org/advisories/RUSTSEC-2024-0370.html), proc-macro-error 1.0.4 | glib-macros 0.18.5; gtk3-macros 0.18.2 | Build-time diagnostics used by the GTK procedural macros. Features: default, syn, syn-error. GTK3-compatible published parents still retain it. Upstream removal commits are [GLib c74a40a](https://github.com/gtk-rs/gtk-rs-core/commit/c74a40a16b23a1b9c8e67842d32d02bc187f551d) and [GTK 4578225](https://github.com/gtk-rs/gtk3-rs/commit/45782251fa35ecf8bfe759ffceb3db32659a0c97); applying these requires separate macro diagnostic and generated-code regression work. |

The remaining fxhash notice is removed through scraper 0.27.0 / selectors
0.38.0. The Annex link-preview parser uses public APIs preserved by this update;
its existing parser and handler tests remain required.

## Certificate-root data license

webpki-roots 1.0.10 and webpki-root-certs 1.0.10 contain certificate-root data under
[CDLA-Permissive-2.0](https://spdx.org/licenses/CDLA-Permissive-2.0.html).
Section 2.1 permits sharing with the agreement text. The exact crate LICENSE
text is included in `../THIRD_PARTY_NOTICES.md`, which desktop bundles and the
server container ship. Each license exception is limited to its exact 1.0.10 package;
The v0.1.0 binaries retain their original bundled license. The public-readiness pass adopts Annex Community License 2.0 on main with an additional KI-owned-code grant for v0.1.0; see [licensing](licensing.md).

## Post-release WebRTC proposal (October 15, 2026)

The proposed webrtc 0.17.2 migration failed the existing real-SRTP publication
rejoin test in CI. It is deferred; see [dependency decisions](dependency-upgrades.md).
Both the published tag and this integration retain webrtc 0.11 and its reviewed
webrtc-dtls 0.10.0 / bincode 1.3.3 graph. All four maintenance dispositions keep
the exact reviewed versions, checksums, features, parents, and expiry. No
additional advisory exception is introduced and no acceptance test is weakened.
