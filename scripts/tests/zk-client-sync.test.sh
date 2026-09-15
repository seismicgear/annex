#!/usr/bin/env bash
# zk-client-sync.test.sh — the client must prove with the key the server verifies.
#
# `scripts/prepare-zk-dev.js` copies the proving keys from `zk/keys` into
# `client/public/zk`, where the browser proof worker fetches them. It used to
# decide it had nothing to do whenever the destination files EXISTED.
#
# That is wrong in the one case that matters. Rotate the keys — a trusted-setup
# ceremony, a circuit rebuild, a rebase that brings new artifacts — and the
# destinations still exist, so the script skipped, the client kept proving with
# the OLD proving key, and the server rejected every proof against the NEW
# verifying key. What a user saw was `invalid proof` on the first screen of the
# app, with nothing anywhere saying the two halves had drifted apart.
#
# It happened here. Running the ceremony replaced all five `_final.zkey` files
# in `zk/keys`; the next UI audit failed its founder setup with `invalid proof`
# and exercised 0 of 104 surfaces. The wasm files were identical (the circuits
# had not changed), so only the zkeys were stale — which is exactly the state an
# existence check cannot see.
#
# This drives the real script against a scratch tree and asserts it notices a
# byte difference, not just an absence.
#
# Run: bash scripts/tests/zk-client-sync.test.sh

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="${ROOT_DIR}/scripts/prepare-zk-dev.js"

ZS_OK=0
ZS_BAD=0
zs_ok()  { echo "[zk-sync] OK   $1"; ZS_OK=$((ZS_OK + 1)); }
zs_bad() { echo "[zk-sync] FAIL $1" >&2; ZS_BAD=$((ZS_BAD + 1)); }

# ── 1. The live tree must already be in sync ────────────────────────────────
#
# Asserted first and separately from the synthetic cases below: this is the
# condition that actually broke the audit, and it is the one a contributor can
# reintroduce by running the ceremony and committing without re-syncing.
if [ -d "${ROOT_DIR}/client/public/zk" ] && [ -d "${ROOT_DIR}/zk/keys" ]; then
  drift=0
  for src in "${ROOT_DIR}"/zk/keys/*_final.zkey; do
    [ -e "$src" ] || continue
    dest="${ROOT_DIR}/client/public/zk/$(basename "$src")"
    # Only the circuits the client actually serves are mirrored; a key with no
    # destination is not drift.
    [ -e "$dest" ] || continue
    if ! cmp -s "$src" "$dest"; then
      zs_bad "client/public/zk/$(basename "$src") differs from zk/keys — the client would prove with a key the server does not verify. Run: node scripts/prepare-zk-dev.js"
      drift=1
    fi
  done
  if [ "$drift" -eq 0 ]; then
    zs_ok "every mirrored proving key in client/public/zk matches zk/keys"
  fi
else
  zs_ok "no zk artifacts present in this checkout — skipping the live-tree check"
fi

# ── 2. The script must detect a CONTENT difference ──────────────────────────
#
# Driven against a scratch copy so the real tree is untouched. Only the
# membership zkey is perturbed: a single stale file is the realistic case and
# the hardest for an existence check.
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

if [ ! -f "${ROOT_DIR}/zk/keys/membership_final.zkey" ]; then
  zs_ok "no membership_final.zkey in this checkout — skipping the drift-detection check"
else
  # A git worktree-shaped scratch: the script resolves its root with
  # `git rev-parse --show-toplevel`, so the copy has to be a repository.
  mkdir -p "$scratch/repo"
  ( cd "$scratch/repo" && git init -q . )
  mkdir -p "$scratch/repo/zk/keys" "$scratch/repo/zk/build" "$scratch/repo/client/public/zk" "$scratch/repo/scripts"
  cp "${SCRIPT}" "$scratch/repo/scripts/prepare-zk-dev.js"
  cp -r "${ROOT_DIR}/zk/keys/." "$scratch/repo/zk/keys/"
  if [ -d "${ROOT_DIR}/zk/build" ]; then
    cp -r "${ROOT_DIR}/zk/build/." "$scratch/repo/zk/build/"
  fi
  cp -r "${ROOT_DIR}/client/public/zk/." "$scratch/repo/client/public/zk/"

  # Baseline: an in-sync tree is a no-op.
  out="$( cd "$scratch/repo" && node scripts/prepare-zk-dev.js 2>&1 )"
  if printf '%s' "$out" | grep -q 'Nothing to do'; then
    zs_ok "an in-sync tree is reported as nothing to do"
  else
    zs_bad "an in-sync tree should be a no-op, got: $(printf '%s' "$out" | tail -3)"
  fi

  # Now corrupt ONE destination zkey's content while keeping its size and
  # keeping the file present. An existence check cannot see this.
  dest="$scratch/repo/client/public/zk/membership_final.zkey"
  printf 'STALE' | dd of="$dest" bs=1 seek=64 conv=notrunc status=none

  out="$( cd "$scratch/repo" && node scripts/prepare-zk-dev.js 2>&1 )"
  rc=$?
  if [ "$rc" -ne 0 ]; then
    zs_bad "the script exited $rc on a recoverable drift: $(printf '%s' "$out" | tail -3)"
  elif printf '%s' "$out" | grep -q 'membership_final.zkey: differs'; then
    zs_ok "a byte-level difference in a present file is detected and named"
  else
    zs_bad "drift went undetected — this is the exact failure the script exists to prevent: $(printf '%s' "$out" | tail -5)"
  fi

  if cmp -s "$scratch/repo/zk/keys/membership_final.zkey" "$dest"; then
    zs_ok "the stale key was replaced with the one from zk/keys"
  else
    zs_bad "the script reported drift but did not repair it"
  fi

  # And a missing file must still be handled — the case the old check DID cover.
  rm -f "$dest"
  out="$( cd "$scratch/repo" && node scripts/prepare-zk-dev.js 2>&1 )"
  if [ -f "$dest" ] && cmp -s "$scratch/repo/zk/keys/membership_final.zkey" "$dest"; then
    zs_ok "a missing key is still restored"
  else
    zs_bad "a missing key was not restored: $(printf '%s' "$out" | tail -3)"
  fi
fi

echo
echo "[zk-sync] ${ZS_OK} passed, ${ZS_BAD} failed"
[ "$ZS_BAD" -eq 0 ]
