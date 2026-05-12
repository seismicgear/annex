#!/usr/bin/env bash
#
# smoke-server.sh — Production smoke test for the Annex server identity flow.
#
# Boots `annex-server` with `enforce_zk_proofs=true` against a temporary
# data directory, drives it through a real registration → Merkle path →
# Groth16 proof → verify-membership → authenticated channel-create round
# trip, and shuts the server down cleanly. The actual API flow lives in
# `scripts/smoke-server-flow.mjs` so the .sh and .ps1 stay thin.
#
# Required artifacts (none of these are dev-only):
#   • zk/keys/membership_vkey.json
#   • zk/build/membership_js/membership.wasm
#   • zk/keys/membership_final.zkey
#
# Usage:
#   bash scripts/smoke-server.sh
#
# Environment knobs:
#   ANNEX_SMOKE_PORT  — port to bind (default: 7321; must be free).
#   ANNEX_SMOKE_HOST  — bind address (default: 127.0.0.1).
#
# Exit code is non-zero on any failure; the trap always reaps the server
# and removes the temp data directory.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

PORT="${ANNEX_SMOKE_PORT:-7321}"
HOST="${ANNEX_SMOKE_HOST:-127.0.0.1}"
URL="http://${HOST}:${PORT}"
SERVER_PID=""
DATA_DIR=""
LOG_FILE=""

cleanup() {
    local exit_code=$?

    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "[smoke-server] stopping server (PID $SERVER_PID)"
        kill "$SERVER_PID" 2>/dev/null || true
        for _ in $(seq 1 10); do
            kill -0 "$SERVER_PID" 2>/dev/null || break
            sleep 0.5
        done
        kill -9 "$SERVER_PID" 2>/dev/null || true
    fi

    if [ -n "$LOG_FILE" ] && [ "$exit_code" -ne 0 ] && [ -f "$LOG_FILE" ]; then
        echo "[smoke-server] ── server log (last 80 lines) ──"
        tail -80 "$LOG_FILE" || true
        echo "[smoke-server] ────────────────────────────────"
    fi

    if [ -n "$DATA_DIR" ] && [ -d "$DATA_DIR" ]; then
        rm -rf "$DATA_DIR"
    fi

    exit "$exit_code"
}
trap cleanup EXIT INT TERM

# ── 1. Verify ZK artifacts ────────────────────────────────────────────────

echo "[smoke-server] verifying ZK artifacts"
required_artifacts=(
    "zk/keys/membership_vkey.json"
    "zk/build/membership_js/membership.wasm"
    "zk/keys/membership_final.zkey"
)
for artifact in "${required_artifacts[@]}"; do
    if [ ! -s "$artifact" ]; then
        echo "[smoke-server] ERROR: missing ZK artifact: $artifact" >&2
        echo "[smoke-server] run: (cd zk && npm ci && node scripts/build-circuits.js && node scripts/setup-groth16.js)" >&2
        exit 1
    fi
done

if [ ! -d "zk/node_modules/snarkjs" ] || [ ! -d "zk/node_modules/circomlibjs" ]; then
    echo "[smoke-server] ERROR: zk/node_modules missing snarkjs/circomlibjs (run: npm --prefix zk ci)" >&2
    exit 1
fi

# ── 2. Build the server (debug; relies on Cargo cache for repeat runs) ────
# We build then exec the binary directly rather than `cargo run` so the
# captured PID is the server itself — `cargo run` keeps the spawned
# binary alive even when the cargo wrapper is killed, which leaks state
# across smoke invocations.

echo "[smoke-server] building annex-server"
cargo build -p annex-server --quiet
SERVER_BINARY="$REPO_ROOT/target/debug/annex-server"
if [ ! -x "$SERVER_BINARY" ]; then
    echo "[smoke-server] ERROR: built binary missing or not executable: $SERVER_BINARY" >&2
    exit 1
fi

# ── 3. Allocate temp data dir ─────────────────────────────────────────────

DATA_DIR="$(mktemp -d -t annex-smoke-XXXXXX)"
LOG_FILE="$DATA_DIR/server.log"
echo "[smoke-server] data dir: $DATA_DIR"

# ── 4. Start server with enforce_zk_proofs=true ───────────────────────────

echo "[smoke-server] starting server on $URL (enforce_zk_proofs=true)"
ANNEX_HOST="$HOST" \
ANNEX_PORT="$PORT" \
ANNEX_DB_PATH="$DATA_DIR/annex.db" \
ANNEX_ENFORCE_ZK_PROOFS=true \
ANNEX_OPEN_BROWSER=false \
RUST_LOG="${RUST_LOG:-warn,annex_server=info}" \
    "$SERVER_BINARY" /dev/null > "$LOG_FILE" 2>&1 &
SERVER_PID=$!

# ── 5. Wait for /health ──────────────────────────────────────────────────

echo "[smoke-server] waiting for /health"
for attempt in $(seq 1 90); do
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
        echo "[smoke-server] ERROR: server exited before becoming ready" >&2
        exit 1
    fi
    if curl -fsS "$URL/health" >/dev/null 2>&1; then
        echo "[smoke-server] /health up after ${attempt}s"
        break
    fi
    sleep 1
    if [ "$attempt" -eq 90 ]; then
        echo "[smoke-server] ERROR: /health never became ready" >&2
        exit 1
    fi
done

# ── 6. Drive the identity + verify-membership + channel-create flow ──────

node "$REPO_ROOT/scripts/smoke-server-flow.mjs" --url "$URL"

# ── 7. Stop cleanly (the trap also reaps if anything above fails) ────────

echo "[smoke-server] flow complete; shutting down server"
kill "$SERVER_PID" 2>/dev/null || true
for _ in $(seq 1 10); do
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.5
done

if kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "[smoke-server] ERROR: server did not stop after SIGTERM" >&2
    kill -9 "$SERVER_PID" 2>/dev/null || true
    exit 1
fi

SERVER_PID=""
echo "[smoke-server] OK"
