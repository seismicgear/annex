#!/usr/bin/env bash
#
# e2e-all.sh — Run BOTH browser-automation lanes against one server.
#
# Boots the Annex server (built client dist + fresh DB on :3000), then runs the
# Playwright suite (client/e2e/) and the Puppeteer screenshot harness
# (client/e2e-puppeteer/) back to back, and tears the server down on exit.
#
# Usage:
#   bash scripts/e2e-all.sh                 # both lanes
#   bash scripts/e2e-all.sh playwright      # Playwright only
#   bash scripts/e2e-all.sh puppeteer       # Puppeteer only
#   bash scripts/e2e-all.sh group-call      # the multi-party call lane
#
# The UI audit is NOT one of these — it needs a server it started itself and
# has its own entry point, `scripts/ui-audit.sh`.
#
# Screenshots:
#   Playwright → client/e2e-results/ + client/e2e-report/
#   Puppeteer  → client/e2e-puppeteer/screenshots/
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

LANE="${1:-both}"

# Validated BEFORE anything starts. It used to be checked in the `case` at the
# bottom, after the server had already been started — so a mistyped lane took
# port 3000, printed a usage message and gave the port back, which is enough to
# kill a UI audit mid-run. `e2e-server.sh` refuses during an audit now, but the
# argument still has no business starting a server before it is known to be
# valid.
case "$LANE" in
    playwright|puppeteer|group-call|both) ;;
    *) echo "Usage: $0 {both|playwright|puppeteer|group-call}" >&2; exit 1 ;;
esac

STARTED_SERVER=0

cleanup() {
    local code=$?
    if [ "$STARTED_SERVER" -eq 1 ]; then
        echo "[e2e-all] stopping server"
        bash scripts/e2e-server.sh stop || true
    fi
    exit "$code"
}
trap cleanup EXIT INT TERM

echo "[e2e-all] starting server"
bash scripts/e2e-server.sh start
STARTED_SERVER=1

run_playwright() {
    echo "[e2e-all] ── Playwright ──"
    (cd client && npm run test:e2e)
}

run_puppeteer() {
    echo "[e2e-all] ── Puppeteer ──"
    (cd client && npm run test:e2e:puppeteer)
}

# A separate Playwright project, and until it was added here it had no entry
# point at all: no script, no workflow and no doc named it, so the guard that
# replaced the pinning test for the SFU rearchitecture ran only if someone
# typed the project name by hand. Not part of `both` — it holds a real
# multi-party call and takes minutes.
run_group_call() {
    echo "[e2e-all] ── Group call ──"
    (cd client && npx playwright test --project=group-call --reporter=list)
}

case "$LANE" in
    playwright) run_playwright ;;
    puppeteer)  run_puppeteer ;;
    group-call) run_group_call ;;
    both)
        run_playwright
        # A fresh server between the lanes, not a shared one.
        #
        # The puppeteer harness checks channel creation, which needs a
        # moderator, and `ensure_founder` grants that to the EARLIEST
        # registrant. Sharing one server means Playwright has already
        # registered by the time puppeteer creates its identity, so it comes
        # up as an ordinary member and logs "no create-channel control
        # (identity is not a moderator) — skipping channel-create". The lane
        # still passes, having quietly tested less than it does on its own —
        # which is the whole reason `both` existed as a convenience.
        echo "[e2e-all] restarting the server so the next lane registers first"
        bash scripts/e2e-server.sh restart
        run_puppeteer
        ;;
esac

echo "[e2e-all] OK — all requested e2e lanes passed"
