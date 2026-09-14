#!/usr/bin/env bash
# version-sync.test.sh — one version, in three files, that must agree.
#
# Annex's version lives in three places and they had drifted to three
# different values: the Cargo workspace said `0.0.1`,
# `crates/annex-desktop/tauri.conf.json` said `0.0.1`, and
# `client/package.json` said `0.0.0`.
#
# That is not cosmetic. `tauri.conf.json::version` is what an installer
# advertises to the OS and what the updater compares to decide whether a user
# is out of date; `CARGO_PKG_VERSION` is what `GET /health` reports and what an
# operator reads when asking which build is running. Two of those disagreeing
# means a bug report names a version that was never released.
#
# Run: bash scripts/tests/version-sync.test.sh

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"

VS_OK=0
VS_BAD=0
vs_ok()  { echo "[version] OK   $1"; VS_OK=$((VS_OK + 1)); }
vs_bad() { echo "[version] FAIL $1" >&2; VS_BAD=$((VS_BAD + 1)); }

# Cargo workspace: the `version = "..."` under [workspace.package].
cargo_version=$(
  sed -n '/^\[workspace\.package\]/,/^\[/p' "${ROOT_DIR}/Cargo.toml" |
    sed -n 's/^version *= *"\([^"]*\)".*/\1/p' | head -1
)
tauri_version=$(
  sed -n 's/^  *"version" *: *"\([^"]*\)".*/\1/p' \
    "${ROOT_DIR}/crates/annex-desktop/tauri.conf.json" | head -1
)
client_version=$(
  node -e 'process.stdout.write(require(process.argv[1]).version)' \
    "${ROOT_DIR}/client/package.json"
)

for pair in "Cargo.toml:${cargo_version}" "tauri.conf.json:${tauri_version}" "client/package.json:${client_version}"; do
  name="${pair%%:*}"
  value="${pair#*:}"
  if [ -z "${value}" ]; then
    vs_bad "could not read a version out of ${name}"
  else
    vs_ok "${name} = ${value}"
  fi
done

if [ "${cargo_version}" = "${tauri_version}" ] && [ "${cargo_version}" = "${client_version}" ]; then
  vs_ok "all three agree on ${cargo_version}"
else
  vs_bad "versions disagree: Cargo=${cargo_version} tauri=${tauri_version} client=${client_version}"
fi

# Semver, and not a placeholder. `0.0.0` is npm's default for a scaffolded
# package and is what `client/package.json` carried; it means "nobody set
# this", which is a different statement from "this is version zero".
case "${cargo_version}" in
  0.0.0)
    vs_bad "0.0.0 is the scaffold default, not a version"
    ;;
  [0-9]*.[0-9]*.[0-9]*)
    vs_ok "${cargo_version} is a semver triple"
    ;;
  *)
    vs_bad "'${cargo_version}' is not a semver triple"
    ;;
esac

# A tagged release must have a CHANGELOG entry for the version it ships.
CHANGELOG="${ROOT_DIR}/CHANGELOG.md"
if [ ! -f "${CHANGELOG}" ]; then
  vs_bad "no CHANGELOG.md"
elif grep -q "^## \[${cargo_version}\]" "${CHANGELOG}"; then
  vs_ok "CHANGELOG.md has a section for ${cargo_version}"
else
  vs_bad "CHANGELOG.md has no '## [${cargo_version}]' section"
fi

echo "[version] ${VS_OK} passed, ${VS_BAD} failed"
[ "${VS_BAD}" -eq 0 ] || exit 1
