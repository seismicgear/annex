#!/usr/bin/env bash
# e2e-server.sh — Start/stop the Annex server for E2E testing.
#
# Usage:
#   bash scripts/e2e-server.sh start   # Build client, start server (background)
#   bash scripts/e2e-server.sh stop    # Stop the server
#   bash scripts/e2e-server.sh restart # Stop + start
set -euo pipefail

# BASH_SOURCE, not $0: under `source` $0 is the CALLER's path, so this would
# resolve the repo root relative to whoever sourced it and cd somewhere else
# entirely.
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PID_FILE="/tmp/annex-e2e-server.pid"
LOG_FILE="/tmp/annex-e2e-server.log"
DB_DIR_FILE="/tmp/annex-e2e-server.dbdir"
PORT=3000

# The pids LISTENING on $PORT — not everything with a socket on it.
#
# `lsof -ti :$PORT` also matches every CLIENT connected to the port: a browser
# context with an open connection to the server is a second pid in that list.
# See scripts/tests/e2e-server-port.test.sh, which fails without the
# restriction.
listener_pids() {
    lsof -ti "tcp:$PORT" -sTCP:LISTEN 2>/dev/null || true
}

# Refuse to take the port out from under a UI audit run.
#
# `scripts/ui-audit.sh` holds an exclusive flock for the length of a run, but
# that lock only ever stopped a second AUDIT. Anything else that reaches this
# script — `e2e-all.sh`, a bare `e2e-server.sh start`, a mistyped argument —
# stopped the audit's server mid-capture, and neither side reports a
# collision: the audit just fails every remaining surface in a few hundred
# milliseconds each.
#
# Proven by doing it, while writing this: `e2e-all.sh bogus` starts a server
# before it validates its argument, and killed a run at surface 52 of 415.
#
# The audit's own call is let through by ANNEX_AUDIT_CHILD, which ui-audit.sh
# exports; everything else is refused while the lock is held.
audit_run_in_progress() {
    [ -n "${ANNEX_AUDIT_CHILD:-}" ] && return 1
    command -v flock >/dev/null 2>&1 || return 1
    local lock="${TMPDIR:-/tmp}/annex-ui-audit.lock"
    [ -e "$lock" ] || return 1
    # Taking the lock in a subshell releases it on exit, so this only ever
    # asks the question.
    if ( flock -n 9 || exit 1 ) 9>>"$lock" 2>/dev/null; then
        return 1
    fi
    return 0
}

refuse_during_audit() {
    if audit_run_in_progress; then
        echo "[e2e] ERROR: a UI audit run holds ${TMPDIR:-/tmp}/annex-ui-audit.lock"
        echo "[e2e] refusing to touch port $PORT — it would kill that run mid-capture."
        echo "[e2e] wait for it to finish (record AND verify), or set ANNEX_AUDIT_CHILD=1"
        echo "[e2e] if you are certain the lock is stale."
        return 1
    fi
    return 0
}

