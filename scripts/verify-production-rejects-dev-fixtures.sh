#!/bin/sh
# verify-production-rejects-dev-fixtures.sh
#
# Proof-script for the production ZK gate.
#
# The previous version of this script asserted that
# `zk/artifacts/membership/manifest.json` still said `dev-fixture`, and treated
# ANY non-zero exit from `verify-artifacts.js` as proof the gate worked. Three
# things were wrong with that, and all three made it report PASS while the gate
# was not doing its job:
#
#   * It checked ONE manifest of six. `membership_v2` is the DEFAULT identity
#     path; it could have been flipped to a false `mpc` claim and this script
#     would still have printed PASS.
#   * Exit code 1 means "the manifest is unparseable". A corrupted manifest
#     read as "gate working".
#   * It never neutralised `ANNEX_ALLOW_DEV_CEREMONY`. With that set in the
#     caller's environment the dev-fixture gate is bypassed entirely, the run
#     falls through to a missing-artifact failure, and the script called that
#     a pass — while proving nothing about the escape hatch it exists to watch.
#
# It also could not survive its own success: once a real ceremony replaced the
# fixtures, its central assertion ("the manifest must still say dev-fixture")
# would have become false and the gate would have had to be deleted.
#
# This version tests the GATE rather than the current state of the tree. It
# builds throwaway manifests in a temp directory and checks what
# verify-artifacts.js does with each, then checks the real artifacts against
# whichever state the repo is actually in.
#
# Run from anywhere:
#
#   sh scripts/verify-production-rejects-dev-fixtures.sh

set -eu

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
VERIFY="${ROOT_DIR}/zk/scripts/verify-artifacts.js"
ARTIFACTS="${ROOT_DIR}/zk/artifacts"
TRANSCRIPT="${ARTIFACTS}/ceremony/transcript.json"
WORKFLOW="${ROOT_DIR}/.github/workflows/release-desktop.yml"

PASSES=0
FAILURES=0

ok() {
  echo "[prod-gate] OK   $1"
  PASSES=$((PASSES + 1))
}

bad() {
  echo "[prod-gate] FAIL $1" >&2
  FAILURES=$((FAILURES + 1))
}

TMP=$(mktemp -d)
trap 'rm -rf "${TMP}"' EXIT

# Run verify-artifacts.js with a DELIBERATELY SCRUBBED environment.
#
# `ANNEX_ALLOW_DEV_CEREMONY` is the one variable that can turn the gate off, so
# every expectation below is meaningless unless it is unset for the child. `env
# -u` rather than a subshell `unset`, so this holds even if the caller exported
# it read-only or the shell is one that scopes `unset` oddly.
verify_status() {
  status=0
  # shellcheck disable=SC2086
  env -u ANNEX_ALLOW_DEV_CEREMONY ANNEX_BUILD_PROFILE="$1" \
    node "${VERIFY}" $2 >"${TMP}/out.log" 2>&1 || status=$?
  echo "${status}"
}

expect_status() {
  label="$1"
  want="$2"
  got="$3"
  if [ "${got}" -eq "${want}" ]; then
    ok "${label} (exit ${got})"
  else
    bad "${label}: expected exit ${want}, got ${got}"
    sed 's/^/[prod-gate]      /' "${TMP}/out.log" >&2 || true
  fi
}

# ── A throwaway circuit whose artifacts exist and hash correctly ────────────
#
# Real files, real hashes: so when a case below fails, it fails for the reason
# under test and not because a file was missing.
mkdir -p "${TMP}/fixture"
printf 'wasm\n' > "${TMP}/fixture/x.wasm"
printf 'zkey\n' > "${TMP}/fixture/x.zkey"
printf 'vkey\n' > "${TMP}/fixture/x_vkey.json"
SHA_WASM=$(node -e 'const c=require("crypto"),f=require("fs");process.stdout.write(c.createHash("sha256").update(f.readFileSync(process.argv[1])).digest("hex"))' "${TMP}/fixture/x.wasm")
SHA_ZKEY=$(node -e 'const c=require("crypto"),f=require("fs");process.stdout.write(c.createHash("sha256").update(f.readFileSync(process.argv[1])).digest("hex"))' "${TMP}/fixture/x.zkey")
SHA_VKEY=$(node -e 'const c=require("crypto"),f=require("fs");process.stdout.write(c.createHash("sha256").update(f.readFileSync(process.argv[1])).digest("hex"))' "${TMP}/fixture/x_vkey.json")

