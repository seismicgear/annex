# Release installer tool

This isolated package installs the official Tauri 2.11.2 CLI and its native
platform package. It does not change Annex's application dependencies. The
committed lockfile records the exact version, registry URL and SHA-512 integrity
of every supported platform package.

Release jobs run `npm ci --ignore-scripts` and `npm audit signatures` here before
invoking `node tools/tauri-cli/node_modules/@tauri-apps/cli/tauri.js`. The signature
audit verifies the registry signatures and package provenance. CLI build flags,
production proof gates and updater signing credentials are unchanged.

The npm CLI is published by the upstream Tauri repository from its Rust CLI:
https://github.com/tauri-apps/tauri/tree/dev/tooling/cli/node

To update this build tool, change the exact dependency version, regenerate the
lockfile, audit its signatures and prove all platform installers again.
