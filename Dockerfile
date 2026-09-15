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
# `--all`, then the ceremony, then INSTALL the verified bytes.
#
# This stage used to run `verify-artifacts.js` with no arguments — a single
# manifest — and stop there. Three gaps, all of which the desktop path had
# already closed:
#
#   * `--all` covers every circuit's manifest, not just membership. Without it
#     a tampered channel-eligibility or federation-attestation key passes.
#   * `verify-ceremony.js` is what checks the artifacts descend from the
#     recorded trusted setup and that its beacon is the published one. The
#     desktop release runs it; Docker did not, so the container's guarantee was
#     strictly weaker than the installer's for the same commit.
#   * `install-ceremony.js` copies the VERIFIED bytes from `zk/artifacts/` into
#     `zk/keys/`, re-hashing each one as it lands. Without it the image shipped
#     whatever happened to be in `zk/keys` — which, in a developer's checkout,
#     is the random-entropy dev fixtures, and which in a clean checkout is
#     nothing at all. Verifying one set of files and shipping another is not a
#     chain of custody.
#
# `--offline` is NOT used: a production image build that cannot reach drand
# should fail rather than quietly ship an unverified beacon.
RUN set -e; \
    case "${ANNEX_BUILD_PROFILE}" in \
      production|release) \
        echo "[docker zk-builder] profile=${ANNEX_BUILD_PROFILE}: verifying pinned ZK artifacts (no fixture generation)"; \
        node scripts/verify-artifacts.js --all; \
        node scripts/verify-ceremony.js; \
        node scripts/install-ceremony.js; \
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

# Re-declared: a global ARG is not in scope inside a stage until the stage
# names it. Without this line the production check below reads an empty string
# and silently never fires — a gate that cannot trigger.
ARG ANNEX_BUILD_PROFILE

RUN apt-get update && apt-get install -y --no-install-recommends \
    curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /piper

# Download Piper binary (linux x86_64), checksum-verified under production.
#
# Version-pinned is not the same as verified: a release asset can be replaced
# under the same tag, and this binary is executed by the server. The digest is
# what makes the pin mean something.
#
# `PIPER_SHA256` is deliberately EMPTY here rather than carrying a value
# nobody measured. This build environment cannot reach GitHub release assets,
# so a digest written here would have been invented — which is worse than no
# pin, because it looks like verification. The build prints the digest it
# measured and refuses to continue under a production profile until someone
# pins it, so the value arrives from a real download rather than from prose.
#
# It has now been measured, and the default below is that measurement:
# `piper_linux_x86_64.tar.gz` for 2023.11.14-2 downloaded from the GitHub
# release and hashed. Leaving it empty meant the DOCUMENTED production command
# (`docker compose -f docker-compose.prod.yml up`, which passes only
# ANNEX_BUILD_PROFILE) could not build at all — the safeguard was correct and
# nothing satisfied it. A pin nobody supplies is a build break, not a gate.
# Override with --build-arg to move to a different release.
ARG PIPER_VERSION=2023.11.14-2
ARG PIPER_SHA256=a50cb45f355b7af1f6d758c1b360717877ba0a398cc8cbe6d2a7a3a26e225992
RUN curl -fSL "https://github.com/rhasspy/piper/releases/download/${PIPER_VERSION}/piper_linux_x86_64.tar.gz" \
    -o piper.tar.gz \
    && actual="$(sha256sum piper.tar.gz | cut -d' ' -f1)" \
    && echo "piper_linux_x86_64.tar.gz sha256 = ${actual}" \
    && if [ -z "${PIPER_SHA256}" ]; then \
         if [ "${ANNEX_BUILD_PROFILE}" = "production" ] || [ "${ANNEX_BUILD_PROFILE}" = "release" ]; then \
           echo "PIPER_SHA256 is unset. A production image will not execute a binary" >&2; \
           echo "it has not verified. Pin it with --build-arg PIPER_SHA256=${actual}" >&2; \
           echo "after confirming that digest against the upstream release." >&2; \
           exit 1; \
         fi; \
         echo "WARNING PIPER_SHA256 unset — not verifying (dev build)." >&2; \
       elif [ "${actual}" != "${PIPER_SHA256}" ]; then \
         echo "piper_linux_x86_64.tar.gz digest mismatch" >&2; \
         echo "  expected ${PIPER_SHA256}" >&2; \
         echo "  actual   ${actual}" >&2; \
         exit 1; \
       fi \
    && tar -xzf piper.tar.gz --strip-components=1 \
    && rm piper.tar.gz \
    && chmod +x piper

