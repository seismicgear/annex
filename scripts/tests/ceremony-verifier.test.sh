#!/usr/bin/env bash
# ceremony-verifier.test.sh — the ceremony verifier must FAIL on bad input.
#
# `zk/scripts/verify-ceremony.js` is the gate that stands between a release and
# an unverifiable trusted setup, and it had a fail-open path: `checkBeacon()`
# returned false when drand answered with a non-200 status, and the caller did
#
#     await checkBeacon(transcript);
#
# discarding the result. Provided the artifact chain checked out, the script
# then printed "All N circuit(s) verified against the ceremony transcript" and
# exited 0 — having not verified the beacon at all. A verifier that reports
# success when its network check did not run is worse than no verifier, because
# the green line is what a release reads.
#
# Everything here drives the REAL script against a doctored transcript. A test
# that checked the source for a `return` would pass against a refactor that
# reintroduced the bug somewhere else.
#
# Run: bash scripts/tests/ceremony-verifier.test.sh

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
ZK_DIR="${ROOT_DIR}/zk"
CEREMONY="${ZK_DIR}/artifacts/ceremony/transcript.json"

CV_OK=0
CV_BAD=0
cv_ok()  { echo "[ceremony] OK   $1"; CV_OK=$((CV_OK + 1)); }
cv_bad() { echo "[ceremony] FAIL $1" >&2; CV_BAD=$((CV_BAD + 1)); }

if [ ! -f "$CEREMONY" ]; then
  cv_ok "no ceremony transcript in this checkout — skipping"
  echo "[ceremony] ${CV_OK} passed, ${CV_BAD} failed"
  exit 0
fi

backup="$(mktemp)"
cp "$CEREMONY" "$backup"
restore() { cp "$backup" "$CEREMONY"; rm -f "$backup"; }
trap restore EXIT

# Mutate the transcript with a small node one-liner and run the verifier.
# Returns the verifier's exit code; prints its output to stderr on request.
mutate_and_run() {
  local expr="$1"
  cp "$backup" "$CEREMONY"
  node -e "
    const fs=require('fs'); const p='$CEREMONY';
    const t=JSON.parse(fs.readFileSync(p,'utf8'));
    ($expr)(t);
    fs.writeFileSync(p, JSON.stringify(t,null,2));
  " || return 99
  ( cd "$ZK_DIR" && node scripts/verify-ceremony.js >/tmp/cv_out.$$ 2>&1 )
  local rc=$?
  LAST_OUT="$(cat /tmp/cv_out.$$)"; rm -f /tmp/cv_out.$$
  return $rc
}

expect_failure() {
  local label="$1" expr="$2" needle="$3"
  mutate_and_run "$expr"
  local rc=$?
  if [ "$rc" -eq 0 ]; then
    cv_bad "$label: verifier exited 0 on a transcript it should reject"
  elif printf '%s' "$LAST_OUT" | grep -qi "$needle"; then
    cv_ok "$label: rejected, and the message names the problem"
  else
    cv_bad "$label: rejected (exit $rc) but the message does not mention '$needle': $(printf '%s' "$LAST_OUT" | tail -3)"
  fi
}

# ── The baseline: the real transcript must verify ───────────────────────────
cp "$backup" "$CEREMONY"
if ( cd "$ZK_DIR" && node scripts/verify-ceremony.js >/tmp/cv_base.$$ 2>&1 ); then
  cv_ok "the committed ceremony verifies"
else
  cv_bad "the committed ceremony does NOT verify: $(tail -4 /tmp/cv_base.$$)"
fi
rm -f /tmp/cv_base.$$

# ── Negative cases ──────────────────────────────────────────────────────────

# The fail-open that started this: an unreachable/erroring beacon endpoint.
# Simulated by pointing the transcript at a round that does not exist, which is
# what a non-200 looks like from the script's side.
expect_failure "nonexistent beacon round" \
  't => { t.phase1.beacon.round = 999999999; }' \
  "beacon"

# Altered beacon metadata: the randomness no longer matches the chain.
expect_failure "tampered randomness" \
  't => { t.phase1.beacon.randomness = "0".repeat(64); }' \
  "randomness"

# A signature that is not the one drand published.
expect_failure "tampered signature" \
  't => { t.phase2.beacon.signature = "1".repeat(192); }' \
  "signature"

# A different chain than the one the transcript claims.
expect_failure "wrong chain public key" \
  't => { t.phase1.beacon.chainPublicKey = "a".repeat(96); }' \
  "public key"

# The commitment recorded AFTER its beacon was public — the case that proves
# the beacon added no unpredictability. This is the one the previous ceremony
# actually exhibited on phase 2, and the verifier could not see it.
expect_failure "commitment recorded after the beacon" \
  't => { t.phase2.beacon.committedAt = new Date(Date.now()).toISOString(); }' \
  "predate"

# The clock-independent form of the same claim.
expect_failure "beacon round already existed at commit time" \
  't => { t.phase1.beacon.latestRoundAtCommit = t.phase1.beacon.round + 1; }' \
  "future"

echo
echo "[ceremony] ${CV_OK} passed, ${CV_BAD} failed"
[ "$CV_BAD" -eq 0 ]
