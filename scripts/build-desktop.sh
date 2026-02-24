#!/usr/bin/env bash
# build-desktop.sh — Builds ZK artifacts and the client for Tauri desktop packaging.
#
# This script is invoked by `tauri.conf.json`'s `beforeBuildCommand`.
# It ensures all ZK circuit artifacts (verification key, WASM prover,
# proving key) are generated and placed where both the server and client
# can find them at runtime.
#
# Usage:
#   ./scripts/build-desktop.sh          # full build (ZK + client)
#   SKIP_ZK=1 ./scripts/build-desktop.sh  # skip ZK, client build only

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

ZK_DIR="$ROOT_DIR/zk"
CLIENT_DIR="$ROOT_DIR/client"
ZK_KEYS_DIR="$ZK_DIR/keys"
ZK_BUILD_DIR="$ZK_DIR/build"
CLIENT_PUBLIC_ZK="$CLIENT_DIR/public/zk"

# ── Step 1: Build ZK circuits (if not already built or SKIP_ZK is set) ──

if [ "${SKIP_ZK:-}" = "1" ]; then
    echo "[build-desktop] Skipping ZK build (SKIP_ZK=1)"
elif [ -f "$ZK_KEYS_DIR/membership_vkey.json" ] \
  && [ -f "$ZK_KEYS_DIR/membership_final.zkey" ] \
  && [ -f "$ZK_BUILD_DIR/membership_js/membership.wasm" ]; then
    echo "[build-desktop] ZK artifacts already exist — skipping ZK build"
else
    echo "[build-desktop] Building ZK circuits..."

    cd "$ZK_DIR"

    # Install ZK dependencies if needed
    if [ ! -d "node_modules" ]; then
        echo "[build-desktop]   Installing ZK dependencies..."
        npm ci
    fi

    echo "[build-desktop]   Compiling circuits..."
    node scripts/build-circuits.js

    echo "[build-desktop]   Running Groth16 trusted setup..."
    node scripts/setup-groth16.js

    echo "[build-desktop] ZK build complete."
    cd "$ROOT_DIR"
fi

# ── Step 2: Copy ZK client artifacts to client/public/zk/ ──

echo "[build-desktop] Copying ZK artifacts to client/public/zk/..."
mkdir -p "$CLIENT_PUBLIC_ZK"

if [ -f "$ZK_BUILD_DIR/membership_js/membership.wasm" ]; then
    cp "$ZK_BUILD_DIR/membership_js/membership.wasm" "$CLIENT_PUBLIC_ZK/membership.wasm"
    echo "[build-desktop]   Copied membership.wasm"
else
    echo "[build-desktop]   WARNING: membership.wasm not found — client proof generation will fail"
fi

if [ -f "$ZK_KEYS_DIR/membership_final.zkey" ]; then
    cp "$ZK_KEYS_DIR/membership_final.zkey" "$CLIENT_PUBLIC_ZK/membership_final.zkey"
    echo "[build-desktop]   Copied membership_final.zkey"
else
    echo "[build-desktop]   WARNING: membership_final.zkey not found — client proof generation will fail"
fi

# ── Step 3: Build the client ──

echo "[build-desktop] Building client..."
cd "$CLIENT_DIR"

if [ ! -d "node_modules" ]; then
    echo "[build-desktop]   Installing client dependencies..."
    npm ci
fi

npm run build
echo "[build-desktop] Client build complete."

echo "[build-desktop] All done."
