#!/usr/bin/env bash
# e2e-server.sh — Start/stop the Annex server for E2E testing.
#
# Usage:
#   bash scripts/e2e-server.sh start   # Build client, start server (background)
#   bash scripts/e2e-server.sh stop    # Stop the server
#   bash scripts/e2e-server.sh restart # Stop + start
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

PID_FILE="/tmp/annex-e2e-server.pid"
LOG_FILE="/tmp/annex-e2e-server.log"
DB_DIR_FILE="/tmp/annex-e2e-server.dbdir"
PORT=3000

start_server() {
    # Stop any existing server
    stop_server 2>/dev/null || true

    # Prepare ZK artifacts for the client
    echo "[e2e] Preparing ZK artifacts..."
    node scripts/prepare-zk-dev.js 2>&1 | tail -3

    # Build the client (skip tsc, just vite build)
    echo "[e2e] Building frontend..."
    (cd client && npx vite build 2>&1 | tail -3)

    # Build the server binary (release for speed, but debug is fine too)
    echo "[e2e] Building server..."
    cargo build -p annex-server 2>&1 | tail -3

    # Use a fresh DB for each E2E run
    local db_dir
    db_dir=$(mktemp -d /tmp/annex-e2e-XXXXXX)
    echo "$db_dir" > "$DB_DIR_FILE"

    echo "[e2e] Starting server (db: $db_dir, log: $LOG_FILE)..."

    # The server PARSES its config-path argument as TOML, so an empty real
    # file is used rather than /dev/null: if anything ever replaces /dev/null
    # with a regular file, passing it here kills the server with a baffling
    # TOML error instead of starting it. Empty means "defaults plus the env
    # overrides below".
    local empty_config="$db_dir/empty-config.toml"
    : > "$empty_config"

    # Rate limits default to 10/10/60 requests per minute, which a browser
    # suite exceeds trivially: the UI audit alone drives ~100 page loads back
    # to back, and every public route keys its bucket by IP, so all of them
    # share one. Left at defaults the suite captures screenshots of
    # "Rate limit exceeded" instead of the UI. Raised here for the harness
    # only — the shipped defaults are unchanged.
    #
    # Presence pruning is disabled (0) for the same class of reason. It fires
    # on a 60s timer against a 300s inactivity threshold and appends a
    # NODE_PRUNED row to the event log, so whether the audit's event-log
    # screenshots contain that row depends on how far into the run the timer
    # landed — the surface diffed against itself between runs. The audit's
    # identities are idle by construction (their sessions are restored from
    # storage state, not driven), so pruning them tests nothing here.
    ANNEX_CLIENT_DIR=client/dist \
    ANNEX_OPEN_BROWSER=false \
    ANNEX_DB_PATH="$db_dir/annex.db" \
    ANNEX_HOST=127.0.0.1 \
    ANNEX_PORT=$PORT \
    ANNEX_RATE_LIMIT_DEFAULT="${ANNEX_RATE_LIMIT_DEFAULT:-100000}" \
    ANNEX_RATE_LIMIT_REGISTRATION="${ANNEX_RATE_LIMIT_REGISTRATION:-100000}" \
    ANNEX_RATE_LIMIT_VERIFICATION="${ANNEX_RATE_LIMIT_VERIFICATION:-100000}" \
    ANNEX_INACTIVITY_THRESHOLD_SECONDS="${ANNEX_INACTIVITY_THRESHOLD_SECONDS:-0}" \
    cargo run -p annex-server -- "$empty_config" > "$LOG_FILE" 2>&1 &

    local pid=$!
    echo "$pid" > "$PID_FILE"

    # Wait for ready
    for i in $(seq 1 90); do
        if curl -s "http://127.0.0.1:$PORT/health" 2>/dev/null | grep -q '"ok"'; then
            echo "[e2e] Server ready on port $PORT (PID $pid) after ${i}s"
            return 0
        fi
        sleep 1
    done

    echo "[e2e] ERROR: Server failed to start within 90s"
    cat "$LOG_FILE"
    return 1
}

stop_server() {
    if [ -f "$PID_FILE" ]; then
        local pid
        pid=$(cat "$PID_FILE")
        if kill -0 "$pid" 2>/dev/null; then
            echo "[e2e] Stopping server (PID $pid)..."
            kill "$pid" 2>/dev/null || true
            # Wait for graceful shutdown
            for i in $(seq 1 10); do
                kill -0 "$pid" 2>/dev/null || break
                sleep 1
            done
            kill -9 "$pid" 2>/dev/null || true
        fi
        rm -f "$PID_FILE"
    fi

    # Clean up temp database directory
    if [ -f "$DB_DIR_FILE" ]; then
        local db_dir
        db_dir=$(cat "$DB_DIR_FILE")
        if [ -d "$db_dir" ]; then
            rm -rf "$db_dir"
        fi
        rm -f "$DB_DIR_FILE"
    fi

    # Also kill any stray server processes on the E2E port
    local stray_pid
    stray_pid=$(lsof -ti :$PORT 2>/dev/null || true)
    if [ -n "$stray_pid" ]; then
        echo "[e2e] Killing stray process on port $PORT (PID $stray_pid)"
        kill "$stray_pid" 2>/dev/null || true
    fi
}

case "${1:-}" in
    start)   start_server ;;
    stop)    stop_server ;;
    restart) stop_server; start_server ;;
    *)
        echo "Usage: $0 {start|stop|restart}"
        exit 1
        ;;
esac
