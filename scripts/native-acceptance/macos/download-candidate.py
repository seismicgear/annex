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

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from candidate_readiness import wait_for_native_candidate

directory = Path(sys.argv[1])
arch = sys.argv[2]
include_updater = len(sys.argv) == 4 and sys.argv[3] == '--updater'
assert len(sys.argv) == 3 or include_updater
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
job_name = f'Build (macOS, required) ({arch}-apple-darwin, {arch}, ' + ('macos-15)' if arch == 'aarch64' else 'macos-15-intel)')
wait_for_native_candidate(api, run_id, source, job_name,
                          int(os.environ.get('ANNEX_CANDIDATE_WAIT_SECONDS', '0')))
jobs = api(f'{base}/actions/runs/{run_id}/jobs?per_page=100')
assert jobs['total_count'] == len(jobs['jobs']) <= 100
preflight = [row for row in jobs['jobs'] if row['name'] == 'Pre-flight gates']
assert len(preflight) == 1 and preflight[0]['head_sha'] == source and preflight[0]['conclusion'] == 'success'
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
source_config = api(f'{base}/contents/crates/annex-desktop/tauri.conf.json?ref={source}')
import base64
assert source_config['encoding'] == 'base64' and source_config['size'] < 1024**2
version = json.loads(base64.b64decode(source_config['content']))['version']
asset = directory / f'annex-desktop-macos-{arch}.dmg'
with zipfile.ZipFile(archive) as package:
    inventories = [row for row in package.infolist() if PurePosixPath(row.filename).name == 'array-installation.json']
    assert len(inventories) == 1 and inventories[0].file_size < 1024**2
    installation = json.loads(package.read(inventories[0]))
    assert installation['schemaVersion'] == 1 and installation['product'] == 'annex'
    assert installation['nativeId'] == 'com.annex.desktop' and installation['platform'] == 'darwin'
    assert installation['arch'] == ('arm64' if arch == 'aarch64' else 'x64')
    assert installation['sourceCommit'] == source and installation['version'] == version
    assert len(installation['artifacts']) == 1
    installed = installation['artifacts'][0]
    assert installed['kind'] == 'dmg' and installed['name'] == asset.name
    assert installed['executable'] == 'Annex.app/Contents/MacOS/annex-desktop'
    assert re.fullmatch(r'[a-f0-9]{64}', installed['executableSha256'])
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
    if include_updater:
        payloads = [row for row in package.infolist() if PurePosixPath(row.filename).name == 'Annex.app.tar.gz']
        assert len(payloads) == 1 and 0 < payloads[0].file_size < 2 * 1024**3
        signatures = [row for row in package.infolist() if row.filename == payloads[0].filename + '.sig']
        assert len(signatures) == 1 and 0 < signatures[0].file_size < 16384
        payload = directory / f'annex-desktop-macos-{arch}.app.tar.gz'
        with package.open(payloads[0]) as original, payload.open('xb') as output:
            shutil.copyfileobj(original, output)
        (directory / (payload.name + '.sig')).write_bytes(package.read(signatures[0]))
        with payload.open('rb') as stream:
            updater_digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        (directory / 'updater-payload.json').write_text(json.dumps(dict(
            name=payload.name, sha256=updater_digest, size=payload.stat().st_size,
            sourceCommit=source, artifactId=artifact['id'], artifactDigest=artifact['digest']
        ), indent=2) + '\n')
        (directory / 'array-installation.json').write_text(json.dumps(installation, indent=2) + '\n')
with asset.open('rb') as stream:
    digest = hashlib.file_digest(stream, 'sha256').hexdigest()
assert installed['sha256'] == digest and installed['size'] == asset.stat().st_size
Path('native-evidence/array-installation.json').write_text(json.dumps(installation, indent=2) + '\n')
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
