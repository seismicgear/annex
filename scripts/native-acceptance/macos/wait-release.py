"""Wait for completed release uploads without mistaking a new draft for readiness."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

repository = os.environ['GITHUB_REPOSITORY']
tag = os.environ['RELEASE_TAG']
source = os.environ['SUBJECT_COMMIT']
arch = sys.argv[1]
assert repository == 'seismicgear/annex'
assert re.fullmatch(r'[a-f0-9]{40}', source)
assert re.fullmatch(r'(?:v[0-9][0-9A-Za-z.+-]*|dry-run-[a-f0-9]{40})', tag)
assert arch in ('aarch64', 'x86_64')
required = {f'annex-desktop-macos-{arch}.dmg', 'downloads.json', 'SHA256SUMS'}


def read(endpoint, allow_missing=False):
    result = subprocess.run(['gh', 'api', endpoint], capture_output=True, text=True)
    if result.returncode:
        if allow_missing and '(HTTP 404)' in result.stderr:
            return None
        raise RuntimeError(f'Authenticated release lookup failed: {result.stderr.strip()}')
    return json.loads(result.stdout)


for attempt in range(90):
    release = read(f'repos/{repository}/releases/tags/{tag}', allow_missing=True)
    if release is None:
        # Untagged drafts can be absent from the tag endpoint. Read the
        # authenticated collection without creating tags or changing a release.
        candidates = []
        for page in range(1, 101):
            rows = read(f'repos/{repository}/releases?per_page=100&page={page}')
            assert isinstance(rows, list) and len(rows) <= 100
            candidates.extend(row for row in rows if row['tag_name'] == tag)
            if len(rows) < 100:
                break
        else:
            raise RuntimeError('Release enumeration was incomplete')
        assert len(candidates) <= 1, 'Ambiguous candidate release'
        release = candidates[0] if candidates else None
    if release is not None:
        assert release['tag_name'] == tag and release['target_commitish'] == source
        assets = [row for row in release['assets'] if row['name'] in required]
        assert len({row['name'] for row in assets}) == len(assets)
        if (len(assets) == len(required) and
                all(row['state'] == 'uploaded' and row['size'] > 0 for row in assets)):
            Path('native-evidence/release.json').write_text(json.dumps({
                'tagName': tag, 'isDraft': release['draft'],
                'targetCommitish': source, 'url': release['html_url'],
                'releaseId': release['id'],
                'requiredAssets': [{key: row[key] for key in ('id', 'name', 'size', 'digest')}
                                   for row in assets],
            }, indent=2) + '\n')
            print(f'Exact {tag} {arch} assets are fully uploaded')
            break
    if attempt == 89:
        raise RuntimeError('Exact release uploads did not complete within 30 minutes')
    time.sleep(20)
