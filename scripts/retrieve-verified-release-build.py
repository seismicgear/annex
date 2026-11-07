"""Recover immutable native outputs after a release-upload failure, read-only."""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import zipfile

repo = os.environ['GITHUB_REPOSITORY']
source = os.environ['SUBJECT_COMMIT']
run_id = os.environ['BUILD_RUN']
assert repo == 'seismicgear/annex'
assert re.fullmatch(r'[a-f0-9]{40}', source)
assert re.fullmatch(r'[1-9][0-9]*', run_id)
assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd='subject', text=True).strip() == source
expected_jobs = {
    'Pre-flight gates', 'Build (server container image)', 'Build (server tarball)',
    'Build (Windows x86_64, required)', 'Build (Linux x86_64, required)',
    'Build (macOS, required) (aarch64-apple-darwin, aarch64, macos-15)',
    'Build (macOS, required) (x86_64-apple-darwin, x86_64, macos-15-intel)',
    'release',
}


def api(endpoint):
    return json.loads(subprocess.check_output(['gh', 'api', endpoint]))


base = f'repos/{repo}'
run = api(f'{base}/actions/runs/{run_id}')
assert run['head_sha'] == source and str(run['id']) == run_id
assert run['status'] == 'completed' and run['conclusion'] in ('success', 'failure')
assert run['event'] == 'workflow_dispatch' and run['path'] == '.github/workflows/release-desktop.yml'
jobs = api(f'{base}/actions/runs/{run_id}/jobs?per_page=100')
if any(row['name'] == 'reassemble' for row in jobs['jobs']):
    expected_jobs.add('reassemble')
assert jobs['total_count'] == len(jobs['jobs']) == len(expected_jobs)
assert {row['name'] for row in jobs['jobs']} == expected_jobs
for job in jobs['jobs']:
    assert job['head_sha'] == source and job['status'] == 'completed'
    if job['name'] == 'reassemble':
        # A normal full build skips the mutually exclusive recovery job.
        # Never use a recovery run as the origin of another recovery.
        assert job['conclusion'] == 'skipped'
        continue
    if job['name'] == 'release' and job['conclusion'] == 'failure':
        failed = [row['name'] for row in job['steps'] if row['conclusion'] == 'failure']
        assert failed == ['Create GitHub Release'], 'Only an upload failure may be recovered'
        stages = [row for row in job['steps'] if row['name'] == 'Verify signatures and assemble canonical release assets']
        assert len(stages) == 1 and stages[0]['conclusion'] == 'success'
    else:
        assert job['conclusion'] == 'success', f"Native gate failed: {job['name']}"
    if job['name'].startswith('Build (macOS,'):
        steps = [row for row in job['steps'] if row['name'] == 'Verify sealed macOS application and embedded code']
        assert len(steps) == 1 and steps[0]['conclusion'] == 'success'
    if job['name'] == 'Build (Windows x86_64, required)':
        for gate in ('Verify authenticated WebView2 installer prerequisites',
                     'Exercise Windows installer prerequisite failures and repairs'):
            steps = [row for row in job['steps'] if row['name'] == gate]
            assert len(steps) == 1 and steps[0]['conclusion'] == 'success'
inventory = api(f'{base}/actions/runs/{run_id}/artifacts?per_page=100')
assert inventory['total_count'] == len(inventory['artifacts']) <= 100
artifacts = [row for row in inventory['artifacts']
             if not row['name'].endswith('.dockerbuild')
             and row['name'] not in ('verified-release-assembly', 'release-draft-upload')]
assert len({row['name'] for row in artifacts}) == len(artifacts)
root = Path('subject/artifacts')
root.mkdir(exist_ok=False)
recorded = []
for artifact in artifacts:
    assert re.fullmatch(r'[A-Za-z0-9_.-]+', artifact['name']) and artifact['name'] not in ('.', '..')
    assert not artifact['expired']
    assert artifact['workflow_run']['head_sha'] == source and str(artifact['workflow_run']['id']) == run_id
    assert re.fullmatch(r'sha256:[a-f0-9]{64}', artifact['digest'])
    archive = root / f"{artifact['id']}.zip"
    with archive.open('xb') as output:
        subprocess.run(['gh', 'api', f"{base}/actions/artifacts/{artifact['id']}/zip"], stdout=output, check=True)
    with archive.open('rb') as stream:
        assert 'sha256:' + hashlib.file_digest(stream, 'sha256').hexdigest() == artifact['digest']
    assert archive.stat().st_size == artifact['size_in_bytes']
    destination = root / artifact['name']
    destination.mkdir()
    entries = []
    with zipfile.ZipFile(archive) as package:
        assert len(package.infolist()) <= 20000
        assert sum(row.file_size for row in package.infolist()) < 20 * 1024**3
        seen = set()
        for member in package.infolist():
            name = PurePosixPath(member.filename)
            assert not name.is_absolute() and '..' not in name.parts and ':' not in member.filename and '\\' not in member.filename
            assert name.as_posix().casefold() not in seen
            seen.add(name.as_posix().casefold())
            assert not stat.S_ISLNK(member.external_attr >> 16)
            target = destination.joinpath(*name.parts)
            if member.is_dir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with package.open(member) as original, target.open('xb') as output:
                shutil.copyfileobj(original, output)
            assert target.stat().st_size == member.file_size
            with target.open('rb') as stream:
                checksum = hashlib.file_digest(stream, 'sha256').hexdigest()
            entries.append(dict(path=name.as_posix(), bytes=member.file_size, sha256=checksum))
    assert api(f"{base}/actions/artifacts/{artifact['id']}") == artifact
    recorded.append(dict(id=artifact['id'], name=artifact['name'], digest=artifact['digest'],
                         bytes=artifact['size_in_bytes'], entries=entries))
    archive.unlink()  # Only this just-verified temporary ZIP; extracted bytes remain.
    print(f"Verified original {artifact['name']} ({len(entries)} files)")
Path('assembly-provenance.json').write_text(json.dumps(dict(
    source_commit=source, original_build_run=run_id, original_conclusion=run['conclusion'],
    assembly_run=os.environ['GITHUB_RUN_ID'], assembly_harness_commit=os.environ['GITHUB_SHA'],
    read_only=True, no_native_payload_rebuilt=True, original_jobs=jobs['jobs'],
    original_artifacts=recorded), indent=2) + '\n')
