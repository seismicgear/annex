"""Acquire a DEB signature from authenticated, source-bound release reassembly."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import zipfile

parser = argparse.ArgumentParser()
parser.add_argument("--run", required=True)
parser.add_argument("--candidate", type=Path, required=True)
args = parser.parse_args()
assert re.fullmatch(r"[1-9][0-9]*", args.run)
base = "repos/seismicgear/annex"


def api(route):
    return json.loads(subprocess.check_output(["gh", "api", route]))


def sha(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


inventory = json.loads((args.candidate / "array-installation.json").read_text())
acquisition = json.loads((args.candidate / "acquisition.json").read_text())
assert inventory["platform"] == "linux" and inventory["arch"] == "x64"
deb = next(row for row in inventory["artifacts"] if row["kind"] == "deb")
assert deb["name"] == "annex-desktop-linux-x86_64.deb"
assert sha(args.candidate / deb["name"]) == deb["sha256"]
run = api(f"{base}/actions/runs/{args.run}")
assert run["path"] == ".github/workflows/release-desktop.yml"
assert run["event"] == "workflow_dispatch" and run["conclusion"] == "success"
jobs = api(f"{base}/actions/runs/{args.run}/jobs?per_page=100")
assert jobs["total_count"] == len(jobs["jobs"]) <= 100
matching = [row for row in jobs["jobs"] if row["name"] == "reassemble / assemble"]
assert len(matching) == 1 and matching[0]["conclusion"] == "success"
assert matching[0]["head_sha"] == run["head_sha"]
artifacts = api(f"{base}/actions/runs/{args.run}/artifacts?per_page=100")
assert artifacts["total_count"] == len(artifacts["artifacts"]) <= 100
matching = [row for row in artifacts["artifacts"] if row["name"] == "updater-variant-metadata"]
assert len(matching) == 1
artifact = matching[0]
assert not artifact["expired"] and artifact["workflow_run"]["head_sha"] == run["head_sha"]
assert str(artifact["workflow_run"]["id"]) == args.run
assert 0 < artifact["size_in_bytes"] < 64 * 1024**2
assert re.fullmatch(r"sha256:[a-f0-9]{64}", artifact["digest"])
archive = args.candidate / "updater-metadata.zip"
with archive.open("xb") as stream:
    subprocess.run(["gh", "api", f"{base}/actions/artifacts/{artifact['id']}/zip"], stdout=stream, check=True)
assert archive.stat().st_size == artifact["size_in_bytes"]
assert "sha256:" + sha(archive) == artifact["digest"]
with zipfile.ZipFile(archive) as package:
    rows = package.infolist()
    assert len(rows) < 100 and sum(row.file_size for row in rows) < 64 * 1024**2
    assert len({row.filename for row in rows}) == len(rows)
    receipt = json.loads(package.read("updater-variants.json"))
    provenance = json.loads(package.read("assembly-provenance.json"))
    assert provenance["source_commit"] == inventory["sourceCommit"]
    assert str(provenance["original_build_run"]) == str(acquisition["buildRun"])
    assert str(provenance["assembly_run"]) == args.run
    assert provenance["assembly_harness_commit"] == run["head_sha"]
    assert provenance["no_native_payload_rebuilt"] is True
    assert receipt["sourceCommit"] == inventory["sourceCommit"] and receipt["version"] == inventory["version"]
    assert receipt["name"] == deb["name"] and receipt["sha256"] == deb["sha256"] and receipt["size"] == deb["size"]
    assert receipt["payloadBytesUnchanged"] is True
    signature = package.read("release-assets/" + deb["name"] + ".sig")
    assert 0 < len(signature) < 16 * 1024
    assert hashlib.sha256(signature).hexdigest() == receipt["signatureSha256"]
assert api(f"{base}/actions/artifacts/{artifact['id']}") == artifact
with (args.candidate / (deb["name"] + ".sig")).open("xb") as stream:
    stream.write(signature)
(args.candidate / "updater-metadata-acquisition.json").write_text(json.dumps(dict(
    sourceCommit=inventory["sourceCommit"], metadataRun=args.run, metadataRevision=run["head_sha"],
    artifactId=artifact["id"], artifactDigest=artifact["digest"], verifiedSignatureReceipt=receipt,
    privateSigningKeyUsed=False, nativePayloadBytesUnchanged=True), indent=2) + "\n")
archive.unlink()
print("Verified exact-source DEB updater signature metadata")
