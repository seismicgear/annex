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

# ── ONE RUN AT A TIME ──────────────────────────────────────────────────────
#
# `e2e-server.sh start` stops whatever holds port 3000 before starting its own
# server, so a second audit launched while one is in flight kills the first
# one's server mid-capture and the two then fight over the port. Neither side
# reports a collision: the victim logs ordinary capture failures, and the new
# run fails its founder setup with "founder must be the earliest registrant"
# because it is talking to a database it did not create.
#
# That cost two full cycles and a wrong diagnosis before it was understood, and
# writing the rule down in CLAUDE.md did not stop it happening again — the
# mistake is easy to make when you wait for a `record` to finish so you can
# commit, forgetting the `verify` behind it is still running. So the script
# refuses rather than relying on anyone remembering.
#
# `flock` is used without a timeout on purpose: failing fast with a clear
# message is more useful than a queued run that starts unattended twenty
# minutes later against a tree that has moved on.
#
# The lock guards more than a second audit now: `e2e-server.sh` consults it
# before touching port 3000, so `e2e-all.sh`, a bare `e2e-server.sh start` or
# a mistyped command can no longer stop this run's server either. That was not
# hypothetical — it happened while the guard was being written.
# `e2e-server.sh` refuses to touch port 3000 while this lock is held, which is
# what stops anything else from killing a run mid-capture. This run's own
# server calls have to be exempt, or the audit could not start its server.
export ANNEX_AUDIT_CHILD=1

AUDIT_LOCK="${TMPDIR:-/tmp}/annex-ui-audit.lock"
exec {AUDIT_LOCK_FD}>"$AUDIT_LOCK"
if ! flock -n "$AUDIT_LOCK_FD"; then
    echo "[ui-audit] another audit run holds $AUDIT_LOCK — refusing to start." >&2
    echo "[ui-audit] a run is record THEN verify; wait for both, not just the record." >&2
    exit 1
fi

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

PW_ARGS=()
if [ "$UPDATE_BASELINES" -eq 1 ]; then
    # `--update-snapshots` records baselines instead of failing on mismatch.
    # Recording is deliberately opt-in: a run that silently rewrote its own
    # baselines could never detect a visual regression.
    echo "[ui-audit] recording baselines (--update-snapshots)"
    PW_ARGS+=(--update-snapshots)
fi

# A failing capture is not a reason to skip the report — it is the main
# reason to produce one. The runner records unreachable surfaces as findings
# and still writes the ledger, so the contact sheet is the fastest way to see
# what broke. The capture's exit status is preserved and re-applied at the end
# so CI still fails.
# The ledgers are append-only within a run, so they have to start empty or the
# report would mix this run's findings with the previous one's. `captured.jsonl`
# is how the report knows what this run exercised at all — stale, it would let a
# run that captured nothing inherit the last one's clean sweep. Diagnostics
# go the same way — a stale screenshot of a surface that has since been fixed
# is worse than none, because it looks like current evidence.
rm -f docs/ui-audit/findings.jsonl docs/ui-audit/captured.jsonl
rm -rf client/e2e/audit/diagnostics

echo "[ui-audit] capturing surfaces"
capture_status=0
(cd client && npx playwright test --project=audit --reporter=list "${PW_ARGS[@]+"${PW_ARGS[@]}"}" "${GREP_ARGS[@]+"${GREP_ARGS[@]}"}") || capture_status=$?

echo "[ui-audit] rendering contact sheet"
node scripts/ui-audit-report.mjs

echo "[ui-audit] done"
echo "  baselines : client/e2e/audit/baselines/"
echo "  ledger    : docs/ui-audit/findings.json"
echo "  report    : docs/ui-audit/index.html"

if [ "$capture_status" -ne 0 ]; then
    echo "[ui-audit] capture reported failures — see the report above" >&2
fi
exit "$capture_status"
