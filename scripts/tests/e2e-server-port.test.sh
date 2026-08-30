#!/usr/bin/env bash
# e2e-server-port.test.sh — the port handling in scripts/e2e-server.sh.
#
# Both cases here failed before the fix they pin, and both failed silently:
# the script reported "Killing stray process" and then killed nothing,
# because `lsof -ti :PORT` matches every CLIENT connected to the port as well
# as the listener, and `kill` given that multi-line string rejects it whole.
#
# Usage: bash scripts/tests/e2e-server-port.test.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

PASS=0
FAIL=0
ok()  { echo "  ok   — $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL — $1"; FAIL=$((FAIL + 1)); }

# `kill -0` is not liveness for our own background children: a killed child
# stays a zombie until the shell reaps it, and signalling a zombie succeeds.
# Read the state field of /proc/<pid>/stat instead.
proc_alive() {
    [ -d "/proc/$1" ] || return 1
    [ "$(awk '{print $3}' "/proc/$1/stat" 2>/dev/null)" != "Z" ]
}

# A port nothing else in this repo uses, so a real run on 3000 is untouched.
SCRATCH_PORT=39417

if ! command -v lsof >/dev/null 2>&1; then
    echo "FAIL — lsof is required: scripts/e2e-server.sh uses it to find the port owner"
    exit 1
fi

TMPDIR_TEST=$(mktemp -d /tmp/e2e-server-test-XXXXXX)

# Everything on the scratch port, listener and clients alike — the helpers are
# started inside command substitutions, so a pid list built by them would be
# lost with the subshell.
cleanup() {
    lsof -ti "tcp:$SCRATCH_PORT" 2>/dev/null | xargs -r kill -9 2>/dev/null || true
    rm -rf "$TMPDIR_TEST" || true
    return 0
}
trap cleanup EXIT

LISTEN_PY='import socket,sys,time
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(8); s.settimeout(0.5)
held=[]; deadline=time.time()+120
while time.time() < deadline:
    try: held.append(s.accept())
    except Exception: pass'

CLIENT_PY='import socket,sys,time
c=socket.socket(); c.connect(("127.0.0.1", int(sys.argv[1]))); time.sleep(120)'

# A stand-in for a leftover annex-server: it holds the port and accepts
# connections, which is all the port handling can see of it. Its output goes
# to a file rather than the command substitution's pipe, which the background
# process would otherwise hold open for its whole lifetime.
start_listener() {
    python3 -c "$LISTEN_PY" "$SCRATCH_PORT" >"$TMPDIR_TEST/listener.log" 2>&1 &
    local pid=$!
    local _
    for _ in $(seq 1 40); do
        if [ "$(lsof -ti tcp:$SCRATCH_PORT -sTCP:LISTEN 2>/dev/null)" = "$pid" ]; then
            echo "$pid"
            return 0
        fi
        sleep 0.1
    done
    return 1
}

# A stand-in for a Chromium context with an open connection to the server.
# This is the whole point: a second process matching `lsof -ti :PORT`.
start_client() {
    python3 -c "$CLIENT_PY" "$SCRATCH_PORT" >"$TMPDIR_TEST/client.log" 2>&1 &
    local pid=$!
    sleep 0.5
    echo "$pid"
}

lsof -ti tcp:$SCRATCH_PORT 2>/dev/null | xargs -r kill -9 2>/dev/null
sleep 0.5

# Load the helpers, then point every piece of shared state at the scratch port
# and this test's tmpdir, so a real run on port 3000 is never touched.
# shellcheck source=../e2e-server.sh
source "$REPO_ROOT/scripts/e2e-server.sh"

# Sourcing brings that script's `set -euo pipefail` with it. This test wants
# to run every case and report at the end, so errexit goes back off; without
# this the cleanup trap aborts on the first `lsof` that matches nothing and
# takes the exit status with it.
set +e

PORT=$SCRATCH_PORT
PID_FILE="$TMPDIR_TEST/pid"
DB_DIR_FILE="$TMPDIR_TEST/dbdir"
LOG_FILE="$TMPDIR_TEST/log"

echo ""
echo ">>> stop_server kills a stray listener while a client is connected"
LISTENER=$(start_listener) || { echo "  FAIL — listener never came up"; exit 1; }
CLIENT=$(start_client)

matched=$(lsof -ti :$SCRATCH_PORT 2>/dev/null | wc -l)
if [ "$matched" -ge 2 ]; then
    ok "the connected client appears alongside the listener ($matched pids)"
else
    bad "expected the client to match too, got $matched pid(s) — not exercising the case"
fi

stop_server >/dev/null 2>&1

if proc_alive "$LISTENER"; then
    bad "stray listener (pid $LISTENER) survived stop_server"
else
    ok "stray listener was killed"
fi

if [ -n "$(lsof -ti tcp:$SCRATCH_PORT -sTCP:LISTEN 2>/dev/null)" ]; then
    bad "port $SCRATCH_PORT is still held after stop_server returned"
else
    ok "port $SCRATCH_PORT is free after stop_server returned"
fi

echo ""
echo ">>> stop_server leaves unrelated processes alone"
LISTENER2=$(start_listener) || { echo "  FAIL — listener never came up"; exit 1; }
CLIENT2=$(start_client)
stop_server >/dev/null 2>&1
if proc_alive "$CLIENT2"; then
    ok "the connected client was not killed"
else
    bad "stop_server killed a client process that merely had the port open"
fi

echo ""
echo ">>> start_server refuses to run against a port it does not own"
LISTENER3=$(start_listener) || { echo "  FAIL — listener never came up"; exit 1; }
# stop_server is what would normally clear the port; neutralised here so the
# precondition is what is under test. This has to be cheap and immediate: the
# check sits ahead of the zk/vite/cargo builds, so a refusal costs nothing and
# a missing one costs ten minutes and a run against the wrong database.
stop_server() { :; }
start_out=$(start_server 2>&1)
start_rc=$?
if [ "$start_rc" -ne 0 ]; then
    ok "start_server returned non-zero ($start_rc)"
else
    bad "start_server returned 0 with a foreign process holding the port"
fi
if echo "$start_out" | grep -q "Server ready"; then
    bad "start_server reported \"Server ready\" against a server it did not start"
else
    ok "start_server did not report ready"
fi
if echo "$start_out" | grep -q "still held"; then
    ok "start_server said why"
else
    bad "start_server gave no reason: $start_out"
fi

echo ""
echo "=== $PASS passed, $FAIL failed ==="
# Explicit, because the EXIT trap runs after this and would otherwise decide
# the script's status.
if [ "$FAIL" -eq 0 ]; then exit 0; fi
exit 1
