# Annex server multi-stage Docker build.
#
# Stage 1: Build Rust server binary
# Stage 2: Compile ZK circuits and generate proving/verification keys
# Stage 3: Download Piper TTS binary and default voice model
# Stage 4: Build client static files (consumes ZK wasm/zkey)
# Stage 5: Minimal runtime image

# ── Build server ──
FROM rust:1.85-slim-bookworm AS server-builder

WORKDIR /build
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
FROM node:22-slim AS zk-builder

WORKDIR /build/zk
COPY zk/ ./
RUN npm ci
RUN node scripts/build-circuits.js
RUN node scripts/setup-groth16.js

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

# ── Download whisper.cpp binary ──
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

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates sqlite3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Server binary
COPY --from=server-builder /build/target/release/annex-server /app/annex-server

# Client static files
COPY --from=client-builder /build/client/dist /app/client/dist

# ZK verification keys
COPY --from=zk-builder /build/zk/keys/membership_vkey.json /app/zk/keys/
COPY --from=zk-builder /build/zk/keys/identity_vkey.json /app/zk/keys/

# Piper TTS binary and libraries
COPY --from=piper-downloader /piper/ /app/assets/piper/

# Default voice model
COPY --from=piper-downloader /voices/ /app/assets/voices/

# Whisper STT binary
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
ENV ANNEX_STT_MODEL_PATH=/app/assets/models/ggml-base.en.bin
ENV ANNEX_CORS_ORIGINS=*

EXPOSE 3000

# Run as non-root user
USER annex

ENTRYPOINT ["/app/docker-entrypoint.sh"]
