#!/usr/bin/env bash
# docker-entrypoint.test.sh — the container must start under the hardening the
# production compose file actually applies.
#
# `docker-compose.prod.yml` sets `cap_drop: ALL`, `read_only: true` and
# `no-new-privileges`. The entrypoint used to start as root, `chown -R` the
# data volume, and drop privileges with `gosu` — all of which need capabilities
# that configuration removes (CAP_CHOWN and CAP_FOWNER for the chown,
# CAP_SETUID/CAP_SETGID for gosu). The documented production compose file could
# not run its own documented entrypoint, and that is a startup contract in
# conflict with itself rather than a missing hardening recommendation.
#
# It also ran the whole server under `timeout 10` and read exit code 124 as
# "migrations succeeded" — elapsed time standing in for a result, in a check
# that could only fail if the server exited early, which is the one thing a
# healthy server does not do.
#
# This drives the REAL entrypoint in a real container under the real flags,
# against a stub binary, so the four cases are cheap enough to run every time:
# a fresh volume, a migration failure, a volume the runtime user cannot write,
# and the init service repairing that last one.
#
# Skips (loudly) when no Docker daemon is reachable.
#
# Run: bash scripts/tests/docker-entrypoint.test.sh

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="annex-entrypoint-contract-test"
VOL_FRESH="annex-ep-test-fresh-$$"
VOL_ROOT="annex-ep-test-rootowned-$$"

DE_OK=0
DE_BAD=0
de_ok()  { echo "[entrypoint] OK   $1"; DE_OK=$((DE_OK + 1)); }
de_bad() { echo "[entrypoint] FAIL $1" >&2; DE_BAD=$((DE_BAD + 1)); }

if ! docker info >/dev/null 2>&1; then
  echo "[entrypoint] SKIP: no Docker daemon reachable."
  echo "[entrypoint] 0 passed, 0 failed"
  exit 0
fi

cleanup() {
  docker volume rm -f "$VOL_FRESH" "$VOL_ROOT" >/dev/null 2>&1 || true
  docker rmi -f "$IMAGE" >/dev/null 2>&1 || true
  rm -rf "$BUILD_DIR"
}
BUILD_DIR="$(mktemp -d)"
trap cleanup EXIT

# The stub stands in for annex-server: it proves the entrypoint CALLS
# `--migrate` and honours its exit code, without needing a built binary or a
# database. What is under test is the entrypoint's contract, not SQLite.
cp "${ROOT_DIR}/docker-entrypoint.sh" "$BUILD_DIR/"
cat > "$BUILD_DIR/Dockerfile" <<'EOF'
FROM debian:bookworm-slim
RUN groupadd --system --gid 10001 annex \
 && useradd --system --uid 10001 --gid annex --no-create-home annex
COPY docker-entrypoint.sh /app/docker-entrypoint.sh
RUN printf '#!/bin/sh\nif [ "$1" = "--migrate" ]; then echo MIGRATE_RAN; exit ${STUB_MIGRATE_EXIT:-0}; fi\necho SERVER_STARTED\nexit 0\n' \
      > /app/annex-server \
 && chmod +x /app/annex-server /app/docker-entrypoint.sh \
 && mkdir -p /app/data && chown annex:annex /app/data
USER annex
ENTRYPOINT ["/app/docker-entrypoint.sh"]
EOF

if ! docker build -q -t "$IMAGE" "$BUILD_DIR" >/dev/null 2>&1; then
  de_bad "could not build the test image"
  echo "[entrypoint] ${DE_OK} passed, ${DE_BAD} failed"
  exit 1
fi

# The production flags, in one place so a test cannot accidentally relax them.
HARDENED=(--rm --cap-drop ALL --read-only --tmpfs /tmp --security-opt no-new-privileges)

# ── 1. A fresh volume, fully hardened ───────────────────────────────────────
out="$(docker run "${HARDENED[@]}" -v "$VOL_FRESH":/app/data "$IMAGE" 2>&1)"
rc=$?
if [ "$rc" -eq 0 ] && printf '%s' "$out" | grep -q SERVER_STARTED; then
  de_ok "starts non-root with ALL capabilities dropped and a read-only root"
else
  de_bad "hardened start failed (exit $rc): $(printf '%s' "$out" | tail -3)"
fi

if printf '%s' "$out" | grep -q MIGRATE_RAN; then
  de_ok "migrations run through an explicit --migrate command"
else
  de_bad "the entrypoint did not invoke --migrate"
fi

# ── 2. A migration failure must stop the container ──────────────────────────
out="$(docker run "${HARDENED[@]}" -e STUB_MIGRATE_EXIT=1 -v "$VOL_FRESH":/app/data "$IMAGE" 2>&1)"
rc=$?
if [ "$rc" -ne 0 ] && ! printf '%s' "$out" | grep -q SERVER_STARTED; then
  de_ok "a failed migration stops the container instead of serving a half-migrated database"
else
  de_bad "the server started after a failed migration (exit $rc)"
fi

# ── 3. A volume the runtime user cannot write ───────────────────────────────
#
# The volume is seeded with a file FIRST. Docker populates an empty named
# volume from the image directory, ownership included, which silently undoes a
# chown and makes this case look like it passes — it did, on the first attempt.
docker run --rm -u 0:0 -v "$VOL_ROOT":/d busybox:1.36 \
  sh -c "touch /d/keep && chown -R 0:0 /d && chmod 0700 /d" >/dev/null 2>&1

out="$(docker run --rm --cap-drop ALL -v "$VOL_ROOT":/app/data "$IMAGE" 2>&1)"
rc=$?
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -qi "not writable"; then
  de_ok "an unwritable data volume is named at startup, not discovered later as a SQLite error"
else
  de_bad "an unwritable volume did not produce a clear refusal (exit $rc): $(printf '%s' "$out" | tail -3)"
fi

# ── 4. The init service repairs exactly that ────────────────────────────────
docker run --rm -u 0:0 --cap-drop ALL --cap-add CHOWN --cap-add FOWNER --cap-add DAC_OVERRIDE \
  --security-opt no-new-privileges -v "$VOL_ROOT":/data busybox:1.36 \
  sh -c "mkdir -p /data && chown -R 10001:10001 /data && chmod 0750 /data" >/dev/null 2>&1

out="$(docker run "${HARDENED[@]}" -v "$VOL_ROOT":/app/data "$IMAGE" 2>&1)"
rc=$?
if [ "$rc" -eq 0 ] && printf '%s' "$out" | grep -q SERVER_STARTED; then
  de_ok "the volume-init service repairs a root-owned volume, with only CHOWN-class capabilities"
else
  de_bad "the entrypoint still failed after the init service ran (exit $rc): $(printf '%s' "$out" | tail -3)"
fi

echo
echo "[entrypoint] ${DE_OK} passed, ${DE_BAD} failed"
[ "$DE_BAD" -eq 0 ]
