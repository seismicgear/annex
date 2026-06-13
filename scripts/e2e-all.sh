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
#
# Screenshots:
#   Playwright → client/e2e-results/ + client/e2e-report/
#   Puppeteer  → client/e2e-puppeteer/screenshots/
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

LANE="${1:-both}"
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

case "$LANE" in
    playwright) run_playwright ;;
    puppeteer)  run_puppeteer ;;
    both)       run_playwright; run_puppeteer ;;
    *) echo "Usage: $0 {both|playwright|puppeteer}" >&2; exit 1 ;;
esac

echo "[e2e-all] OK — all requested e2e lanes passed"
