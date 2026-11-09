# Verified Linux packaging tools

Before a Linux x86_64 Tauri bundle, run this from the repository root with a
job-owned absolute cache path:

```sh
export XDG_CACHE_HOME="$RUNNER_TEMP/annex-linux-bundler"
node scripts/prepare-linux-bundler.mjs
# Keep the same XDG_CACHE_HOME for the subsequent Tauri build.
```

Both CI and the desktop release workflow perform this step. The committed
`scripts/linux-bundler-tools.json` pins the five files consumed by Tauri CLI
2.11.2's AppImage bundler. Binary downloads use the official GitHub asset API
with binary Accept; shell plugins use exact commits of Tauri's upstream forks.
Every file must match its byte count and SHA-256 before entering Tauri's cache.
Transient transport errors receive bounded retries; integrity errors fail closed.

Tauri changes bytes 8–10 of linuxdeploy during bundling. Verified original
downloads therefore live separately from the mutable `tauri/` cache. Every
preparation restores fresh verified executable copies, including on repeat
builds. Links and unsafe cache entries are refused; unrelated files are retained.
These are build tools, not additional shipped application resources.

When updating the CLI, review the upstream bundler's cache paths, tool names,
source revisions, and mutations before changing pins. Obtain hashes from the
official downloaded bytes and compare published upstream digests where present.
Run the Linux bundler and desktop download regression tests and the actual Linux bundle and
installed-package audits; helper tests alone do not qualify an installer.
