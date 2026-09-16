#!/usr/bin/env bash
# prune-stale-test-binaries.sh — reclaim target/debug/deps.
#
# Cargo never removes the previous hash of a rebuilt test binary, and every one
# of this workspace's ~121 test binaries statically links webrtc-rs, arkworks,
# axum and tokio. At ~66 MB each (after `[profile.dev.package."*"] debug = 0`
# in the root Cargo.toml), a few dozen orphans is several gigabytes — and on a
# fixed disk allowance that is the difference between a full
# `cargo test --workspace` and a link step dying with "No space left on device",
# which reads as a broken machine rather than a full one.
#
# Keeps the newest binary per target name. Safe to run at any time: anything it
# removes, cargo rebuilds.
#
# Usage: bash scripts/prune-stale-test-binaries.sh [--dry-run]
set -uo pipefail

DEPS="$(cd "$(dirname "$0")/.." && pwd)/target/debug/deps"
[ -d "$DEPS" ] || { echo "[prune] no $DEPS — nothing to do"; exit 0; }

DRY=0
[ "${1:-}" = "--dry-run" ] && DRY=1

cd "$DEPS" || exit 0
freed=0
removed=0
for base in $(find . -maxdepth 1 -type f -executable ! -name '*.so' -printf '%f\n' \
              | sed -E 's/-[0-9a-f]{16}$//' | sort -u); do
  # `ls -t` newest first; everything after the first is an orphan.
  mapfile -t files < <(ls -t "${base}"-???????????????? 2>/dev/null)
  for old in "${files[@]:1}"; do
    sz=$(stat -c %s "$old" 2>/dev/null || echo 0)
    freed=$((freed + sz))
    removed=$((removed + 1))
    [ "$DRY" -eq 1 ] || rm -f "$old"
  done
done

verb="pruned"
[ "$DRY" -eq 1 ] && verb="would prune"
printf '[prune] %s %d stale test binaries, %.1f GB\n' \
  "$verb" "$removed" "$(awk "BEGIN{print $freed/1073741824}")"
