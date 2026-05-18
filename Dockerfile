# Annex server multi-stage Docker build.
#
# Stage 1: Build Rust server binary
# Stage 2: Prepare ZK proving/verification artifacts (profile-aware)
# Stage 3: Download Piper TTS binary and default voice model
# Stage 4: Build whisper.cpp STT binary (no model file is bundled by default)
# Stage 5: Build client static files (consumes ZK wasm/zkey)
# Stage 6: Minimal runtime image
#
# Public endpoint provisioning is handled at the application level
# (Annex router for desktop, or a standard reverse proxy for production).
# No tunnel binary is bundled in the Docker image.
#
# ── Build profiles ─────────────────────────────────────────────────────────
# This Dockerfile is profile-aware. The build-time argument
# `ANNEX_BUILD_PROFILE` selects between two ZK code paths:
#
#   ANNEX_BUILD_PROFILE=production (DEFAULT)
#     - DOES NOT generate Groth16 artifacts inside the image.
#     - Calls `node zk/scripts/verify-artifacts.js` against the pinned
#       manifest at zk/artifacts/membership/manifest.json. The script
#       refuses dev-fixture ceremony metadata under a production profile
#       and exits non-zero, which fails the Docker build. This is the
#       intended behaviour until real multi-party ceremony artifacts
#       (membership.wasm, membership_final.zkey, membership_vkey.json)
#       are produced and shipped in the build context — see
#       docs/refactor/zk-merkle-production.md.
#     - The build context MUST contain the ceremony artifacts at the
#       paths declared in the manifest. They are gitignored on purpose,
#       so producers ship them out-of-band.
#
#   ANNEX_BUILD_PROFILE=dev
#     - Compiles the circuits with circom inside the image and runs
#       `node zk/scripts/dev-setup-groth16.js`, which generates Groth16
#       keys from random entropy on a single machine. Useful for local
#       Docker iteration; NEVER use this image as a production release.
#
# Usage:
#   # Production (default) — fails until real ceremony artifacts exist:
#   docker build -t annex:prod .
#
#   # Dev / local — explicitly opts in to random-entropy keys:
#   docker build --build-arg ANNEX_BUILD_PROFILE=dev -t annex:dev .
# ───────────────────────────────────────────────────────────────────────────

# `production` is the safe default: it refuses dev-fixture artifacts and
# never generates fresh ones inside the image. Override at build time
# (`--build-arg ANNEX_BUILD_PROFILE=dev`) to take the dev path.
ARG ANNEX_BUILD_PROFILE=production

# ── Build server ──
# Rust toolchain version is pinned to match rust-toolchain.toml and the
# `dtolnay/rust-toolchain@1.88` step in .github/workflows/release-desktop.yml.
# Drift between CI and Docker has bitten us before — change all three together
# or not at all.
FROM rust:1.88-slim-bookworm AS server-builder

WORKDIR /build
COPY rust-toolchain.toml ./
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

# Install build dependencies for openssl-sys
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Build release binary
RUN cargo build --release --bin annex-server \
    && strip target/release/annex-server

# ── Build ZK artifacts ──
#
# In production: verifies pinned artifacts only; fails on dev-fixture or
# missing files. In dev: compiles circuits and generates fresh random-
# entropy keys. The branching keeps the production image off the dev path
# entirely — there is no code path here that would silently ship dev
# fixtures under a production tag.
FROM node:22-slim AS zk-builder

ARG ANNEX_BUILD_PROFILE
ENV ANNEX_BUILD_PROFILE=${ANNEX_BUILD_PROFILE}

WORKDIR /build/zk
COPY zk/ ./
RUN npm ci

# Profile branch.
#
# production / release: verify pinned artifacts; refuse dev-fixture and
# missing files. Exits non-zero on any failure — the Docker build fails
# fast and obviously, which is the whole point of this gate.
#
# dev: compile circuits with circom, then run dev-setup-groth16.js to
# produce random-entropy keys. dev-setup-groth16.js itself refuses to run
# when ANNEX_BUILD_PROFILE=production, so even if the branch logic ever
# regresses, the inner script catches it.
RUN set -e; \
    case "${ANNEX_BUILD_PROFILE}" in \
      production|release) \
        echo "[docker zk-builder] profile=${ANNEX_BUILD_PROFILE}: verifying pinned ZK artifacts (no fixture generation)"; \
        node scripts/verify-artifacts.js; \
        ;; \
      dev|development|"") \
        echo "[docker zk-builder] profile=${ANNEX_BUILD_PROFILE:-dev}: generating dev-fixture Groth16 keys (DEV ONLY)"; \
        node scripts/build-circuits.js; \
        node scripts/dev-setup-groth16.js; \
        ;; \
      *) \
        echo "[docker zk-builder] unrecognised ANNEX_BUILD_PROFILE='${ANNEX_BUILD_PROFILE}' — use 'production' or 'dev'" >&2; \
        exit 1; \
        ;; \
    esac

# ── Download Piper TTS + default voice model ──
FROM debian:bookworm-slim AS piper-downloader

RUN apt-get update && apt-get install -y --no-install-recommends \
    curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /piper

