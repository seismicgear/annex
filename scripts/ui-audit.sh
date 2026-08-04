#!/usr/bin/env bash
#
# ui-audit.sh — Run the exhaustive UI/UX audit lane end to end.
#
# Captures a screenshot of every surface in `client/e2e/audit/surfaces.ts` at
# every viewport, runs the automated audit battery (axe-core, console errors,
# failed requests, layout overflow, dialog keyboard contract) against each, and
# writes:
#
#   client/e2e/audit/baselines/<viewport>/<surface>.png   tracked baselines
#   docs/ui-audit/findings.json                           machine-readable ledger
#   docs/ui-audit/index.html                              contact sheet
#
# Usage:
#   bash scripts/ui-audit.sh                # full run against a fresh server
#   bash scripts/ui-audit.sh --keep-server  # reuse a running server (fast iteration)
#   bash scripts/ui-audit.sh --grep chat    # only surfaces matching a pattern
#
# WHY THE SERVER IS RESTARTED BY DEFAULT
#
# The audit's `founder` role must be the earliest identity to register, because
# the server promotes the earliest registrant to moderator (`ensure_founder`)
# and that is the only way to reach the admin surfaces. Running against a
# server that already has identities produces a founder with no admin rights
# and the setup project fails loudly rather than capturing a half-empty audit.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

KEEP_SERVER=0
GREP_ARGS=()
UPDATE_BASELINES=0

while [ $# -gt 0 ]; do
    case "$1" in
        --keep-server) KEEP_SERVER=1; shift ;;
        --grep) GREP_ARGS+=(--grep "$2"); shift 2 ;;
        --update-baselines) UPDATE_BASELINES=1; shift ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

STARTED_SERVER=0
cleanup() {
    local code=$?
    if [ "$STARTED_SERVER" -eq 1 ]; then
        echo "[ui-audit] stopping server"
        bash scripts/e2e-server.sh stop || true
    fi
    exit "$code"
}
trap cleanup EXIT INT TERM

if [ "$KEEP_SERVER" -eq 0 ]; then
    echo "[ui-audit] starting a fresh server (clean DB — founder must register first)"
    bash scripts/e2e-server.sh start
    STARTED_SERVER=1
else
    echo "[ui-audit] reusing the running server (--keep-server)"
    echo "[ui-audit] NOTE: warm-role setup will fail if identities already exist."
fi

if [ "$UPDATE_BASELINES" -eq 1 ]; then
    echo "[ui-audit] removing existing baselines so this run rewrites them"
    rm -rf client/e2e/audit/baselines
fi

echo "[ui-audit] capturing surfaces"
(cd client && npx playwright test --project=audit --reporter=list "${GREP_ARGS[@]+"${GREP_ARGS[@]}"}")

echo "[ui-audit] rendering contact sheet"
node scripts/ui-audit-report.mjs

echo "[ui-audit] done"
echo "  baselines : client/e2e/audit/baselines/"
echo "  ledger    : docs/ui-audit/findings.json"
echo "  report    : docs/ui-audit/index.html"