start_server() {
    refuse_during_audit || return 1

    # Every port decision below goes through lsof. Without it listener_pids
    # returns nothing, which reads as "the port is free" — the one answer that
    # is never safe to guess.
    if ! command -v lsof >/dev/null 2>&1; then
        echo "[e2e] ERROR: lsof is required to identify the process holding port $PORT"
        return 1
    fi

    # Stop any existing server
    stop_server 2>/dev/null || true

    # And refuse to build for ten minutes if that did not work. Everything
    # below assumes this process ends up owning the port.
    local held
    held=$(listener_pids)
    if [ -n "$held" ]; then
        echo "[e2e] ERROR: port $PORT is still held by PID $(echo $held | tr '\n' ' ') after stop"
        return 1
    fi

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
    # A public URL is set for a third reason of the same kind. Left unset, the
    # server slug comes from `generate_server_slug()` — random per boot — and
    # it is rendered in the header chip. Masking hides the text but not its
    # width, so the mask box changed size every run and shifted its neighbour's
    # box with it; captures diffed against themselves roughly one run in three.
    # With a URL set the slug is `sha256(url)[..6]`: same value, every time.
    # HTTPS specifically, because that is the shape a real deployment has and
    # it exercises the invite path rather than the "no public URL" branch.
    #
    # WebRTC is configured so the voice stage can capture an actual call. The
    # SFU is in-process (`crates/annex-voice`, webrtc-rs), so the URL is this
    # same server's WebSocket — the credentials only have to be non-empty for
    # `voice_status` to report configured. Left unset, every voice surface can
    # only ever be a pre-call state and the participant grid, media controls
    # and diagnostics are unreachable.
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
    ANNEX_PUBLIC_URL="${ANNEX_PUBLIC_URL:-https://annex.audit.test}" \
    ANNEX_WEBRTC_URL="${ANNEX_WEBRTC_URL:-ws://127.0.0.1:$PORT}" \
    ANNEX_WEBRTC_API_KEY="${ANNEX_WEBRTC_API_KEY:-audit}" \
    ANNEX_WEBRTC_API_SECRET="${ANNEX_WEBRTC_API_SECRET:-audit-secret}" \
    cargo run -p annex-server -- "$empty_config" > "$LOG_FILE" 2>&1 &

    local pid=$!
    echo "$pid" > "$PID_FILE"

    # Wait for ready — and for the server that is ready to be OURS.
    #
    # `curl /health` on its own cannot tell this server from a leftover on the
    # same port. When the port was already held, the survivor answered "ok" on
    # the first iteration and this printed "Server ready on port 3000 (PID N)"
    # one second after launch, while PID N was on its way to exiting with
    # AddrInUse. The run then drove a database this script never created,
    # which surfaces much later and much less legibly as "founder must be the
    # earliest registrant". `cargo run` execs the binary on Unix, so $pid is
    # the server itself and the listening pid is a direct identity check.
    for i in $(seq 1 90); do
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "[e2e] ERROR: server process $pid exited during startup"
            tail -20 "$LOG_FILE"
            return 1
        fi
        if [ "$(listener_pids)" = "$pid" ] &&
           curl -s "http://127.0.0.1:$PORT/health" 2>/dev/null | grep -q '"ok"'; then
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

    # Also kill any stray server still listening on the E2E port — a run
    # whose pidfile was lost, or a server started by hand.
    #
    # This used to read `lsof -ti :$PORT` and pass the result to `kill` as one
    # word. With any client connected the value is multi-line, and `kill`
    # rejects such a string outright ("arguments must be process or job IDs")
    # without signalling anything; the message above still printed. The stray
    # then survived, `start_server` could not bind, and its readiness curl was
    # answered by the survivor — so the run proceeded against a database it
    # had not created, which surfaces much later as "founder must be the
    # earliest registrant".
    local stray p
    stray=$(listener_pids)
    if [ -n "$stray" ]; then
        echo "[e2e] Killing stray process on port $PORT (PID $(echo $stray | tr '\n' ' '))"
        for p in $stray; do
            kill "$p" 2>/dev/null || true
        done
        for _ in $(seq 1 10); do
            if [ -z "$(listener_pids)" ]; then
                break
            fi
            sleep 1
        done
        stray=$(listener_pids)
        if [ -n "$stray" ]; then
            for p in $stray; do
                kill -9 "$p" 2>/dev/null || true
            done
            sleep 1
        fi
    fi

    if [ -n "$(listener_pids)" ]; then
        echo "[e2e] ERROR: port $PORT is still held after stop"
        return 1
    fi
    return 0
}

# Only dispatch when executed. Sourcing exposes the functions above without
# running anything, which is how scripts/tests/e2e-server-port.test.sh drives
# the port handling against a scratch port.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    case "${1:-}" in
        start)   start_server ;;
        stop)    refuse_during_audit && stop_server ;;
        restart) refuse_during_audit && stop_server && start_server ;;
        *)
            echo "Usage: $0 {start|stop|restart}"
            exit 1
            ;;
    esac
fi