# Download Piper binary (linux x86_64)
ARG PIPER_VERSION=2023.11.14-2
RUN curl -fSL "https://github.com/rhasspy/piper/releases/download/${PIPER_VERSION}/piper_linux_x86_64.tar.gz" \
    -o piper.tar.gz \
    && tar -xzf piper.tar.gz --strip-components=1 \
    && rm piper.tar.gz \
    && chmod +x piper

# Download en_US-lessac-medium voice model
WORKDIR /voices
RUN curl -fSL "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx" \
    -o en_US-lessac-medium.onnx \
    && curl -fSL "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json" \
    -o en_US-lessac-medium.onnx.json

# ── Build whisper.cpp binary ──
#
# We build the binary so STT *can* run if an operator mounts a GGML model.
# We deliberately DO NOT download a GGML model into the image: the previous
# default of `ANNEX_STT_MODEL_PATH=/app/assets/models/ggml-base.en.bin`
# pointed at a file that was never copied in, so STT calls failed at request
# time while the config implied STT was ready. Now the operator must opt in
# by mounting a model and setting ANNEX_STT_MODEL_PATH.
FROM debian:bookworm-slim AS whisper-builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    curl ca-certificates git cmake build-essential \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /whisper
RUN git clone --depth 1 https://github.com/ggerganov/whisper.cpp.git /tmp/whisper && \
    cd /tmp/whisper && cmake -B build && cmake --build build --config Release && \
    mkdir -p /whisper/bin && \
    cp /tmp/whisper/build/bin/main /whisper/bin/whisper && \
    rm -rf /tmp/whisper

# ── Build client ──
FROM node:22-slim AS client-builder

WORKDIR /build/client
COPY client/package.json client/package-lock.json ./
RUN npm ci

COPY client/ ./
COPY --from=zk-builder /build/zk/build/membership_js/membership.wasm public/zk/
COPY --from=zk-builder /build/zk/keys/membership_final.zkey public/zk/
RUN npm run build

# ── Runtime ──
FROM debian:bookworm-slim

ARG ANNEX_BUILD_PROFILE
ENV ANNEX_BUILD_PROFILE=${ANNEX_BUILD_PROFILE}

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates sqlite3 gosu \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Server binary
COPY --from=server-builder /build/target/release/annex-server /app/annex-server

# Client static files
COPY --from=client-builder /build/client/dist /app/client/dist

# ZK verification keys.
#
# Only the membership vkey is used at runtime — the server's startup
# loads it via ANNEX_ZK_KEY_PATH and verifies every channel-access proof
# against it. The identity vkey is exercised only by test fixtures in
# `crates/annex-identity/tests` and `zk/scripts/test-proofs.js`, not by
# the production server, so it is intentionally NOT copied into the
# runtime image.
COPY --from=zk-builder /build/zk/keys/membership_vkey.json /app/zk/keys/

# Piper TTS binary and libraries
COPY --from=piper-downloader /piper/ /app/assets/piper/

# Default voice model
COPY --from=piper-downloader /voices/ /app/assets/voices/

# Whisper STT binary (no model is bundled — see the whisper-builder stage)
COPY --from=whisper-builder /whisper/bin/whisper /app/assets/whisper/whisper

# Default config
COPY config.toml /app/config.toml

# Entrypoint script (runs migrations + seeds server row on first start)
COPY docker-entrypoint.sh /app/docker-entrypoint.sh
RUN sed -i 's/\r$//' /app/docker-entrypoint.sh && chmod +x /app/docker-entrypoint.sh

# Create non-root user for runtime
RUN groupadd --system annex && useradd --system --gid annex --no-create-home annex

# Create data directory for SQLite (owned by runtime user)
RUN mkdir -p /app/data && chown annex:annex /app/data

ENV ANNEX_CONFIG_PATH=/app/config.toml
ENV ANNEX_ZK_KEY_PATH=/app/zk/keys/membership_vkey.json
ENV ANNEX_DB_PATH=/app/data/annex.db
ENV ANNEX_TTS_BINARY_PATH=/app/assets/piper/piper
ENV ANNEX_TTS_VOICES_DIR=/app/assets/voices
ENV ANNEX_CLIENT_DIR=/app/client/dist
ENV ANNEX_STT_BINARY_PATH=/app/assets/whisper/whisper
# ANNEX_STT_MODEL_PATH is intentionally NOT set. Operators who want STT
# must mount a whisper.cpp GGML model into the container and set
# ANNEX_STT_MODEL_PATH explicitly. Leaving the var unset keeps STT
# inert + visible in /api/voice/config-status rather than silently
# 500-ing on the first transcription attempt.
#
# ANNEX_CORS_ORIGINS is intentionally NOT set. A wildcard CORS default
# in the production image was a footgun — operators must declare their
# real allowed origins (e.g. https://app.example.com). The server
# refuses to start when ANNEX_BUILD_PROFILE=production and the
# resolved CORS policy is wildcard or empty. For local/dev work, set
# ANNEX_BUILD_PROFILE=dev or override ANNEX_CORS_ORIGINS explicitly.

EXPOSE 3000

# The entrypoint starts as root to fix data-volume ownership, then
# drops to the non-root "annex" user via gosu before exec-ing the server.
ENTRYPOINT ["/app/docker-entrypoint.sh"]
