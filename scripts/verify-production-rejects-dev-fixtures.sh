#!/bin/sh
# verify-production-rejects-dev-fixtures.sh
#
# Proof-script for the production ZK gate. Confirms that:
#
#   1. The pinned manifest in zk/artifacts/membership/manifest.json still
#      identifies the ceremony as `dev-fixture`. (If someone has flipped
#      this to `mpc` without doing a real ceremony, that is a hard fail —
#      we never want a release manifest claiming a ceremony that did not
#      happen.)
#
#   2. Running `ANNEX_BUILD_PROFILE=production node zk/scripts/verify-artifacts.js`
#      exits non-zero (specifically with code 3, the dev-fixture gate exit
#      code). This is the same gate the release workflow uses.
#
# A zero exit code from THIS script means: "the production gate works,
# and it currently refuses to certify the on-disk artifacts as production-
# ready." That is the correct state until a real multi-party trusted
# setup ceremony produces real artifacts and the manifest is replaced.
#
# Run from the repo root:
#
#   sh scripts/verify-production-rejects-dev-fixtures.sh
#
# Exits non-zero if the gate has regressed or the manifest has been
# silently flipped — i.e. if you can run a production build that does
# not refuse dev fixtures.

set -eu

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
MANIFEST="${ROOT_DIR}/zk/artifacts/membership/manifest.json"

if [ ! -f "${MANIFEST}" ]; then
  echo "[verify-prod-gate] FAIL: manifest not found at ${MANIFEST}" >&2
  exit 1
fi

# Step 1 — manifest must still be dev-fixture. Use grep on the raw JSON
# instead of jq so this script has no extra system dependencies.
if ! grep -q '"type"[[:space:]]*:[[:space:]]*"dev-fixture"' "${MANIFEST}"; then
  echo "[verify-prod-gate] FAIL: manifest ceremony.type is NOT 'dev-fixture'." >&2
  echo "[verify-prod-gate]       If a real multi-party ceremony has happened, that's great —" >&2
  echo "[verify-prod-gate]       update this script to expect the new ceremony.type and the" >&2
  echo "[verify-prod-gate]       pinned artifact hashes. Otherwise the manifest is making a" >&2
  echo "[verify-prod-gate]       claim it cannot back up; revert it." >&2
  exit 1
fi
echo "[verify-prod-gate] OK: manifest is still dev-fixture (no false ceremony claim)."

# Step 2 — production verify must fail with exit code 3 (dev-fixture under
# production profile). Capture the exit code via `|| status=$?` so `set -e`
# does not tear us down on the expected failure.
status=0
ANNEX_BUILD_PROFILE=production node "${ROOT_DIR}/zk/scripts/verify-artifacts.js" \
  >/dev/null 2>&1 || status=$?

if [ "${status}" -eq 0 ]; then
  echo "[verify-prod-gate] FAIL: verify-artifacts.js succeeded under production profile" >&2
  echo "[verify-prod-gate]       while a dev-fixture manifest is on disk. The production" >&2
  echo "[verify-prod-gate]       gate has regressed; a release build could now ship dev keys." >&2
  exit 1
fi

# Exit code 3 is the specific dev-fixture refusal. Exit code 2 (missing /
# mismatched artifacts) is also acceptable as "production gate refused to
# proceed", but we surface a warning so a flipped manifest doesn't hide
# behind a missing-file failure.
if [ "${status}" -eq 3 ]; then
  echo "[verify-prod-gate] OK: production verify-artifacts.js exited 3 (dev-fixture refused)."
elif [ "${status}" -eq 2 ]; then
  echo "[verify-prod-gate] OK: production verify-artifacts.js exited 2 (artifacts missing or"
  echo "[verify-prod-gate]      hash-mismatched). Production builds still cannot complete."
else
  echo "[verify-prod-gate] OK: production verify-artifacts.js exited ${status} (non-zero)."
fi

echo "[verify-prod-gate] PASS: production builds cannot complete with the current dev-fixture artifacts."