# Download en_US-lessac-medium voice model
WORKDIR /voices
# The default voice model, pinned to a revision and verified.
#
# This fetched from `resolve/main`, which is a moving target: the same
# Dockerfile built a week apart could bundle different model weights, and
# nothing downstream would notice. Two images claiming the same version would
# then speak with different voices.
#
# Pinned to a commit on the model repo, with both files' digests checked. The
# revision and digests were obtained by fetching them, not transcribed.
ARG PIPER_VOICE_REVISION=1162a9173d0ce503555aed757976b7a9912eae4c
ARG PIPER_VOICE_ONNX_SHA256=5efe09e69902187827af646e1a6e9d269dee769f9877d17b16b1b46eeaaf019f
ARG PIPER_VOICE_JSON_SHA256=efe19c417bed055f2d69908248c6ba650fa135bc868b0e6abb3da181dab690a0
RUN set -e; \
    base="https://huggingface.co/rhasspy/piper-voices/resolve/${PIPER_VOICE_REVISION}/en/en_US/lessac/medium"; \
    curl -fSL "${base}/en_US-lessac-medium.onnx" -o en_US-lessac-medium.onnx; \
    curl -fSL "${base}/en_US-lessac-medium.onnx.json" -o en_US-lessac-medium.onnx.json; \
    for pair in "en_US-lessac-medium.onnx ${PIPER_VOICE_ONNX_SHA256}" \
                "en_US-lessac-medium.onnx.json ${PIPER_VOICE_JSON_SHA256}"; do \
      set -- ${pair}; \
      actual="$(sha256sum "$1" | cut -c1-64)"; \
      if [ "${actual}" != "$2" ]; then \
        echo "voice model $1 digest mismatch" >&2; \
        echo "  expected $2" >&2; \
        echo "  actual   ${actual}" >&2; \
        exit 1; \
      fi; \
      echo "voice model $1 sha256 verified"; \
    done

# ── Build whisper.cpp binary ──
#
# We build the binary so STT *can* run if an operator mounts a GGML model.
# We deliberately DO NOT download a GGML model into the image: the previous
# default of `ANNEX_STT_MODEL_PATH=/app/assets/models/ggml-base.en.bin`
# pointed at a file that was never copied in, so STT calls failed at request
# time while the config implied STT was ready. Now the operator must opt in
# by mounting a model and setting ANNEX_STT_MODEL_PATH.
FROM debian:bookworm-slim AS whisper-builder

# Re-declared for the same reason as in `piper-downloader`.
ARG ANNEX_BUILD_PROFILE

