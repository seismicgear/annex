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

# ── Restoring a TRACKED file is the dangerous part of this test ─────────────
#
# This test doctors `zk/artifacts/ceremony/transcript.json` in place, which is a
# committed artifact, and the previous version left it doctored. Its `restore()`
# deleted the backup as it copied, so the second call found nothing to copy
# from; every later mutation stayed on disk, and the run ended with a tracked
# transcript carrying a `1111…` beacon signature. Nothing downstream could tell
# that from a real corruption: `verify-ceremony.js` failed, the production ZK
# gate failed with it, and `git status` showed one modified artifact with no
# indication that a test had done it.
#
# So: the backup lives in its own directory that is removed only at the very
# end, `restore` is idempotent and never deletes it, restoration happens after
# EVERY mutation rather than only at exit, and the last thing this file does is
# assert that the transcript is byte-identical to what it started as. A test
# that can corrupt the repository has to prove it did not.
BACKUP_DIR="$(mktemp -d)"
backup="${BACKUP_DIR}/transcript.json"
cp "$CEREMONY" "$backup"
ORIGINAL_SHA="$(sha256sum "$CEREMONY" | cut -d" " -f1)"
restore() { [ -f "$backup" ] && cp "$backup" "$CEREMONY"; }
cleanup() { restore; rm -rf "$BACKUP_DIR"; }
trap cleanup EXIT

# Mutate the transcript with a small node one-liner and run the verifier.
# Returns the verifier's exit code; the output is left in LAST_OUT.
mutate_and_run() {
  local expr="$1"
  # Always bound, so a node failure below cannot trip `set -u` and take the
  # rest of the suite with it. That is how four of these cases came to report
  # "the message does not mention ..." about an empty string.
  LAST_OUT=""
  cp "$backup" "$CEREMONY"
  if ! node -e "
    const fs=require('fs'); const p='$CEREMONY';
    const t=JSON.parse(fs.readFileSync(p,'utf8'));
    ($expr)(t);
    fs.writeFileSync(p, JSON.stringify(t,null,2));
  " 2>&1; then
    LAST_OUT="the mutation itself failed — the expression does not match the transcript's shape"
    restore
    return 99
  fi
  local out
  out="$(cd "$ZK_DIR" && node scripts/verify-ceremony.js 2>&1)"
  local rc=$?
  LAST_OUT="$out"
  # Immediately, not at exit. The transcript spends the shortest possible time
  # in a doctored state.
  restore
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

# Every case below addresses `t.phase2.beacon`. Four of them said `t.phase1`,
# where there is no beacon at all: the mutation threw, the verifier never ran,
# and the case was reported as "rejected, but the message does not mention X"
# about an empty string. Four negative cases that tested nothing while failing
# loudly enough to look like they were testing something.
#
# The fail-open that started this: an unreachable/erroring beacon endpoint.
# Simulated by pointing the transcript at a round that does not exist, which is
# what a non-200 looks like from the script's side.
expect_failure "nonexistent beacon round" \
  't => { t.phase2.beacon.round = 999999999; }' \
  "beacon"

# Altered beacon metadata: the randomness no longer matches the chain.
expect_failure "tampered randomness" \
  't => { t.phase2.beacon.randomness = "0".repeat(64); }' \
  "randomness"

# A signature that is not the one drand published.
expect_failure "tampered signature" \
  't => { t.phase2.beacon.signature = "1".repeat(192); }' \
  "signature"

# A different chain than the one the transcript claims.
expect_failure "wrong chain public key" \
  't => { t.phase2.beacon.chainPublicKey = "a".repeat(96); }' \
  "public key"

# The commitment recorded AFTER its beacon was public — the case that proves
# the beacon added no unpredictability. This is the one the previous ceremony
# actually exhibited on phase 2, and the verifier could not see it.
expect_failure "commitment recorded after the beacon" \
  't => { t.phase2.beacon.committedAt = new Date(Date.now()).toISOString(); }' \
  "predate"

# The clock-independent form of the same claim.
expect_failure "beacon round already existed at commit time" \
  't => { t.phase2.beacon.latestRoundAtCommit = t.phase2.beacon.round + 1; }' \
  "future"

# ── And the repository has to be where we found it ──────────────────────────
restore
FINAL_SHA="$(sha256sum "$CEREMONY" | cut -d" " -f1)"
if [ "$FINAL_SHA" = "$ORIGINAL_SHA" ]; then
  cv_ok "the committed transcript is byte-identical to how this test found it"
else
  cv_bad "THIS TEST LEFT THE TRACKED TRANSCRIPT MODIFIED (${ORIGINAL_SHA:0:12} -> ${FINAL_SHA:0:12}) — restore it with 'git checkout zk/artifacts/ceremony/transcript.json'"
fi

echo
echo "[ceremony] ${CV_OK} passed, ${CV_BAD} failed"
[ "$CV_BAD" -eq 0 ]
