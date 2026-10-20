# Dependency security review — updated 2026-10-20

The release audit continues to deny unreviewed advisories, yanked packages,
unknown registries, unknown Git sources, and disallowed licenses. A successful
build alone is not security acceptance. This review distinguishes repaired
security defects from upstream maintenance notices; maintenance is not claimed
as a vulnerability fix.

## October 20 lockfile audit

A fresh full-lockfile audit found `quinn-proto 0.11.13` retained behind optional
HTTP/QUIC features even though the resolved-feature release gate passed.
The lockfile now selects compatible `0.11.18`, covering RUSTSEC-2026-0037,
RUSTSEC-2026-0185, RUSTSEC-2026-0338, RUSTSEC-2026-0339,
RUSTSEC-2026-0340 and RUSTSEC-2026-0342. The refreshed audit reports zero
vulnerabilities. The four existing maintenance notices below remain separately
reviewed; no new ignore was added.

The shared blocking security gate now runs pinned `cargo-audit 0.22.2` against
the complete `Cargo.lock` after the existing cargo-deny, provenance, maintenance
and behavioral backport checks. Optional and other-target dependencies are
therefore checked even when absent from the current enabled feature graph.

## JavaScript advisory exposure — October 20

Both current npm lockfiles report 15 low-severity affected package entries and
zero moderate, high or critical entries. These are one advisory propagated
through `circomlibjs 0.1.7 → ethers 5 → elliptic 6.6.1`, not 15 independent
cryptographic defects. [GHSA-848j-6mx2-7j84](https://github.com/advisories/GHSA-848j-6mx2-7j84)
concerns ECDSA signing and lists no patched version. npm's proposed
`circomlibjs 0.0.8` downgrade is not an accepted proof-system repair.

The October 20 production Vite build's rendered-module inventory contained no
`elliptic`, ethers `signing-key`, `transactions`, `hdnode` or `wallet` code.
Some ethers byte/encoding utilities remain. Annex uses circomlibjs's Poseidon
implementation for identity commitments; its application signatures use the
existing Ed25519/WebCrypto implementations, not elliptic ECDSA. Real proof
round-trips and browser identity/encryption journeys remain required.

The low advisory is retained in the installed Node dependency graph. This
exposure assessment does not mean npm audit is clean and must be repeated if
crypto imports, bundling or dependencies change. Review expires 2027-01-13.
The existing blocking production npm audit remains enabled without advisory
ignores.

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

These are upstream maintenance-status notices. None of the three retained advisories
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

webpki-roots 1.0.6 contains certificate-root data under
[CDLA-Permissive-2.0](https://spdx.org/licenses/CDLA-Permissive-2.0.html).
Section 2.1 permits sharing with the agreement text. The exact crate LICENSE
text is included in `../THIRD_PARTY_NOTICES.md`, which desktop bundles and the
server container ship. The license exception is limited to webpki-roots 1.0.6;
The v0.1.0 binaries retain their original bundled license. The public-readiness pass adopts Annex Community License 2.0 on main with an additional KI-owned-code grant for v0.1.0; see [licensing](licensing.md).

## Post-release WebRTC proposal (October 15, 2026)

The proposed webrtc 0.17.2 migration failed the existing real-SRTP publication
rejoin test in CI. It is deferred; see [dependency decisions](dependency-upgrades.md).
Both the published tag and this integration retain webrtc 0.11 and its reviewed
webrtc-dtls 0.10.0 / bincode 1.3.3 graph. All four maintenance dispositions keep
the exact reviewed versions, checksums, features, parents, and expiry. No
additional advisory exception is introduced and no acceptance test is weakened.
