#!/usr/bin/env python3
"""Combine cargo-cyclonedx's per-crate SBOMs into one workspace inventory.

`cargo cyclonedx --all` writes a `bom.json` beside EVERY crate's Cargo.toml.
The release workflow used to collapse those with

    find . -name 'bom.json' -exec cp {} artifacts/sbom/rust-workspace.json \;

which copies each in turn to the same destination, so the file named
"rust-workspace" was whichever of the twelve crates `find` reached last: a
single crate's dependency list, shipped under the workspace's name, and the
completeness check downstream passed on it because a single crate does have
components.

Usage: combine-rust-sboms.py <per-crate-dir> <output.json>
"""
import json
import pathlib
import sys


def combine(src_dir: pathlib.Path, out_path: pathlib.Path) -> int:
    components: list[dict] = []
    seen: set[tuple] = set()
    files = sorted(src_dir.glob("*.json"))
    if not files:
        print(f"error: no per-crate SBOMs in {src_dir}", file=sys.stderr)
        return 1
    for f in files:
        doc = json.loads(f.read_text())
        for c in doc.get("components", []):
            # Dedupe on (name, version): the crates share most of their graph,
            # and a union with twelve copies of `tokio` is not an inventory.
            key = (c.get("name"), c.get("version"))
            if key in seen:
                continue
            seen.add(key)
            components.append(c)
    out = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.4",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "name": "annex-workspace",
            }
        },
        "components": components,
    }
    out_path.write_text(json.dumps(out, indent=2))
    print(f"{out_path}: {len(components)} unique component(s) from {len(files)} crate SBOM(s)")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    sys.exit(combine(pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])))
