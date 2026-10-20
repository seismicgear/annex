"""Acquire exact native packages from a successful source-bound build job."""
import argparse
import base64
import hashlib
import json
import re
import shutil
import subprocess
import zipfile
from pathlib import Path, PurePosixPath

parser = argparse.ArgumentParser()
parser.add_argument("--run", required=True)
parser.add_argument("--commit", required=True)
parser.add_argument("--platform", choices=("linux", "win32"), required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--subject", type=Path, help="Optional local source checkout for an additional identity check")
args = parser.parse_args()
assert re.fullmatch(r"[1-9][0-9]*", args.run)
assert re.fullmatch(r"[a-f0-9]{40}", args.commit)
base = "repos/seismicgear/annex"


def api(endpoint):
    return json.loads(subprocess.check_output(["gh", "api", endpoint]))


def digest(file):
    with file.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


run = api(f"{base}/actions/runs/{args.run}")
assert run["head_sha"] == args.commit
assert run["path"] == ".github/workflows/release-desktop.yml"
assert run["event"] in ("workflow_dispatch", "push")
# Read inert version data at the authenticated build's exact source. Acceptance
# never checks out or executes code selected by an arbitrary dispatch input.
config = api(f"{base}/contents/crates/annex-desktop/tauri.conf.json?ref={args.commit}")
assert config["type"] == "file" and config["encoding"] == "base64" and config["size"] < 1024**2
source_config = json.loads(base64.b64decode(config["content"]))
version = source_config["version"]
assert re.fullmatch(r"\d+\.\d+\.\d+", version)
if args.subject:
    assert subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=args.subject, text=True).strip() == args.commit
    assert json.loads((args.subject / "crates/annex-desktop/tauri.conf.json").read_text()) == source_config
jobs = api(f"{base}/actions/runs/{args.run}/jobs?per_page=100")
assert jobs["total_count"] == len(jobs["jobs"]) <= 100
platform_label = "Linux" if args.platform == "linux" else "Windows"
for name in ("Pre-flight gates", f"Build ({platform_label} x86_64, required)"):
    matching = [row for row in jobs["jobs"] if row["name"] == name]
    assert len(matching) == 1
    assert matching[0]["head_sha"] == args.commit and matching[0]["conclusion"] == "success"
inventory = api(f"{base}/actions/runs/{args.run}/artifacts?per_page=100")
assert inventory["total_count"] == len(inventory["artifacts"]) <= 100
name = f"annex-{platform_label.lower()}-x86_64"
artifacts = [row for row in inventory["artifacts"] if row["name"] == name]
assert len(artifacts) == 1
artifact = artifacts[0]
assert not artifact["expired"]
assert artifact["workflow_run"]["head_sha"] == args.commit
assert str(artifact["workflow_run"]["id"]) == args.run
assert re.fullmatch(r"sha256:[a-f0-9]{64}", artifact["digest"])
args.output.mkdir(parents=True, exist_ok=False)
archive = args.output / "candidate.zip"
with archive.open("xb") as stream:
    subprocess.run(["gh", "api", f"{base}/actions/artifacts/{artifact['id']}/zip"], stdout=stream, check=True)
assert archive.stat().st_size == artifact["size_in_bytes"]
assert "sha256:" + digest(archive) == artifact["digest"]
patterns = (
    {"deb": r"_amd64\.deb$", "appimage": r"_amd64\.AppImage$"}
    if args.platform == "linux"
    else {"msi": r"_x64_en-US\.msi$", "nsis": r"_x64-setup\.exe$"}
)
with zipfile.ZipFile(archive) as package:
    rows = package.infolist()
    assert len(rows) < 1000 and sum(row.file_size for row in rows) < 20 * 1024**3
    inventories = [row for row in rows if PurePosixPath(row.filename).name == "array-installation.json"]
    assert len(inventories) == 1 and inventories[0].file_size < 1024**2
    installation = json.loads(package.read(inventories[0]))
    assert installation["schemaVersion"] == 1
    assert installation["product"] == "annex" and installation["nativeId"] == "com.annex.desktop"
    assert installation["sourceCommit"] == args.commit and installation["version"] == version
    assert installation["platform"] == args.platform and installation["arch"] == "x64"
    assert len(installation["artifacts"]) == len(patterns)
    assert {entry["kind"] for entry in installation["artifacts"]} == set(patterns)
    for entry in installation["artifacts"]:
        assert re.fullmatch(r"annex-desktop-(linux|windows)-x86_64\.(deb|AppImage|msi|exe)", entry["name"])
        assert re.fullmatch(r"[a-f0-9]{64}", entry["sha256"])
        assert re.fullmatch(r"[a-f0-9]{64}", entry["executableSha256"])
        matches = [row for row in rows if re.search(patterns[entry["kind"]], row.filename)
                   and PurePosixPath(row.filename).name.startswith(f"Annex_{version}_")]
        assert len(matches) == 1
        member = matches[0]
        assert member.file_size == entry["size"] and 0 < member.file_size <= 16 * 1024**3
        # Only exact inventory basenames become local paths; archive paths are never extracted.
        target = args.output / entry["name"]
        with package.open(member) as original, target.open("xb") as stream:
            shutil.copyfileobj(original, stream)
        assert digest(target) == entry["sha256"]
        signatures = [row for row in rows if row.filename == member.filename + ".sig"]
        # Current builds sign DEB alongside the other native artifacts. Older
        # builds may acquire its signature from authenticated reassembly.
        if entry["kind"] != "deb" or signatures:
            assert len(signatures) == 1 and signatures[0].file_size < 16 * 1024
            (args.output / (entry["name"] + ".sig")).write_bytes(package.read(signatures[0]))
(args.output / "array-installation.json").write_text(json.dumps(installation, indent=2) + "\n")
assert api(f"{base}/actions/artifacts/{artifact['id']}") == artifact
receipt = dict(sourceCommit=args.commit, buildRun=args.run, artifactId=artifact["id"],
               artifactDigest=artifact["digest"], platform=args.platform, version=version,
               publishedInstaller=False, acquisition="authenticated immutable Actions artifact",
               files=installation["artifacts"])
(args.output / "acquisition.json").write_text(json.dumps(receipt, indent=2) + "\n")
archive.unlink()
print(json.dumps(receipt, indent=2))
