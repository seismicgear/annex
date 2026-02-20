# Annex server multi-stage Docker build.
#
# Stage 1: Build Rust server binary
# Stage 2: Compile ZK circuits and generate proving/verification keys
# Stage 3: Build client static files (consumes ZK wasm/zkey)
# Stage 4: Minimal runtime image

# ── Build server ──
FROM rust:1.82-slim-bookworm AS server-builder

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

# Default config
COPY config.toml /app/config.toml

# Create data directory for SQLite
RUN mkdir -p /app/data

ENV ANNEX_CONFIG_PATH=/app/config.toml
ENV ANNEX_ZK_KEY_PATH=/app/zk/keys/membership_vkey.json
ENV ANNEX_DB_PATH=/app/data/annex.db

EXPOSE 3000

CMD ["/app/annex-server"]
