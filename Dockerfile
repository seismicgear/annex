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
    ca-certificates \
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

# Default config
COPY config.toml /app/config.toml

# Create data directory for SQLite
RUN mkdir -p /app/data

ENV ANNEX_CONFIG_PATH=/app/config.toml
ENV ANNEX_ZK_KEY_PATH=/app/zk/keys/membership_vkey.json
ENV ANNEX_DB_PATH=/app/data/annex.db
ENV ANNEX_TTS_BINARY_PATH=/app/assets/piper/piper
ENV ANNEX_TTS_VOICES_DIR=/app/assets/voices

EXPOSE 3000

CMD ["/app/annex-server"]
