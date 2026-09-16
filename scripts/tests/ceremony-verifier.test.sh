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

# ── This test no longer touches the tracked transcript ─────────────────────
#
# It used to doctor `zk/artifacts/ceremony/transcript.json` in place and restore
# it afterwards. Two separate failures came out of that, and the second is why
# in-place-with-restore is not good enough however carefully it is written:
#
#   1. `restore()` deleted the backup as it copied, so the second call had
#      nothing to copy from. Every mutation after the first stayed on disk and a
#      run ended with a tracked transcript carrying a `1111…` beacon signature.
#      `verify-ceremony.js` failed on it, the production ZK gate failed with it,
#      and `git status` showed one modified artifact with no sign a test had done
#      it.
#   2. Fixing (1) — restore after every mutation, plus a byte-identical
#      assertion at the end — still leaves a window. Between a mutation and its
#      restore the repository holds a corrupt artifact, and anything reading the
#      working tree in that window sees it. Commit `7d0b2f4` shipped a transcript
#      with a `"aaaa…"` chain public key, taken from this test's own "wrong chain
#      public key" case, because a `git add -A` landed mid-run. No assertion
#      inside the test can prevent that: the assertion runs after the window
#      closes.
#
# So the hazard is removed rather than detected. The whole ceremony directory is
# copied to a temp dir, the doctored transcripts are written THERE, and
# `verify-ceremony.js --transcript <path>` is pointed at the copy. The tracked
# file is read once and never written.
WORK_DIR="$(mktemp -d)"
CEREMONY_SRC_DIR="$(dirname "$CEREMONY")"
cp -a "${CEREMONY_SRC_DIR}/." "${WORK_DIR}/"
backup="${WORK_DIR}/transcript.json.pristine"
cp "$CEREMONY" "$backup"
ORIGINAL_SHA="$(sha256sum "$CEREMONY" | cut -d" " -f1)"
WORK_TRANSCRIPT="${WORK_DIR}/transcript.json"
cleanup() { rm -rf "$WORK_DIR"; }
trap cleanup EXIT

# Mutate the transcript with a small node one-liner and run the verifier.
# Returns the verifier's exit code; the output is left in LAST_OUT.
mutate_and_run() {
  local expr="$1"
  # Always bound, so a node failure below cannot trip `set -u` and take the
  # rest of the suite with it. That is how four of these cases came to report
  # "the message does not mention ..." about an empty string.
  LAST_OUT=""
  cp "$backup" "$WORK_TRANSCRIPT"
  if ! node -e "
    const fs=require('fs'); const p='$WORK_TRANSCRIPT';
    const t=JSON.parse(fs.readFileSync(p,'utf8'));
    ($expr)(t);
    fs.writeFileSync(p, JSON.stringify(t,null,2));
  " 2>&1; then
    LAST_OUT="the mutation itself failed — the expression does not match the transcript's shape"
    return 99
  fi
  local out
  out="$(cd "$ZK_DIR" && node scripts/verify-ceremony.js --transcript "$WORK_TRANSCRIPT" 2>&1)"
  local rc=$?
  LAST_OUT="$out"
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
#
# Read from the tracked location, so this is a statement about the repository and
# not about the copy.
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
#
# Kept even though nothing in this file writes the tracked path any more. It is
# now a statement that the design holds rather than a cleanup check, and it costs
# one sha256 — if a future edit reintroduces an in-place mutation, this is what
# says so.
FINAL_SHA="$(sha256sum "$CEREMONY" | cut -d" " -f1)"
if [ "$FINAL_SHA" = "$ORIGINAL_SHA" ]; then
  cv_ok "the tracked transcript was never written by this test"
else
  cv_bad "THIS TEST MODIFIED THE TRACKED TRANSCRIPT (${ORIGINAL_SHA:0:12} -> ${FINAL_SHA:0:12}) — it is supposed to work on a temp copy; restore with 'git checkout zk/artifacts/ceremony/transcript.json'"
fi

echo
echo "[ceremony] ${CV_OK} passed, ${CV_BAD} failed"
[ "$CV_BAD" -eq 0 ]
