"""Recover inert verified assembly bytes into an existing unpublished draft."""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import zipfile

run_id = os.environ["ASSEMBLY_RUN"]
source = os.environ["SUBJECT_COMMIT"]
release_id = os.environ["EXISTING_RELEASE_ID"]
assert re.fullmatch(r"[1-9][0-9]*", run_id)
assert re.fullmatch(r"[1-9][0-9]*", release_id)
assert re.fullmatch(r"[a-f0-9]{40}", source)
assert os.environ["GITHUB_REPOSITORY"] == "seismicgear/annex"
base = "repos/seismicgear/annex"
tag = "dry-run-" + source


def api(route):
    return json.loads(subprocess.check_output(["gh", "api", route]))


def sha(file):
    with file.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def existing_draft():
    release = api(f"{base}/releases/{release_id}")
    assert str(release["id"]) == release_id and release["draft"] is True
    assert release["published_at"] is None and release["target_commitish"] == source
    assert release["tag_name"] == tag
    return release


existing_draft()
run = api(f"{base}/actions/runs/{run_id}")
assert run["path"] == ".github/workflows/release-desktop.yml"
assert run["event"] == "workflow_dispatch" and run["conclusion"] == "success"
jobs = api(f"{base}/actions/runs/{run_id}/jobs?per_page=100")
assert jobs["total_count"] == len(jobs["jobs"]) <= 100
matching = [row for row in jobs["jobs"] if row["name"] == "reassemble / assemble"]
assert len(matching) == 1 and matching[0]["conclusion"] == "success"
assert matching[0]["head_sha"] == run["head_sha"]
listed = api(f"{base}/actions/runs/{run_id}/artifacts?per_page=100")
assert listed["total_count"] == len(listed["artifacts"]) <= 100
matching = [row for row in listed["artifacts"] if row["name"] == "verified-release-assembly"]
assert len(matching) == 1
artifact = matching[0]
assert not artifact["expired"] and str(artifact["workflow_run"]["id"]) == run_id
assert artifact["workflow_run"]["head_sha"] == run["head_sha"]
assert 0 < artifact["size_in_bytes"] < 6 * 1024**3
assert re.fullmatch(r"sha256:[a-f0-9]{64}", artifact["digest"])
archive = Path(os.environ["RUNNER_TEMP"]) / "annex-verified-assembly.zip"
with archive.open("xb") as stream:
    subprocess.run(["gh", "api", f"{base}/actions/artifacts/{artifact['id']}/zip"], stdout=stream, check=True)
assert archive.stat().st_size == artifact["size_in_bytes"]
assert "sha256:" + sha(archive) == artifact["digest"]
output = Path("release-assets")
output.mkdir(exist_ok=False)
with zipfile.ZipFile(archive) as package:
    rows = [row for row in package.infolist() if not row.is_dir()]
    assert len(rows) < 100 and sum(row.file_size for row in rows) < 6 * 1024**3
    assert len({row.filename for row in rows}) == len(rows)
    for row in rows:
        path = PurePosixPath(row.filename)
        assert "\\" not in row.filename and not path.is_absolute() and ".." not in path.parts
        assert (row.external_attr >> 16) & 0o170000 != 0o120000
        if path.parts[:-1] in (("subject", "release-assets"), ("release-assets",)):
            assert re.fullmatch(r"[A-Za-z0-9_.+-]+", path.name)
            target = output / path.name
        else:
            assert row.filename in ("assembly-provenance.json", "RELEASE_NOTES.md")
            target = Path(row.filename)
        with package.open(row) as original, target.open("xb") as destination:
            shutil.copyfileobj(original, destination)
provenance = json.loads(Path("assembly-provenance.json").read_text())
assert provenance["source_commit"] == source and str(provenance["assembly_run"]) == run_id
assert provenance["assembly_harness_commit"] == run["head_sha"]
assert provenance["no_native_payload_rebuilt"] is True
catalog = json.loads((output / "downloads.json").read_text())
assert catalog["sourceCommit"] == source and catalog["tag"] == tag
assert catalog["unsignedDryRun"] is False and re.fullmatch(r"\d+\.\d+\.\d+", catalog["version"])
checksums = {}
for line in (output / "SHA256SUMS").read_text().splitlines():
    match = re.fullmatch(r"([a-f0-9]{64})  ([A-Za-z0-9_.+-]+)", line)
    assert match and match[2] not in checksums
    assert sha(output / match[2]) == match[1]
    checksums[match[2]] = match[1]
assert set(checksums) == {file.name for file in output.iterdir()} - {"SHA256SUMS"}
assert api(f"{base}/actions/artifacts/{artifact['id']}") == artifact
existing_draft()
Path("assembly-acquisition.json").write_text(json.dumps(dict(
    sourceCommit=source, assemblyRun=run_id, assemblyRevision=run["head_sha"],
    artifact=artifact, existingReleaseId=release_id, filesVerified=len(checksums),
    artifactDigestVerified=True, allFilesChecksumVerified=True,
    nativePayloadBytesUnchanged=True, draftOnly=True, publication=False), indent=2) + "\n")
archive.unlink()
print("Authenticated complete assembly and existing exact-source unpublished draft")
