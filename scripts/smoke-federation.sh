#!/usr/bin/env bash
#
# smoke-federation.sh — LIVE multi-server federation proof.
#
# Boots a real annex-server ("Server B") on a temp DB + port, then runs
# scripts/smoke-federation-relay.mjs which plays a remote peer ("Server A"):
# it seeds B with the post-handshake/attestation state, signs a real
# FederatedMessageEnvelope with A's Ed25519 key, and relays it to B over HTTP.
# B verifies the signature + agreement + membership and persists/broadcasts it.
#
# Exit non-zero on any failure; the trap reaps the server + temp dir.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

PORT="${ANNEX_FED_PORT:-7333}"
HOST=127.0.0.1
URL="http://${HOST}:${PORT}"
SERVER_PID=""
DATA_DIR=""

cleanup() {
  local code=$?
  if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    for _ in $(seq 1 10); do kill -0 "$SERVER_PID" 2>/dev/null || break; sleep 0.3; done
    kill -9 "$SERVER_PID" 2>/dev/null || true
  fi
  [ -n "$DATA_DIR" ] && [ -d "$DATA_DIR" ] && rm -rf "$DATA_DIR"
  exit "$code"
}
trap cleanup EXIT INT TERM

echo "[fed-smoke] building annex-server"
cargo build -p annex-server --quiet
BIN="$REPO_ROOT/target/debug/annex-server"

DATA_DIR="$(mktemp -d -t annex-fed-XXXXXX)"
DB="$DATA_DIR/annex.db"
echo "[fed-smoke] data dir: $DATA_DIR"

echo "[fed-smoke] starting Server B on $URL"
ANNEX_HOST="$HOST" ANNEX_PORT="$PORT" ANNEX_DB_PATH="$DB" \
  ANNEX_OPEN_BROWSER=false RUST_LOG="warn,annex_server=info" \
  "$BIN" /dev/null > "$DATA_DIR/server.log" 2>&1 &
SERVER_PID=$!

echo "[fed-smoke] waiting for /health"
for attempt in $(seq 1 90); do
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "[fed-smoke] server exited early"; cat "$DATA_DIR/server.log"; exit 1; }
  if curl -fsS "$URL/health" >/dev/null 2>&1; then echo "[fed-smoke] up after ${attempt}s"; break; fi
  sleep 1
  [ "$attempt" -eq 90 ] && { echo "[fed-smoke] /health never ready"; exit 1; }
done

node "$REPO_ROOT/scripts/smoke-federation-relay.mjs" --db "$DB" --url "$URL"

echo "[fed-smoke] OK"
