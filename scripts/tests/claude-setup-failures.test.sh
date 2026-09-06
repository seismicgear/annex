#!/usr/bin/env bash
# claude-setup-failures.test.sh — the failure paths of scripts/claude-setup.sh.
#
# Setup is the first thing a new environment runs, so its diagnostics are the
# only thing between an operator and a silent abort. Every message asserted
# here was unreachable when this test was written.
#
# The script derives REPO_ROOT from `dirname "$0"`, so a symlink inside a
# scratch tree runs the real script against an empty repo — every section that
# is skipped on a provisioned machine (system deps present, ZK keys present,
# node_modules present) is reachable there, and nothing touches this checkout.
#
# Usage: bash scripts/tests/claude-setup-failures.test.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
REAL_SCRIPT="$REPO_ROOT/scripts/claude-setup.sh"

PASS=0
FAIL=0
ok()  { echo "  ok   — $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL — $1"; FAIL=$((FAIL + 1)); }

WORK=$(mktemp -d /tmp/claude-setup-test-XXXXXX)
trap 'rm -rf "$WORK"' EXIT

# A scratch repo root: the script's own path decides where it thinks it is.
FAKE_ROOT="$WORK/root"
mkdir -p "$FAKE_ROOT/scripts" "$FAKE_ROOT/zk/bin" "$FAKE_ROOT/client"
ln -s "$REAL_SCRIPT" "$FAKE_ROOT/scripts/claude-setup.sh"
: > "$FAKE_ROOT/zk/bin/circom"   # the gate on the ZK generation branch

# Stubs, ahead of the real tools on PATH. Each exits with the code its
# STUB_*_RC says, so one script can drive every failure independently.
STUBS="$WORK/bin"
mkdir -p "$STUBS"
make_stub() {
    local name="$1" var="$2"
    cat > "$STUBS/$name" <<STUB
#!/bin/sh
echo "[stub $name] \$*"
exit \${$var:-0}
STUB
    chmod +x "$STUBS/$name"
}
make_stub pkg-config STUB_PKGCONFIG_RC
make_stub apt-get     STUB_APT_RC
# npm runs in two places — zk/ and client/ — and the cases below need to fail
# them independently, so this stub keys off its working directory.
cat > "$STUBS/npm" <<'STUB'
#!/bin/sh
echo "[stub npm] $* (cwd=$PWD)"
case "$PWD" in
    */client) exit ${STUB_NPM_CLIENT_RC:-0} ;;
    *)        exit ${STUB_NPM_RC:-0} ;;
esac
STUB
chmod +x "$STUBS/npm"
make_stub node        STUB_NODE_RC
make_stub cargo       STUB_CARGO_RC
make_stub espeak-ng   STUB_ESPEAK_RC

run_setup() {
    env PATH="$STUBS:$PATH" \
        STUB_PKGCONFIG_RC="${STUB_PKGCONFIG_RC:-1}" \
        STUB_APT_RC="${STUB_APT_RC:-0}" \
        STUB_NPM_RC="${STUB_NPM_RC:-0}" \
        STUB_NPM_CLIENT_RC="${STUB_NPM_CLIENT_RC:-0}" \
        STUB_NODE_RC="${STUB_NODE_RC:-0}" \
        STUB_CARGO_RC="${STUB_CARGO_RC:-0}" \
        bash "$FAKE_ROOT/scripts/claude-setup.sh" 2>&1
}

# Each case starts from a root with nothing generated in it.
reset_root() {
    rm -rf "$FAKE_ROOT/zk/keys" "$FAKE_ROOT/client/node_modules" \
           "$FAKE_ROOT/assets"
}

echo ""
echo ">>> a failed system-dependency install says so"
reset_root
out=$(STUB_APT_RC=100 run_setup); rc=$?
if [ "$rc" -ne 0 ]; then
    ok "exits non-zero (rc=$rc)"
else
    bad "exited 0 after apt-get failed"
fi
if echo "$out" | grep -q "System dependency install failed"; then
    ok "names the failure"
else
    bad "no diagnostic; operator sees only: $(echo "$out" | tail -2 | tr '\n' ' ')"
fi

echo ""
echo ">>> a failed ZK key generation degrades to dummy vkeys, as documented"
reset_root
out=$(STUB_NODE_RC=1 run_setup); rc=$?
if echo "$out" | grep -q "dummy vkeys"; then
    ok "warns about dummy vkeys"
else
    bad "no dummy-vkey warning; the fallback it documents was never announced"
fi
if [ "$rc" -eq 0 ]; then
    ok "setup continues (rc=0)"
else
    bad "setup aborted (rc=$rc) even though dummy vkeys are the documented fallback"
fi

echo ""
echo ">>> a failed frontend install says so"
reset_root
out=$(STUB_NPM_CLIENT_RC=7 run_setup); rc=$?
if echo "$out" | grep -q "Frontend dependencies installed."; then
    bad "reported \"Frontend dependencies installed.\" after npm ci failed"
else
    ok "does not claim the install succeeded"
fi
if echo "$out" | grep -q "Frontend dependency install failed"; then
    ok "names the failure"
else
    bad "aborted with no diagnostic: $(echo "$out" | tail -2 | tr '\n' ' ')"
fi
if [ "$rc" -ne 0 ]; then
    ok "exits non-zero (rc=$rc)"
else
    bad "exited 0 after npm ci failed"
fi

echo ""
echo ">>> a failing cargo check warns without ending setup"
# This branch was already correct, and only because `pipefail` makes the
# pipeline carry cargo's status rather than tail's. Pinned so it stays that
# way: setup is meant to finish and tell you, not stop at the last step.
reset_root
out=$(STUB_CARGO_RC=101 run_setup); rc=$?
if echo "$out" | grep -q "Rust compilation had issues"; then
    ok "warns about the compile"
else
    bad "no warning about the failed cargo check"
fi
if [ "$rc" -eq 0 ]; then
    ok "setup still finishes (rc=0)"
else
    bad "setup aborted (rc=$rc) on a check that is advisory"
fi

echo ""
echo ">>> a clean run still reports ready"
reset_root
out=$(run_setup); rc=$?
if [ "$rc" -eq 0 ] && echo "$out" | grep -q "Environment Ready"; then
    ok "rc=0 and reports ready"
else
    bad "clean run failed (rc=$rc): $(echo "$out" | tail -3 | tr '\n' ' ')"
fi

echo ""
echo "=== $PASS passed, $FAIL failed ==="
if [ "$FAIL" -eq 0 ]; then exit 0; fi
exit 1
