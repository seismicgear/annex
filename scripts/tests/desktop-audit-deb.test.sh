#!/usr/bin/env bash
# desktop-audit-deb.test.sh — which .deb the desktop audit installs.
#
# The install/launch/uninstall steps are the point of that lane, and they used
# to run against `find target -name '*.deb' | head -1` regardless of whether
# the build that was supposed to produce it had succeeded. A failed bundle
# build followed by an install of an earlier run's artifact reported every
# install step as passing — about a package this run never built.
#
# Usage: bash scripts/tests/desktop-audit-deb.test.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Deliberately not PASS/FAIL: sourcing the script under test brings ITS
# counters of those names, and its `step` increments them — so the tally here
# silently absorbed the `step false` assertion below.
T_PASS=0
T_FAIL=0
ok()  { echo "  ok   — $1"; T_PASS=$((T_PASS + 1)); }
bad() { echo "  FAIL — $1"; T_FAIL=$((T_FAIL + 1)); }

WORK=$(mktemp -d /tmp/desktop-audit-test-XXXXXX)
trap 'rm -rf "$WORK"' EXIT

# Sourcing stops before the audit body; only the helpers come across.
# shellcheck source=../desktop-audit.sh
source "$REPO_ROOT/scripts/desktop-audit.sh"
set +e

mkdir -p "$WORK/target/release/bundle/deb"
OLD="$WORK/target/release/bundle/deb/Annex_0.0.1_amd64.deb"
: > "$OLD"
touch -d '2020-01-01 00:00:00' "$OLD"

echo ""
echo ">>> an artifact older than the build is not offered"
started=$(date +%s)
found=$(newest_deb "$WORK/target" "$started")
if [ -z "$found" ]; then
    ok "a stale .deb is ignored"
else
    bad "offered a .deb from before the build: $found"
fi

echo ""
echo ">>> an artifact this build produced is found"
sleep 1
NEW="$WORK/target/release/bundle/deb/Annex_0.0.2_amd64.deb"
: > "$NEW"
found=$(newest_deb "$WORK/target" "$started")
if [ "$found" = "$NEW" ]; then
    ok "found the fresh one"
else
    bad "expected $NEW, got '$found'"
fi

echo ""
echo ">>> the newest wins when a build produces more than one"
sleep 1
NEWER="$WORK/target/release/bundle/deb/annex_0.0.3_amd64.deb"
: > "$NEWER"
found=$(newest_deb "$WORK/target" "$started")
if [ "$found" = "$NEWER" ]; then
    ok "picked the newest"
else
    bad "expected $NEWER, got '$found'"
fi

echo ""
echo ">>> step reports whether its command succeeded"
if step "true" true >/dev/null 2>&1; then
    ok "step returns 0 for a passing command"
else
    bad "step returned non-zero for a passing command"
fi
if step "false" false >/dev/null 2>&1; then
    bad "step returned 0 for a FAILING command — a caller cannot gate on it"
else
    ok "step returns non-zero for a failing command"
fi

echo ""
echo "=== $T_PASS passed, $T_FAIL failed ==="
if [ "$T_FAIL" -eq 0 ]; then exit 0; fi
exit 1
