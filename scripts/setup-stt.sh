#!/usr/bin/env bash
# setup-stt.sh — install a whisper.cpp GGML model for live call captions.
#
# Usage:
#   ./scripts/setup-stt.sh                  # fetch base.en if missing, verify always
#   ./scripts/setup-stt.sh --model tiny.en  # smaller and faster, less accurate
#   ./scripts/setup-stt.sh --verify         # verify what is installed, never download
#
# Creates:
#   assets/models/ggml-<model>.bin
#
# and prints the ANNEX_STT_MODEL_PATH line to add to the server's environment.
#
# ── Why this is opt-in rather than bundled ─────────────────────────────────
#
# The container image ships the whisper.cpp BINARY and no model, and that split
# is deliberate: `ggml-base.en.bin` is 141 MiB, which is nineteen times the
# whole rest of the image's model payload, for a feature many deployments do
# not use. An operator who wants captions runs this; one who does not pays
# nothing, and `/api/voice/config-status` reports `stt_ready: false` so the app
# says captions are unavailable instead of showing an empty strip forever.
#
# ── Why the digests are pinned, and where they come from ───────────────────
#
# The model is fed to a binary this server executes. A pinned VERSION is not a
# pinned artifact: `resolve/main` is a moving target and a repository can be
# rewritten under the same branch. The digests below are the git-lfs object ids
# from revision ${WHISPER_REVISION} of `ggerganov/whisper.cpp` — which, by the
# definition of git-lfs, ARE the SHA-256 of the file contents. They were read
# from the LFS pointers rather than invented:
#
#     curl -sS https://huggingface.co/ggerganov/whisper.cpp/raw/<rev>/ggml-base.en.bin
#     version https://git-lfs.github.com/spec/v1
#     oid sha256:a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002
#     size 147964211
#
# A mismatch is fatal. There is no "warn and continue" branch, because the
# thing being verified is an input to a subprocess.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

REPO_ID="ggerganov/whisper.cpp"
WHISPER_REVISION="5359861c739e955e79d9a303bcbc70fb988958b1"
DEST_DIR="$PROJECT_ROOT/assets/models"

# name : sha256 : bytes
MODELS=(
  "tiny.en:921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f:77704715"
  "base.en:a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002:147964211"
)
DEFAULT_MODEL="base.en"

MODEL="$DEFAULT_MODEL"
VERIFY_ONLY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --model) MODEL="${2:-}"; shift 2 ;;
    --model=*) MODEL="${1#--model=}"; shift ;;
    --verify) VERIFY_ONLY=1; shift ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "[setup-stt] unknown argument: $1" >&2; exit 2 ;;
  esac
done

lookup() {
  local want="$1" entry
  for entry in "${MODELS[@]}"; do
    if [ "${entry%%:*}" = "$want" ]; then
      printf '%s' "$entry"
      return 0
    fi
  done
  return 1
}

if ! ENTRY="$(lookup "$MODEL")"; then
  echo "[setup-stt] unknown model '$MODEL'." >&2
  echo "[setup-stt] known models: $(printf '%s ' "${MODELS[@]%%:*}")" >&2
  echo "[setup-stt] Adding another means adding its pinned digest here — read it" >&2
  echo "[setup-stt] from the LFS pointer, do not take it from a download." >&2
  exit 2
fi

rest="${ENTRY#*:}"
EXPECTED_SHA="${rest%%:*}"
EXPECTED_BYTES="${rest##*:}"
FILE_NAME="ggml-${MODEL}.bin"
DEST="$DEST_DIR/$FILE_NAME"

digest_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    echo "[setup-stt] neither sha256sum nor shasum is available; cannot verify." >&2
    exit 1
  fi
}

verify() {
  local path="$1"
  local actual
  actual="$(digest_of "$path")"
  if [ "$actual" != "$EXPECTED_SHA" ]; then
    echo "[setup-stt] DIGEST MISMATCH for $path" >&2
    echo "[setup-stt]   expected $EXPECTED_SHA" >&2
    echo "[setup-stt]   actual   $actual" >&2
    echo "[setup-stt] This file is not the pinned model. It is executed by whisper.cpp" >&2
    echo "[setup-stt] on call audio; refusing to install it." >&2
    return 1
  fi
  echo "[setup-stt] OK  $path"
  echo "[setup-stt]     sha256 $actual"
  return 0
}

mkdir -p "$DEST_DIR"

if [ -f "$DEST" ]; then
  echo "[setup-stt] $FILE_NAME already present; verifying."
  verify "$DEST"
elif [ "$VERIFY_ONLY" -eq 1 ]; then
  echo "[setup-stt] --verify: $DEST is not installed." >&2
  exit 1
else
  URL="https://huggingface.co/${REPO_ID}/resolve/${WHISPER_REVISION}/${FILE_NAME}"
  echo "[setup-stt] downloading $FILE_NAME ($(( EXPECTED_BYTES / 1024 / 1024 )) MiB) from revision ${WHISPER_REVISION:0:12}…"
  # To a temp name first: a half-written file left at the destination by an
  # interrupted download is indistinguishable from a complete one to the
  # `-f "$DEST"` check above, and the next run would "verify" a truncated model
  # and fail with a digest mismatch that reads like tampering.
  TMP="$DEST.part"
  trap 'rm -f "$TMP"' EXIT
  if ! curl -fSL --progress-bar "$URL" -o "$TMP"; then
    echo "[setup-stt] download failed." >&2
    exit 1
  fi
  actual_bytes="$(wc -c < "$TMP" | tr -d ' ')"
  if [ "$actual_bytes" != "$EXPECTED_BYTES" ]; then
    echo "[setup-stt] size mismatch: expected $EXPECTED_BYTES bytes, got $actual_bytes." >&2
    exit 1
  fi
  if ! verify "$TMP"; then
    exit 1
  fi
  mv "$TMP" "$DEST"
  trap - EXIT
  echo "[setup-stt] installed $DEST"
fi

echo
echo "[setup-stt] To turn on live captions, give the server:"
echo
echo "    ANNEX_STT_MODEL_PATH=$DEST"
echo
echo "[setup-stt] The whisper.cpp binary must also be present"
echo "[setup-stt] (ANNEX_STT_BINARY_PATH; the container image ships it at"
echo "[setup-stt] /app/assets/whisper/whisper). GET /api/voice/config-status"
echo "[setup-stt] reports \`stt_ready\` once both are in place, and the client"
echo "[setup-stt] says captions are unavailable until they are."