write_manifest() {
  # $1 = output path, $2 = ceremony JSON (or the literal word "none")
  ceremony="$2"
  if [ "${ceremony}" = "none" ]; then
    ceremony_field=""
  else
    ceremony_field="\"ceremony\": ${ceremony},"
  fi
  cat > "$1" <<MANIFEST
{
  "schemaVersion": 1,
  "circuit": "fixture",
  "circuitVersion": "1.0.0",
  "curve": "bn254",
  "provingSystem": "groth16",
  "treeDepth": 20,
  "publicSignals": ["root", "commitment"],
  "wasm_sha256": "${SHA_WASM}",
  "zkey_sha256": "${SHA_ZKEY}",
  "vkey_sha256": "${SHA_VKEY}",
  ${ceremony_field}
  "paths": { "wasm": "./x.wasm", "zkey": "./x.zkey", "vkey": "./x_vkey.json" }
}
MANIFEST
}

# 1. dev-fixture under production → refused with the dedicated exit code 3.
write_manifest "${TMP}/fixture/manifest.json" '{"type":"dev-fixture"}'
expect_status "dev-fixture manifest refused under production" 3 \
  "$(verify_status production "--manifest ${TMP}/fixture/manifest.json")"

# 2. ...and accepted under dev, so the gate is a production gate and not a
#    blanket refusal that would make local work impossible.
expect_status "dev-fixture manifest accepted under dev" 0 \
  "$(verify_status dev "--manifest ${TMP}/fixture/manifest.json")"

# 3. A ceremony type the gate does not know is refused rather than waved
#    through. Without this, inventing a type name is a bypass.
write_manifest "${TMP}/fixture/manifest.json" '{"type":"totally-legit-ceremony"}'
expect_status "unknown ceremony.type refused under production" 3 \
  "$(verify_status production "--manifest ${TMP}/fixture/manifest.json")"

# 4. A real-sounding ceremony claim with no transcript is refused. This is the
#    case the old script could not catch: someone editing `dev-fixture` to
#    `mpc` to get a build out.
write_manifest "${TMP}/fixture/manifest.json" '{"type":"mpc"}'
expect_status "ceremony claim without a transcript refused under production" 3 \
  "$(verify_status production "--manifest ${TMP}/fixture/manifest.json")"

# 5. No ceremony block at all is refused under production.
write_manifest "${TMP}/fixture/manifest.json" none
expect_status "manifest with no ceremony block refused under production" 3 \
  "$(verify_status production "--manifest ${TMP}/fixture/manifest.json")"

# 6. A transcript that is NAMED but not PRESENT is still a claim with no
#    evidence behind it, and is the likelier accident of the two.
write_manifest "${TMP}/fixture/manifest.json" '{"type":"single-contributor-beacon","transcript":"../ceremony/transcript.json"}'
expect_status "ceremony naming a missing transcript refused under production" 3 \
  "$(verify_status production "--manifest ${TMP}/fixture/manifest.json")"

# 7. A well-formed ceremony claim WITH a transcript present passes the
#    provenance gate — and then the hashes are still checked, so a tampered
#    artifact is exit 2 rather than 0.
mkdir -p "${TMP}/ceremony"
printf '{"schemaVersion":1}\n' > "${TMP}/ceremony/transcript.json"
expect_status "beacon ceremony with transcript passes the provenance gate" 0 \
  "$(verify_status production "--manifest ${TMP}/fixture/manifest.json")"

