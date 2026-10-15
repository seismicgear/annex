#!/usr/bin/env bash
# Generate the same pinned inventories in release assembly and local qualification.
set -euo pipefail
repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$repo_root"
: "${GITHUB_SHA:?A full release source commit is required}"
python3 -m unittest discover -s scripts/tests -p release_sboms_test.py
gate_dir="$(mktemp -d)"
trap 'rm -rf -- "$gate_dir"' EXIT
sha256sum Cargo.lock client/package-lock.json zk/package-lock.json > "$gate_dir/input-locks.sha256"
cargo metadata --locked --format-version 1 > "$gate_dir/metadata.json"
node scripts/verify-dependency-backports.mjs --metadata "$gate_dir/metadata.json"
cargo install cargo-cyclonedx@0.5.7 --locked --root "$gate_dir/tools"
export PATH="$gate_dir/tools/bin:$PATH"
cargo cyclonedx --format json --spec-version 1.4 --target all --all
mkdir -p artifacts/sbom/rust
# 0.5.7 emits <crate>.cdx.json; --all means transitive dependencies, not workspace selection.
python3 - "$gate_dir/metadata.json" <<'PY'
import json, pathlib, shutil, sys
metadata = json.loads(pathlib.Path(sys.argv[1]).read_text())
packages = {package['id']: package for package in metadata['packages']}
for key in metadata['workspace_members']:
    package = packages[key]
    source = pathlib.Path(package['manifest_path']).parent / f"{package['name']}.cdx.json"
    if not source.is_file():
        raise SystemExit(f"Missing generated inventory for {package['name']}: {source}")
    shutil.copyfile(source, pathlib.Path('artifacts/sbom/rust') / f"{package['name']}.json")
PY
python3 scripts/combine-rust-sboms.py artifacts/sbom/rust artifacts/sbom/rust-workspace.json \
  --metadata "$gate_dir/metadata.json" --backports scripts/dependency-backports.json --revision "$GITHUB_SHA"
npx --yes @cyclonedx/cyclonedx-npm@1.19.3 --output-file artifacts/sbom/client.json --package-lock-only client/package.json
npx --yes @cyclonedx/cyclonedx-npm@1.19.3 --output-file artifacts/sbom/zk.json --package-lock-only zk/package.json
python3 - <<'PY'
import json, pathlib
for name in ('rust-workspace', 'client', 'zk'):
    doc = json.loads(pathlib.Path(f'artifacts/sbom/{name}.json').read_text())
    count = len(doc.get('components', []))
    if doc.get('bomFormat') != 'CycloneDX' or count < 1:
        raise SystemExit(f'Invalid or empty {name} inventory')
    print(f'{name}: {count} components')
PY
sha256sum --check "$gate_dir/input-locks.sha256"
# Preserve exact backport bytes and upstream archives beside the inventories.
mkdir -p provenance/dependency-backports
cp scripts/dependency-backports.json provenance/dependency-backports/manifest.json
cp vendor/README.md provenance/dependency-backports/README.md
cp -a vendor/upstream provenance/dependency-backports/
