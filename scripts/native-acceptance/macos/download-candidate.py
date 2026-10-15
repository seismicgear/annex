"""Read a source-bound signed build artifact without access to draft releases.

This is candidate qualification, not published-installer acceptance. The public
run uses the ordinary release-download path and must test the same DMG digest.
"""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import zipfile

directory = Path(sys.argv[1])
arch = sys.argv[2]
source = os.environ['SUBJECT_COMMIT']
run_id = os.environ['CANDIDATE_RUN']
repository = os.environ['GITHUB_REPOSITORY']
assert repository == 'seismicgear/annex'
assert re.fullmatch(r'[a-f0-9]{40}', source)
assert re.fullmatch(r'[1-9][0-9]*', run_id)
assert os.environ['RELEASE_TAG'] == 'dry-run-' + source
assert arch in ('aarch64', 'x86_64')


def api(endpoint):
    return json.loads(subprocess.check_output(['gh', 'api', endpoint]))


base = f'repos/{repository}'
run = api(f'{base}/actions/runs/{run_id}')
assert run['head_sha'] == source and str(run['id']) == run_id
assert run['path'] == '.github/workflows/release-desktop.yml'
jobs = api(f'{base}/actions/runs/{run_id}/jobs?per_page=100')
assert jobs['total_count'] <= 100
job_name = f'Build (macOS, required) ({arch}-apple-darwin, {arch}, ' + ('macos-15)' if arch == 'aarch64' else 'macos-15-intel)')
matching_jobs = [row for row in jobs['jobs'] if row['name'] == job_name]
assert len(matching_jobs) == 1
job = matching_jobs[0]
assert job['head_sha'] == source and job['conclusion'] == 'success'
gates = [row for row in job['steps'] if row['name'] == 'Verify sealed macOS application and embedded code']
assert len(gates) == 1 and gates[0]['conclusion'] == 'success'
inventory = api(f'{base}/actions/runs/{run_id}/artifacts?per_page=100')
assert inventory['total_count'] <= 100
artifacts = [row for row in inventory['artifacts'] if row['name'] == f'annex-macos-{arch}']
assert len(artifacts) == 1
artifact = artifacts[0]
assert not artifact['expired'] and artifact['workflow_run']['head_sha'] == source
assert str(artifact['workflow_run']['id']) == run_id
assert re.fullmatch(r'sha256:[a-f0-9]{64}', artifact['digest'])
directory.mkdir(parents=True, exist_ok=False)
archive = directory / 'candidate-artifact.zip'
with archive.open('xb') as output:
    subprocess.run(['gh', 'api', f"{base}/actions/artifacts/{artifact['id']}/zip"], stdout=output, check=True)
with archive.open('rb') as stream:
    assert 'sha256:' + hashlib.file_digest(stream, 'sha256').hexdigest() == artifact['digest']
assert archive.stat().st_size == artifact['size_in_bytes']
version = json.loads(Path('subject/crates/annex-desktop/tauri.conf.json').read_text())['version']
asset = directory / f'annex-desktop-macos-{arch}.dmg'
with zipfile.ZipFile(archive) as package:
    matches = [row for row in package.infolist() if row.filename.endswith('.dmg')]
    assert len(matches) == 1
    member = matches[0]
    parts = PurePosixPath(member.filename)
    assert not parts.is_absolute() and '..' not in parts.parts
    assert parts.name.startswith(f'Annex_{version}_') and member.file_size > 0
    # Extract one known file into a fixed destination; never extract archive paths.
    with package.open(member) as original, asset.open('xb') as output:
        shutil.copyfileobj(original, output)
    assert asset.stat().st_size == member.file_size
with asset.open('rb') as stream:
    digest = hashlib.file_digest(stream, 'sha256').hexdigest()
assert api(f"{base}/actions/artifacts/{artifact['id']}") == artifact
receipt = dict(passed=True, source_commit=source, tag=None, version=version,
    asset=asset.name, sha256=digest, bytes=asset.stat().st_size,
    acquisition='authenticated read-only GitHub Actions build artifact',
    published_installer=False, candidate_build_run=run_id,
    artifact_id=artifact['id'], artifact_sha256=artifact['digest'],
    artifact_member=member.filename, browser_quarantine_journey=False)
Path('native-evidence/installer-verification.json').write_text(json.dumps(receipt, indent=2) + '\n')
Path('native-evidence/release.json').write_text(json.dumps(dict(
    tagName=None, isDraft=None, targetCommitish=source, url=run['html_url'],
    candidateArtifact=True, publishedInstaller=False), indent=2) + '\n')
print(f'Exact candidate artifact verified: {asset.name}, {digest}; no publication claimed')