printf 'tampered\n' > "${TMP}/fixture/x.zkey"
expect_status "tampered artifact refused under production" 2 \
  "$(verify_status production "--manifest ${TMP}/fixture/manifest.json")"

# ── The real tree ──────────────────────────────────────────────────────────
#
# Which assertion is correct here depends on whether the ceremony has been run.
# Both states are legitimate; silently passing in either without saying which
# one is not.
if [ -f "${TRANSCRIPT}" ]; then
  expect_status "every pinned manifest verifies under production" 0 \
    "$(verify_status production --all)"

  if grep -rl '"type"[[:space:]]*:[[:space:]]*"dev-fixture"' "${ARTIFACTS}" >/dev/null 2>&1; then
    bad "a ceremony transcript exists but some manifest is still marked dev-fixture"
    grep -rl '"type"[[:space:]]*:[[:space:]]*"dev-fixture"' "${ARTIFACTS}" >&2
  else
    ok "no pinned manifest claims dev-fixture"
  fi
else
  echo "[prod-gate] NOTE no ceremony transcript at ${TRANSCRIPT}."
  echo "[prod-gate]      The tree is still on dev fixtures, so a production build MUST refuse."
  status=$(verify_status production --all)
  if [ "${status}" -eq 0 ]; then
    bad "production --all succeeded with no ceremony transcript on disk"
  else
    ok "production --all refuses the un-ceremonied tree (exit ${status})"
  fi
fi

# ── The workflow has to actually call the gate ──────────────────────────────
#
# A gate nothing invokes is decoration. The old script proved the script
# refuses and never checked that a release runs it.
if [ -f "${WORKFLOW}" ]; then
  if grep -q 'verify-artifacts.js --all' "${WORKFLOW}"; then
    ok "release-desktop.yml runs verify-artifacts.js --all"
  else
    bad "release-desktop.yml does not run 'verify-artifacts.js --all' — a release would gate one circuit, not all of them"
  fi
  # Comments are stripped first. A line WARNING against the bypass is the
  # opposite of a line setting it, and a guard that cannot tell them apart
  # punishes the documentation it wants.
  if grep -v '^[[:space:]]*#' "${WORKFLOW}" | grep -q 'ANNEX_ALLOW_DEV_CEREMONY'; then
    bad "release-desktop.yml SETS ANNEX_ALLOW_DEV_CEREMONY — the dev-fixture bypass must never appear in a tag-driven release"
  else
    ok "release-desktop.yml does not set the dev-ceremony bypass"
  fi
else
  bad "release-desktop.yml not found at ${WORKFLOW}"
fi

# ── And something has to actually call THIS ────────────────────────────────
#
# The same lesson one level out, and it had already happened here: this script
# asserts that a release runs `verify-artifacts.js`, and nothing ran this
# script. `release-gates.md` named a workflow step for it that does not exist;
# the globbed "Harness script tests" step in ci.yml matches
# `scripts/tests/*.test.sh`, and this file is neither in that directory nor
# named with that suffix, so it matched nothing and nobody noticed. Twelve
# assertions about the most consequential gate in the repo, run by hand, when
# someone remembered.
for caller in "${ROOT_DIR}/scripts/test-all.sh" "${ROOT_DIR}/.github/workflows/ci.yml"; do
  name=$(basename "${caller}")
  if [ ! -f "${caller}" ]; then
    bad "${name} not found — cannot confirm this gate is invoked"
  elif grep -q 'verify-production-rejects-dev-fixtures.sh' "${caller}"; then
    ok "${name} invokes this gate"
  else
    bad "${name} does not invoke this gate — it would run nowhere again"
  fi
done

echo "[prod-gate] ${PASSES} passed, ${FAILURES} failed"
[ "${FAILURES}" -eq 0 ] || exit 1
echo "[prod-gate] PASS: the production ZK gate refuses everything it should and is wired into the release."
