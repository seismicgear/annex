#!/usr/bin/env bash
# Shared blocking gate for PR CI and every signed/unsigned release dispatch.
set -euo pipefail

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$repo_root"
gate_dir="$(mktemp -d)"
trap 'rm -rf -- "$gate_dir"' EXIT

cargo metadata --locked --all-features --format-version 1 > "$gate_dir/metadata.json"
node scripts/verify-dependency-backports.mjs --metadata "$gate_dir/metadata.json"
node scripts/verify-dependency-maintenance.mjs --metadata "$gate_dir/metadata.json"
node --test scripts/tests/dependency-backports.test.mjs scripts/tests/dependency-maintenance.test.mjs
cargo test --locked --manifest-path scripts/dtls-regressions/Cargo.toml --lib supported_group_regression
cargo test --locked --release --manifest-path scripts/security-regressions/Cargo.toml
cargo test --locked --release --manifest-path scripts/updater-archive-regressions/Cargo.toml
dbus-run-session -- cargo test --locked --release --manifest-path scripts/single-instance-regressions/Cargo.toml

# The official release binary is the same version used in local verification.
# Its immutable SHA-256 is the GitHub release asset digest; verify before unpacking.
archive_name="cargo-deny-0.18.6-x86_64-unknown-linux-musl"
archive="$gate_dir/$archive_name.tar.gz"
curl --fail --location --retry 3 --max-time 120 \
  "https://github.com/EmbarkStudios/cargo-deny/releases/download/0.18.6/$archive_name.tar.gz" \
  --output "$archive"
printf '%s  %s\n' a9c3db6db22288045ad8407bd0febbb13c6339b3d948d666ae8ef634fb3f3853 "$archive" | sha256sum --check -
tar -xzf "$archive" -C "$gate_dir" --strip-components=1 "$archive_name/cargo-deny"
"$gate_dir/cargo-deny" --version
"$gate_dir/cargo-deny" --all-features --locked check --metadata-path "$gate_dir/metadata.json"

# cargo-deny checks the resolved feature graph. Optional/other-target packages
# can remain in Cargo.lock without entering that graph (for example reqwest's
# QUIC transport). Audit the complete shipped lockfile as well. Maintenance
# dispositions remain separately enforced above; no vulnerability is ignored.
cargo install --locked cargo-audit --version 0.22.2
cargo audit --file Cargo.lock
