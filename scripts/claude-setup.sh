#!/usr/bin/env bash
# claude-setup.sh — Idempotent environment setup for Claude Code sessions.
# Installs system deps, generates ZK keys, installs npm deps, verifies compilation.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

info()  { echo -e "${GREEN}[setup]${NC} $*"; }
warn()  { echo -e "${YELLOW}[setup]${NC} $*"; }
error() { echo -e "${RED}[setup]${NC} $*"; }

# ---------- 1. System dependencies ----------
if pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
    info "WebKitGTK already installed ($(pkg-config --modversion webkit2gtk-4.1))"
else
    info "Installing system dependencies (WebKitGTK, GTK, PipeWire)..."
    # Package list must stay in sync with .github/workflows/ci.yml's
    # check-desktop-linux job. Note `libjavascriptcoregtk-4.1-dev` — the
    # `lib` prefix is required; `javascriptcoregtk-4.1-dev` does not exist
    # and apt fails the whole transaction on it.
    #
    # Both commands are inside the `if`, and their output goes to a file
    # rather than through `| tail`. `set -e` with `pipefail` aborts AT a
    # failing pipeline, so a status check placed after one never runs: the
    # error below was unreachable, and a failed install ended setup with a
    # couple of lines of apt noise and no explanation of what went wrong.
    # Conditions are exempt from `set -e`, which is what makes it reachable.
    # On failure the log is shown far less truncated, because the reason apt
    # gives is usually further up than three lines.
    apt_log=$(mktemp)
    if apt-get update -qq >"$apt_log" 2>&1 &&
       apt-get install -y --no-install-recommends \
        libwebkit2gtk-4.1-dev \
        libappindicator3-dev \
        librsvg2-dev \
        patchelf \
        libgtk-3-dev \
        libsoup-3.0-dev \
        libjavascriptcoregtk-4.1-dev \
        libpipewire-0.3-dev \
        espeak-ng \
        >>"$apt_log" 2>&1
    then
        tail -3 "$apt_log"
        rm -f "$apt_log"
        info "System dependencies installed."
    else
        tail -25 "$apt_log"
        rm -f "$apt_log"
        error "System dependency install failed — annex-desktop will not build."
        exit 1
    fi
fi

# espeak-ng is the System-TTS backend the built-in "default" voice profile
# falls back to when Piper isn't provisioned, so agent voice works out of the
# box. Install it even when the WebKit deps were already present.
if ! command -v espeak-ng >/dev/null 2>&1; then
    info "Installing espeak-ng (System TTS fallback for agent voice)..."
    apt-get install -y --no-install-recommends espeak-ng 2>&1 | tail -2 || \
        warn "espeak-ng install failed; agent voice needs Piper or espeak-ng to synthesize."
fi

# ---------- 2. ZK keys ----------
if [ -f "zk/keys/membership_vkey.json" ]; then
    info "ZK keys already exist."
else
    info "Generating ZK keys..."
    if [ -f "zk/bin/circom" ] && command -v node >/dev/null 2>&1; then
        # Chained into the condition, not run as three statements above it.
        # Dummy vkeys are the documented fallback here — the tests carry
        # `generate_dummy_vkey()` for exactly this — but as separate
        # statements under `set -e` any one of them failing ended setup
        # outright, so the warning below only ever printed in the odd case
        # where all three succeeded and produced no key.
        if (cd zk && npm ci --prefer-offline 2>&1 | tail -2) &&
           (cd zk && node scripts/build-circuits.js 2>&1 | tail -4) &&
           (cd zk && node scripts/setup-groth16.js 2>&1 | tail -4) &&
           [ -f "zk/keys/membership_vkey.json" ]; then
            info "ZK keys generated successfully."
        else
            warn "ZK key generation failed. Tests will use dummy vkeys."
        fi
    else
        warn "circom or node not available. Tests will use dummy vkeys."
    fi
fi

# ---------- 3. Asset stub directories ----------
mkdir -p assets/piper assets/voices assets/embedding
info "Asset directories ready."

# ---------- 3b. VRP alignment model ----------
#
# Fetched here rather than left to the developer, because without it a local
# server falls back to the lexicon scorer and the only sign is `lexicon-v1` in a
# handshake. `crates/annex-vrp/tests/alignment_calibration.rs` and
# `wordpiece_vectors.rs` both SKIP when it is absent, so a missing model shows
# up as tests that pass while measuring nothing.
#
# Failure is a warning, not an exit: huggingface.co is not reachable from every
# environment this script runs in, and the dev fallback is a working scorer.
if [ -f "assets/embedding/model.safetensors" ]; then
    info "VRP alignment model already present."
else
    info "Fetching VRP alignment model (7.5 MB, digest-pinned)..."
    if bash scripts/setup-embedding-model.sh 2>&1 | tail -3; then
        info "VRP alignment model ready."
    else
        warn "VRP alignment model fetch failed; alignment will use the lexicon fallback \
and the calibration tests will skip."
    fi
fi

# ---------- 3b. Whisper GGML model (opt-in) ----------
#
# NOT fetched by default, unlike the alignment model above. `ggml-base.en.bin`
# is 141 MB — nineteen times the alignment model — for a feature (live call
# captions) most sessions never exercise. An environment that wants it sets
# ANNEX_FETCH_STT_MODEL=1.
#
# The cost of leaving it out is visible rather than silent:
# `/api/voice/config-status` reports `stt_ready: false` with an `stt_detail`
# naming the missing file, and the call UI says captions are unavailable.
if [ "${ANNEX_FETCH_STT_MODEL:-0}" = "1" ]; then
    if bash scripts/setup-stt.sh 2>&1 | tail -3; then
        info "Whisper STT model ready."
    else
        warn "Whisper STT model fetch failed; live captions will be unavailable."
    fi
elif [ -f "assets/models/ggml-base.en.bin" ] || [ -f "assets/models/ggml-tiny.en.bin" ]; then
    info "Whisper STT model already present."
else
    info "No Whisper STT model (live captions inert). \
Run scripts/setup-stt.sh, or set ANNEX_FETCH_STT_MODEL=1 to fetch it here."
fi

# ---------- 4. Frontend npm deps ----------
if [ -d "client/node_modules" ]; then
    info "Frontend node_modules already installed."
else
    info "Installing frontend dependencies..."
    # In a condition for the same reason as the apt block: a bare pipeline
    # under `set -e` aborts with whatever three lines npm happened to print
    # last and no statement of what failed. There is no fallback for this
    # one — nothing in the frontend runs without it — so it still exits.
    if (cd client && npm ci --prefer-offline 2>&1 | tail -3); then
        info "Frontend dependencies installed."
    else
        error "Frontend dependency install failed — the client cannot build or test."
        exit 1
    fi
fi

# ---------- 5. Rust compilation check ----------
info "Checking Rust compilation..."
if cargo check --workspace --exclude annex-desktop 2>&1 | tail -3; then
    info "Rust workspace compiles successfully."
else
    warn "Rust compilation had issues. Run 'cargo check --workspace --exclude annex-desktop' for details."
fi

# ---------- Summary ----------
echo ""
info "=== Environment Ready ==="
info "Run tests:  bash scripts/test-all.sh"
info "Quick test: bash scripts/test-all.sh --quick"
info "Rust only:  cargo test --workspace --exclude annex-desktop"
info "Frontend:   cd client && npm test"
