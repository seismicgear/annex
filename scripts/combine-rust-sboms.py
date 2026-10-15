#!/usr/bin/env python3
"""Validate every workspace inventory, preserve its graph, and identify backports."""
import argparse
import copy
import hashlib
import json
import pathlib
import re


def combine(src_dir, out_path, metadata, backports, revision, manifest_hash):
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("release revision must be a full Git commit")
    packages = {package["id"]: package for package in metadata["packages"]}
    expected = {packages[key]["name"]: packages[key] for key in metadata["workspace_members"]}
    files = {file.stem: file for file in src_dir.glob("*.json")}
    if not expected or set(files) != set(expected):
        raise ValueError(f"per-crate inventory mismatch: expected {sorted(expected)}, got {sorted(files)}")
    patched = {}
    for entry in backports["crates"]:
        matches = [p for p in packages.values() if p["name"] == entry["name"] and p["version"] == entry["version"]]
        if len(matches) != 1 or matches[0]["source"] is not None:
            raise ValueError(f"backport is not the unique resolved local source: {entry['name']}")
        patched[(entry["name"], entry["version"])] = (matches[0]["id"], entry)

    components, roots, dependencies, found_backports = {}, {}, {}, set()
    for name, file in sorted(files.items()):
        doc = json.loads(file.read_text(encoding="utf-8"))
        root = doc.get("metadata", {}).get("component", {})
        package = expected[name]
        if (root.get("name"), root.get("version"), root.get("bom-ref")) != (name, package["version"], package["id"]):
            raise ValueError(f"inventory root does not match resolved workspace member: {name}")
        if doc.get("bomFormat") != "CycloneDX" or doc.get("specVersion") != "1.4" or not doc.get("components"):
            raise ValueError(f"invalid or empty CycloneDX 1.4 inventory: {name}")
        for component in doc["components"]:
            identity = component.get("name"), component.get("version")
            if identity in patched:
                source_id, entry = patched[identity]
                if component.get("bom-ref") != source_id:
                    raise ValueError(f"inventory substitutes unreviewed source: {identity[0]}")
                component["properties"] = component.get("properties", []) + [
                    {"name": "annex:source-kind", "value": "reviewed-upstream-backport"},
                    {"name": "annex:source-path", "value": entry["path"]},
                    {"name": "annex:source-revision", "value": revision},
                    {"name": "annex:upstream-archive-sha256", "value": entry["archive"]["sha256"]},
                    {"name": "annex:backport-manifest-sha256", "value": manifest_hash},
                    {"name": "annex:patches", "value": json.dumps(entry["patches"], separators=(",", ":"))},
                ]
                component["externalReferences"] = component.get("externalReferences", []) + [
                    {"type": "vcs", "url": f"https://github.com/seismicgear/annex/tree/{revision}/{entry['path']}"},
                    {"type": "other", "url": f"https://github.com/seismicgear/annex/blob/{revision}/scripts/dependency-backports.json"},
                ]
                found_backports.add(identity)
            # A same-name/version registry and path source must never collapse.
            ref = component.get("bom-ref")
            if not ref:
                raise ValueError(f"component lacks a source identity in {name}")
            components.setdefault(ref, component)
        roots[root["bom-ref"]] = copy.deepcopy(root)
        for dependency in doc.get("dependencies", []):
            dependencies.setdefault(dependency["ref"], set()).update(dependency.get("dependsOn", []))
        file.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    if found_backports != set(patched):
        raise ValueError("workspace inventory omits a reviewed backport")
    components.update(roots)

    def all_refs(component):
        yield component["bom-ref"]
        for child in component.get("components", []):
            yield from all_refs(child)

    refs = {ref for component in components.values() for ref in all_refs(component)}
    for ref, children in dependencies.items():
        if ref not in refs or not children.issubset(refs):
            raise ValueError(f"inventory dependency graph has unresolved references: {ref}")
    out = {
        "bomFormat": "CycloneDX", "specVersion": "1.4", "version": 1,
        "metadata": {"component": {"type": "application", "name": "annex-workspace"}, "properties": [
            {"name": "annex:source-revision", "value": revision},
            {"name": "annex:workspace-members", "value": json.dumps(sorted(expected))},
            {"name": "annex:inventory-scope", "value": "All Cargo target platforms; default enabled features; runtime and build dependencies"},
        ]},
        "components": [components[key] for key in sorted(components)],
        "dependencies": [{"ref": key, "dependsOn": sorted(dependencies[key])} for key in sorted(dependencies)],
    }
    out_path.write_text(json.dumps(out, indent=2) + "\n", encoding="utf-8")
    print(f"{out_path}: {len(components)} source identities from all {len(expected)} workspace crates; {len(found_backports)} identified backports")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("src_dir", type=pathlib.Path)
    parser.add_argument("out_path", type=pathlib.Path)
    parser.add_argument("--metadata", required=True, type=pathlib.Path)
    parser.add_argument("--backports", required=True, type=pathlib.Path)
    parser.add_argument("--revision", required=True)
    args = parser.parse_args()
    manifest = args.backports.read_bytes()
    combine(args.src_dir, args.out_path,
            json.loads(args.metadata.read_text(encoding="utf-8")), json.loads(manifest),
            args.revision, hashlib.sha256(manifest).hexdigest())
