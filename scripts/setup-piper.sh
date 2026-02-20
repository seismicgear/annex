#!/usr/bin/env bash
# Downloads the Piper TTS binary and the en_US-lessac-medium voice model.
#
# Usage:
#   ./scripts/setup-piper.sh
#
# This creates:
#   assets/piper/piper          — the Piper binary
#   assets/voices/*.onnx        — voice model
#   assets/voices/*.onnx.json   — voice model config

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

PIPER_VERSION="2023.11.14-2"
VOICE_MODEL="en_US-lessac-medium"
VOICE_BASE_URL="https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium"

PIPER_DIR="$PROJECT_ROOT/assets/piper"
VOICES_DIR="$PROJECT_ROOT/assets/voices"

# ── Helpers ──

info()  { echo ":: $1"; }
ok()    { echo "   OK: $1"; }
fail()  { echo "   FAIL: $1" >&2; exit 1; }

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "$1 is required but not found"
}

# ── Detect platform ──

detect_platform() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)  os="linux" ;;
        Darwin) os="macos" ;;
        *)      fail "Unsupported OS: $os" ;;
    esac

    case "$arch" in
        x86_64|amd64)   arch="x86_64" ;;
        aarch64|arm64)  arch="aarch64" ;;
        *)              fail "Unsupported architecture: $arch" ;;
    esac

    echo "${os}_${arch}"
}

# ── Download Piper binary ──

setup_piper_binary() {
    if [ -x "$PIPER_DIR/piper" ]; then
        ok "Piper binary already exists at $PIPER_DIR/piper"
        return
    fi

    require_cmd curl
    require_cmd tar

    local platform
    platform="$(detect_platform)"

    local tarball_name="piper_${platform}.tar.gz"
    local url="https://github.com/rhasspy/piper/releases/download/${PIPER_VERSION}/${tarball_name}"

    info "Downloading Piper binary ($platform)..."
    mkdir -p "$PIPER_DIR"

    local tmp_tar
    tmp_tar="$(mktemp)"
    curl -fSL --retry 3 "$url" -o "$tmp_tar" || fail "Failed to download Piper from $url"

    tar -xzf "$tmp_tar" -C "$PIPER_DIR" --strip-components=1
    rm -f "$tmp_tar"

    chmod +x "$PIPER_DIR/piper"
    ok "Piper binary installed to $PIPER_DIR/piper"
}

# ── Download voice model ──

setup_voice_model() {
    local onnx_file="$VOICES_DIR/${VOICE_MODEL}.onnx"
    local json_file="$VOICES_DIR/${VOICE_MODEL}.onnx.json"

    if [ -f "$onnx_file" ] && [ -f "$json_file" ]; then
        ok "Voice model ${VOICE_MODEL} already exists"
        return
    fi

    require_cmd curl

    info "Downloading voice model: ${VOICE_MODEL}..."
    mkdir -p "$VOICES_DIR"

    if [ ! -f "$onnx_file" ]; then
        curl -fSL --retry 3 "${VOICE_BASE_URL}/${VOICE_MODEL}.onnx" -o "$onnx_file" \
            || fail "Failed to download ${VOICE_MODEL}.onnx"
    fi

    if [ ! -f "$json_file" ]; then
        curl -fSL --retry 3 "${VOICE_BASE_URL}/${VOICE_MODEL}.onnx.json" -o "$json_file" \
            || fail "Failed to download ${VOICE_MODEL}.onnx.json"
    fi

    ok "Voice model installed to $VOICES_DIR/"
}

# ── Main ──

info "Setting up Piper TTS for Annex"
echo ""
setup_piper_binary
setup_voice_model
echo ""
ok "Piper TTS setup complete"
echo "   Binary:  $PIPER_DIR/piper"
echo "   Model:   $VOICES_DIR/${VOICE_MODEL}.onnx"
echo "   Config:  $VOICES_DIR/${VOICE_MODEL}.onnx.json"
