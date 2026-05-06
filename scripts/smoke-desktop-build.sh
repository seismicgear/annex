#!/usr/bin/env bash
#
# smoke-desktop-build.sh — Production smoke test for the Linux desktop
# build. Pinned to the release-critical surface only:
#
#   1. Verify ZK artifacts (real ones — no `|| true`, no dev fallback).
#   2. Build the client bundle (tsc + vite).
#   3. Build the desktop binary (`cargo build -p annex-desktop --release`).
#   4. Confirm the resources Tauri will bundle exist on disk.
#
# This is intentionally narrower than the full Tauri bundle build that
# `release-desktop.yml` runs — that lane validates `.deb` / `.AppImage`
# packaging end-to-end and is too slow for every smoke pass. Use this
# script when you want a fast pass/fail on the link + resource wiring.
#
# Usage:
#   bash scripts/smoke-desktop-build.sh
#
# Environment knobs:
#   SKIP_CLIENT_BUILD=1  — skip the client build step (dev-only, e.g. when
#                          iterating on the Rust side and the existing
#                          client/dist is known to be current). NOT for
#                          release / CI.
#
# Exit code is non-zero on any failure; no `|| true` around release-critical
# commands.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

echo "[smoke-desktop] repo root: $REPO_ROOT"

# ── 1. Verify ZK artifacts ────────────────────────────────────────────────
#
# tauri.conf.json declares `../../zk/keys/membership_vkey.json` as a
# bundle resource. The client also needs `membership.wasm` and
# `membership_final.zkey` to generate proofs at runtime — these are
# copied into client/public/zk/ by build-desktop.js. All three must
# exist as non-empty files before we attempt to build.

echo "[smoke-desktop] verifying ZK artifacts"
required_zk=(
    "zk/keys/membership_vkey.json"
    "zk/build/membership_js/membership.wasm"
    "zk/keys/membership_final.zkey"
)
for artifact in "${required_zk[@]}"; do
    if [ ! -s "$artifact" ]; then
        echo "[smoke-desktop] ERROR: missing ZK artifact: $artifact" >&2
        echo "[smoke-desktop] run: (cd zk && npm ci && node scripts/build-circuits.js && node scripts/setup-groth16.js)" >&2
        exit 1
    fi
done

# Ensure the vkey at least parses as JSON so we don't ship a corrupt
# resource into the bundle.
if ! node -e "JSON.parse(require('fs').readFileSync('zk/keys/membership_vkey.json','utf8'))" >/dev/null 2>&1; then
    echo "[smoke-desktop] ERROR: zk/keys/membership_vkey.json is not parseable JSON" >&2
    exit 1
fi

# ── 2. Build the client + populate Tauri bundle inputs ───────────────────
#
# We run `node scripts/build-desktop.js` rather than `npm run build`
# directly because it is the same script Tauri's `beforeBuildCommand`
# invokes. It (a) copies `membership.wasm` and `membership_final.zkey`
# into `client/public/zk/` so the proof worker can serve them, then
# (b) runs the same `tsc -b && vite build` we'd otherwise invoke. Using
# the real entry point keeps the smoke aligned with the bundle path.
#
# `SKIP_PIPER=1` keeps the smoke fast — Piper TTS assets are validated
# separately by the release-desktop workflow's setup-piper step. They
# are not on the Rust build path that this smoke is gating.

if [ "${SKIP_CLIENT_BUILD:-}" = "1" ]; then
    echo "[smoke-desktop] DEV-ONLY: SKIP_CLIENT_BUILD=1 set, skipping client build"
    if [ ! -f "client/dist/index.html" ]; then
        echo "[smoke-desktop] ERROR: SKIP_CLIENT_BUILD=1 but client/dist/index.html is missing" >&2
        exit 1
    fi
else
    echo "[smoke-desktop] installing client deps"
    if [ ! -d "client/node_modules" ]; then
        npm --prefix client ci
    fi

    echo "[smoke-desktop] running build-desktop.js (ZK copy + client build)"
    SKIP_PIPER=1 node scripts/build-desktop.js
fi

# ── 3. Build the desktop binary ──────────────────────────────────────────

echo "[smoke-desktop] cargo build -p annex-desktop --release"
cargo build -p annex-desktop --release

# ── 4. Confirm packaged resources exist ──────────────────────────────────

echo "[smoke-desktop] verifying packaged resources"

declare -a release_outputs=(
    "client/dist/index.html"
    "client/public/zk/membership.wasm"
    "client/public/zk/membership_final.zkey"
    "zk/keys/membership_vkey.json"
)

# `npm run build` runs `tsc -b && vite build`; vite emits the index plus
# at least one JS chunk under client/dist/assets/. Confirm the chunk
# directory exists rather than guessing the hashed filename.
if [ "${SKIP_CLIENT_BUILD:-}" != "1" ]; then
    if [ ! -d "client/dist/assets" ]; then
        echo "[smoke-desktop] ERROR: client/dist/assets missing after build" >&2
        exit 1
    fi
fi

for resource in "${release_outputs[@]}"; do
    if [ ! -s "$resource" ]; then
        echo "[smoke-desktop] ERROR: expected resource missing: $resource" >&2
        exit 1
    fi
done

# Linux release binary path. `target/release/annex-desktop` exists once
# the cargo build above succeeds.
release_binary="target/release/annex-desktop"
if [ ! -s "$release_binary" ]; then
    echo "[smoke-desktop] ERROR: release binary missing: $release_binary" >&2
    exit 1
fi

# Sanity check: the binary should at least be marked executable.
if [ ! -x "$release_binary" ]; then
    echo "[smoke-desktop] ERROR: $release_binary is not executable" >&2
    exit 1
fi

binary_size=$(wc -c < "$release_binary")
echo "[smoke-desktop] release binary: $release_binary ($binary_size bytes)"

echo "[smoke-desktop] OK"
