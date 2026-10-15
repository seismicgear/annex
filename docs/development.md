# Build and verify Annex

For normal use, install [v0.1.1](installation.md). `main` includes later work
and is not identical to the published binaries. To reproduce a release, check
out its exact tag (`v0.1.0` or `v0.1.1`); do not regenerate its ceremony or replace
its assets.

## Tools

- Rust **1.96.0**, selected by `rust-toolchain.toml`; include rustfmt and Clippy.
- Node.js **22.12 or newer** and npm; CI uses Node 22.
- A C/C++ compiler, CMake, pkg-config, and libclang for native speech/DSP bindings.
- On Linux, desktop builds additionally need GTK 3, WebKitGTK 4.1, Soup 3,
  librsvg, AppIndicator, and PipeWire development packages; see the exact
  installation list in `.github/workflows/ci.yml`.
- Desktop builds require the native platform toolchain and Tauri CLI 2. The
  release workflow pins its CLI and prepares platform-specific resources.

## Server and browser development

From the repository root:

```bash
git clone https://github.com/seismicgear/annex.git
cd annex
npm ci --prefix zk
node zk/scripts/verify-artifacts.js --all --profile production
node zk/scripts/install-ceremony.js
node scripts/prepare-zk-dev.js
bash scripts/setup-embedding-model.sh
npm ci --prefix client
npm run build --prefix client
cargo run -p annex-server -- --check
cargo run -p annex-server
```

The server listens on port 3000 by default and serves `client/dist`. In another
terminal, `npm run dev --prefix client` starts Vite for frontend development.
Use the URL printed by Vite. The development prep helper copies the **pinned**
provers into the client; it is deliberately refused under a production/release
build profile. Do not generate random replacement keys to fix a build.

The alignment-model download is pinned and verified. It is needed by production
startup and calibration tests. A missing model can make some local tests skip;
that is not a calibration pass. See [deployment](deployment.md) for production
CORS, HTTPS, persistence, and signing-key configuration. The root default Compose
file is for development; production uses `docker-compose.prod.yml`.

## Checks

Run the checks for the surfaces you changed. The full server CI lane is:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings
cargo test --workspace --exclude annex-desktop
npm run lint --prefix client
npm test --prefix client
npm run build --prefix client
npm test --prefix zk
node zk/scripts/test-proofs.js
bash scripts/tests/roadmap-consistency.test.sh
```

Server tests for shared admission also use Redis; CI supplies the service and
its environment. Native desktop checks belong in the native platform lane.
Do not report excluded/skipped targets as passed. Browser tests and the UI audit
use real server/proof setup; see [client/README.md](../client/README.md) and
[the audit guide](ui-audit/README.md). Snapshot updates require deliberate visual
review and a separate commit.

## Desktop builds

Before Tauri resolves bundle resources, prepare the actual pinned native
resources on the matching OS/architecture:

```bash
node scripts/prepare-desktop-assets.mjs
```

This downloads/builds speech executables and verifies voices, the Whisper model,
and the alignment model. Placeholder directories do not prove speech readiness.
Use `scripts/build-desktop.js` through Tauri's configured frontend build hook;
see [RELEASING.md](RELEASING.md) and the release workflow for signing and packaging.
Cross-platform installer acceptance requires the actual OS, not just a successful
cross-build or a browser mock.

## Dependency changes

Keep lockfiles committed and use the existing supply-chain gates. Do not suppress
peer conflicts, disable certificate verification, regenerate cryptographic
artifacts, or relax audit checks to make a version bump green. Current
compatibility decisions are recorded in [dependency upgrades](dependency-upgrades.md).