RUN apt-get update && apt-get install -y --no-install-recommends \
    curl ca-certificates git cmake build-essential \
    && rm -rf /var/lib/apt/lists/*

# Pinned to a tag, and under production verified against a commit SHA.
#
# This was `git clone --depth 1` of `master`. Two images built a week apart
# contained different, unreviewed C++ compiled into the thing that processes
# audio a user uploads — and neither build recorded which. A tag alone is not
# enough either: a tag can be moved, and `rev-parse` is what turns a
# repeatable build into a reproducible one.
#
# Measured, like PIPER_SHA256 above: `git ls-remote https://github.com/
# ggerganov/whisper.cpp refs/tags/v1.7.4` resolves to the commit pinned below.
# The original note follows, and still explains why a value must never be
# written here from prose.
#
# `WHISPER_CPP_COMMIT` was empty for the same reason `PIPER_SHA256` was: it is
# not knowable from this environment, and a guessed SHA would fail every build
# while looking authoritative. The build prints what the tag resolved to.
WORKDIR /whisper
ARG WHISPER_CPP_TAG=v1.7.4
ARG WHISPER_CPP_COMMIT=8a9ad7844d6e2a10cddf4b92de4089d7ac2b14a9
RUN git clone --depth 1 --branch "${WHISPER_CPP_TAG}" \
        https://github.com/ggerganov/whisper.cpp.git /tmp/whisper && \
    cd /tmp/whisper && \
    actual="$(git rev-parse HEAD)" && \
    echo "whisper.cpp ${WHISPER_CPP_TAG} = ${actual}" && \
    if [ -z "${WHISPER_CPP_COMMIT}" ]; then \
        if [ "${ANNEX_BUILD_PROFILE}" = "production" ] || [ "${ANNEX_BUILD_PROFILE}" = "release" ]; then \
            echo "WHISPER_CPP_COMMIT is unset. A production image will not compile a tree" >&2; \
            echo "it has not pinned — a tag can be moved. Pin it with" >&2; \
            echo "--build-arg WHISPER_CPP_COMMIT=${actual} after checking that commit." >&2; \
            exit 1; \
        fi; \
        echo "WARNING WHISPER_CPP_COMMIT unset — building an unpinned tag (dev build)." >&2; \
    elif [ "${actual}" != "${WHISPER_CPP_COMMIT}" ]; then \
        echo "whisper.cpp ${WHISPER_CPP_TAG} resolved to ${actual}, expected ${WHISPER_CPP_COMMIT}." >&2; \
        echo "The tag moved, or the pin is stale. Refusing to compile an unreviewed tree." >&2; \
        exit 1; \
    fi && \
    cmake -B build && cmake --build build --config Release && \
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

# `curl` is here for the HEALTHCHECK below and nothing else. It is a real, if
# small, addition to the runtime surface; the alternative is a container that
# reports "running" while unable to serve a request, which is the failure this
# image had. `sqlite3` is already present for the entrypoint's seeding and for
# `scripts/backup.sh`.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates sqlite3 gosu curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Server binary
COPY --from=server-builder /build/target/release/annex-server /app/annex-server

# Client static files
COPY --from=client-builder /build/client/dist /app/client/dist

# ZK verification keys — EVERY circuit the server can be asked to verify.
#
# This copied `membership_vkey.json` alone, on the reasoning that it is the
# only key the runtime needs. That was wrong in two ways, and the first one
# meant the default image could not start at all:
#
#   * `SecurityConfig` defaults `enabled_zk_versions` to BOTH `v1` and `v2`,
#     and under the default `enforce_zk_proofs = true` startup refuses to run
#     without the v2 verifying key. So a `docker compose -f
#     docker-compose.prod.yml up` — the documented ordinary command — built an
#     image that aborted on boot, and the only way past it was to disable v2 or
#     relax enforcement, i.e. to weaken the thing the image exists to enforce.
#   * The capability circuits are treated as OPTIONAL by the loader, so their
#     absence disables those endpoints rather than stopping the server. That is
#     worse than a hard failure, not better: the container comes up healthy and
#     proof-backed features answer "not configured" forever, with nothing in
#     the logs connecting that to a packaging decision.
#
# The whole directory is copied so that adding a circuit does not silently
# require remembering this line.
COPY --from=zk-builder /build/zk/keys/ /app/zk/keys/

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

# The operator scripts travel with the image.
#
# `docker-compose.prod.yml` runs `scripts/backup.sh` from a sidecar against the
# same volume, and an operator recovering from a bad upgrade needs
# `scripts/restore.sh` inside the container where the data is. Shipping the
# runbook's tools separately from the runtime is how a recovery procedure ends
# up depending on a checkout nobody has at 3am.
COPY scripts/backup.sh scripts/restore.sh /app/scripts/
RUN chmod +x /app/scripts/backup.sh /app/scripts/restore.sh
RUN sed -i 's/\r$//' /app/docker-entrypoint.sh && chmod +x /app/docker-entrypoint.sh

# Create non-root user for runtime
RUN groupadd --system annex && useradd --system --gid annex --no-create-home annex

# Create data directory for SQLite (owned by runtime user)
RUN mkdir -p /app/data && chown annex:annex /app/data

ENV ANNEX_CONFIG_PATH=/app/config.toml
ENV ANNEX_ZK_KEY_PATH=/app/zk/keys/membership_vkey.json
# v2 is enabled by default and startup refuses to run without its key under
# enforcement, so this is not optional configuration — it is the difference
# between an image that boots and one that does not.
ENV ANNEX_ZK_KEY_PATH_V2=/app/zk/keys/membership_v2_vkey.json
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
# The container is only healthy when it can actually serve a request.
#
# There was no HEALTHCHECK at all, so Docker and Compose reported "running" for
# a container whose database was unreachable — and `/health` would have agreed,
# because it returned a literal. `/readyz` takes a pooled connection, queries,
# and reads the storage gate and the Merkle tree.
#
# `start-period` is generous: first boot runs migrations and, on a fresh
# volume, generates a signing key.
HEALTHCHECK --interval=30s --timeout=5s --start-period=90s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${ANNEX_PORT:-3000}/readyz" >/dev/null || exit 1

ENTRYPOINT ["/app/docker-entrypoint.sh"]
