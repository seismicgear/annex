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
    apt-get update -qq 2>/dev/null
    # Package list must stay in sync with .github/workflows/ci.yml's
    # check-desktop-linux job. Note `libjavascriptcoregtk-4.1-dev` — the
    # `lib` prefix is required; `javascriptcoregtk-4.1-dev` does not exist
    # and apt fails the whole transaction on it.
    #
    # `PIPESTATUS` is checked explicitly because `| tail` would otherwise
    # mask a non-zero apt-get exit under `set -o pipefail`'s sibling rules:
    # the pipeline status is tail's, so a failed install looked successful.
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
        2>&1 | tail -3
    if [ "${PIPESTATUS[0]}" -ne 0 ]; then
        error "System dependency install failed — annex-desktop will not build."
        exit 1
    fi
    info "System dependencies installed."
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
        (cd zk && npm ci --prefer-offline 2>&1 | tail -2)
        (cd zk && node scripts/build-circuits.js 2>&1 | tail -4)
        (cd zk && node scripts/setup-groth16.js 2>&1 | tail -4)
        if [ -f "zk/keys/membership_vkey.json" ]; then
            info "ZK keys generated successfully."
        else
            warn "ZK key generation failed. Tests will use dummy vkeys."
        fi
    else
        warn "circom or node not available. Tests will use dummy vkeys."
    fi
fi

# ---------- 3. Asset stub directories ----------
mkdir -p assets/piper assets/voices
info "Asset directories ready."

# ---------- 4. Frontend npm deps ----------
if [ -d "client/node_modules" ]; then
    info "Frontend node_modules already installed."
else
    info "Installing frontend dependencies..."
    (cd client && npm ci --prefer-offline 2>&1 | tail -3)
    info "Frontend dependencies installed."
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
