# Reviewed dependency backports

These crates retain their published versions and licenses. The original
crates.io archives are preserved in `upstream/`; their SHA-256 values match the
original Cargo registry checksums. This is a compatibility backport, not a
claim that upstream published a newer version.

`scripts/dependency-backports.json` records each archive URL/hash, every changed
file, and exact before/after replacements. `scripts/verify-dependency-backports.mjs`
reconstructs the intended source from those archives and rejects any missing,
extra, or changed vendored file. It also verifies the regression harness bytes.

Cargo-deny does not match registry advisories to path-sourced crates. No GLib
advisory ignore is configured: the independent patch verification is mandatory.
CI passes full locked Cargo metadata to the gate, proving that Cargo resolves these exact local
crates, refuses registry substitution or an additional vulnerable GLib version,
and verifies Ark resolves a repaired tracing subscriber. A broad advisory
ignore, renamed crate, or manufactured version number is not used.

## GLib 0.18.5

The only source change is the official two-line fix for RUSTSEC-2024-0429:
`VariantStrIter::impl_get` uses a mutable C out-pointer and passes `&mut p`.
The optimized regression exercises `next`, `next_back`, `nth`, `nth_back`,
`last`, empty arrays, Unicode, and mixed forward/backward iteration.

Upstream fix: https://github.com/gtk-rs/gtk-rs-core/commit/05dff0ee696f9bcd8617cd48c4b812d046d440cb

Advisory: https://rustsec.org/advisories/RUSTSEC-2024-0429.html

## ark-relations 0.5.1

The normalized Cargo manifest requires tracing-subscriber 0.3.23 with its
registry feature, retaining disabled default features. The no-op Layer callback
is renamed from `new_span` to the current API's `on_new_span`. No constraint,
field, proof, serialization, or arithmetic implementation is changed. The
original `Cargo.toml.orig` remains the untouched published source metadata;
Cargo builds from the normalized, explicitly patched `Cargo.toml`.

This is a narrow compatibility adaptation using the API also adopted upstream,
not the full upstream R1CS-to-GR1CS redesign. Regressions verify nested constraint
span capture, source locations, rendered traces, and constraint filtering with
the current subscriber.

Upstream API reference: https://github.com/arkworks-rs/snark/blob/d4c771c2a0f04954c2ff9f05916221dc5844266a/relations/src/gr1cs/trace.rs

Advisory: https://rustsec.org/advisories/RUSTSEC-2025-0055.html

## tauri-plugin-single-instance 2.5.4

The normalized Linux manifest uses exact zbus 6.0.0. zbus integrated the
previously separate name and wire-format crates; those retired crates must no
longer appear anywhere in the locked application dependency graph. The plugin's
Linux adapter uses zbus's supported `block_on` entry point around its current
asynchronous connection API. Ownership, argument forwarding, cleanup, and all
other platforms are unchanged. Source verification checks every replacement
against the original crates.io archive; Cargo
resolution verification also rejects an older bus or a retired wire/name crate.

Upstream announcement: https://github.com/z-galaxy/zbus/issues/2043

The optimized regression opens a real isolated session bus. It verifies that
an owner continues receiving invite arguments after setup returns, another
connection cannot replace it, and cleanup permits a new owner to acquire the
name. This component check does not substitute for installed-app acceptance.

## Verification

The updater 2.14.0 archive is also preserved byte for byte. Its only patch is
the Windows ZIP dependency requirement, from withdrawn major 4 to exact 9.0.3.
Updater verification, installation and rollback code are unchanged. Cargo
metadata must resolve only that reviewed reader, and the preflight exercises
its actual extraction API with installer bytes and a path-traversal fixture.
This is a dependency compatibility patch, not a claimed upstream updater release.
No yanked-package exception or audit bypass is configured.

```sh
node scripts/verify-dependency-backports.mjs --source-only
node --test scripts/tests/dependency-backports.test.mjs
cargo metadata --locked --all-features --format-version 1 > dependency-metadata.json
node scripts/verify-dependency-backports.mjs --metadata dependency-metadata.json
cargo test --locked --release --manifest-path scripts/security-regressions/Cargo.toml
dbus-run-session -- cargo test --locked --release --manifest-path scripts/single-instance-regressions/Cargo.toml
```

The last command runs real GLib and Ark code on Linux with the GLib development
libraries installed. Source verification alone does not claim those optimized
runtime regressions passed. The dependency audit follows both gates in CI.
