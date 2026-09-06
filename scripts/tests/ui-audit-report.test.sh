#!/usr/bin/env bash
# ui-audit-report.test.sh — what the contact sheet claims about a run.
#
# "N surfaces captured · 0 findings" is the phrase this project treats as
# proof of health. It was computed from the tracked baselines directory, which
# no run ever clears — so a run that captured nothing at all (server never came
# up, a --grep that matched nothing, playwright failing to launch) rendered
# every previous baseline as its own evidence and announced a clean sweep. The
# same run also overwrote the TRACKED findings.json with an empty list.
#
# The script resolves its paths from its own location, so the cases below copy
# it into a scratch tree with fabricated baselines; this checkout is untouched.
#
# Usage: bash scripts/tests/ui-audit-report.test.sh
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

PASS=0
FAIL=0
ok()  { echo "  ok   — $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL — $1"; FAIL=$((FAIL + 1)); }

WORK=$(mktemp -d /tmp/ui-audit-report-test-XXXXXX)
trap 'rm -rf "$WORK"' EXIT

ROOT="$WORK/root"
DOCS="$ROOT/docs/ui-audit"
BASE="$ROOT/client/e2e/audit/baselines"
mkdir -p "$ROOT/scripts" "$DOCS" "$BASE/desktop" "$BASE/mobile"
cp "$REPO_ROOT/scripts/ui-audit-report.mjs" "$ROOT/scripts/"

for v in desktop mobile; do
    for s in alpha beta gamma; do : > "$BASE/$v/$s.png"; done
done

# A tracked record from an earlier, real run.
PRIOR='{"generatedBy":"earlier-run","findings":[{"surfaceId":"alpha","severity":"p1"}]}'
seed_prior() { echo "$PRIOR" > "$DOCS/findings.json"; }

run_report() { (cd "$ROOT" && node scripts/ui-audit-report.mjs 2>&1); }

echo ""
echo ">>> a run that captured nothing does not claim it captured everything"
rm -f "$DOCS/findings.jsonl" "$DOCS/captured.jsonl"
seed_prior
out=$(run_report)
page=$(cat "$DOCS/index.html" 2>/dev/null || echo "")
if echo "$page" | grep -qE '3 surfaces captured'; then
    bad "page claims \"3 surfaces captured\" after capturing nothing"
else
    ok "page does not claim to have captured them"
fi
if echo "$page" | grep -qiE 'captured nothing|no surfaces were captured'; then
    ok "page says the run captured nothing"
else
    ok_missing=1
    bad "page gives no sign this run captured nothing"
fi
if grep -q 'earlier-run' "$DOCS/findings.json"; then
    ok "the tracked findings.json was left alone"
else
    bad "the tracked findings.json was overwritten by a run that captured nothing"
fi

echo ""
echo ">>> a partial run says it is partial"
seed_prior
printf '{"surfaceId":"alpha","viewport":"desktop"}\n{"surfaceId":"beta","viewport":"desktop"}\n' \
    > "$DOCS/captured.jsonl"
rm -f "$DOCS/findings.jsonl"
out=$(run_report)
page=$(cat "$DOCS/index.html")
if echo "$page" | grep -q '2 of 3'; then
    ok "page reports 2 of 3"
else
    bad "page does not report a partial run: $(echo "$page" | grep -o '<p class="sub">[^<]*</p>')"
fi
if grep -q 'earlier-run' "$DOCS/findings.json"; then
    ok "the tracked findings.json was left alone on a partial run"
else
    bad "a partial run narrowed the tracked findings.json to its subset"
fi

echo ""
echo ">>> a full run reports fully and writes the record"
seed_prior
printf '{"surfaceId":"alpha","viewport":"desktop"}\n{"surfaceId":"beta","viewport":"desktop"}\n{"surfaceId":"gamma","viewport":"desktop"}\n' \
    > "$DOCS/captured.jsonl"
printf '{"surfaceId":"gamma","stage":"01","severity":"p2","audit":"console","detail":"boom"}\n' \
    > "$DOCS/findings.jsonl"
out=$(run_report)
page=$(cat "$DOCS/index.html")
if echo "$page" | grep -q '3 of 3'; then
    ok "page reports 3 of 3"
else
    bad "page does not report a full run: $(echo "$page" | grep -o '<p class="sub">[^<]*</p>')"
fi
if grep -q 'earlier-run' "$DOCS/findings.json"; then
    bad "a full run did not refresh the tracked findings.json"
else
    ok "the tracked findings.json was refreshed"
fi
if grep -q 'boom' "$DOCS/findings.json"; then
    ok "this run's finding is in the record"
else
    bad "the finding did not reach findings.json"
fi

echo ""
echo "=== $PASS passed, $FAIL failed ==="
if [ "$FAIL" -eq 0 ]; then exit 0; fi
exit 1
