"""Bind the actual DMG to authenticated release metadata and source."""
import hashlib
import json
import os
from pathlib import Path
import sys

directory = Path(sys.argv[1]).resolve(strict=True)
arch = sys.argv[2]
catalog = json.loads((directory / 'downloads.json').read_text())
assert catalog['sourceCommit'] == os.environ['SUBJECT_COMMIT']
assert catalog['tag'] == os.environ['RELEASE_TAG']
assert not catalog['unsignedDryRun']
expected = catalog['platforms']['darwin-' + arch]
assert expected['name'] == f'annex-desktop-macos-{arch}.dmg'
asset = directory / expected['name']
with asset.open('rb') as stream:
    digest = hashlib.file_digest(stream, 'sha256').hexdigest()
assert digest == expected['sha256']
assert asset.stat().st_size == expected['size']
assert f'{digest}  {asset.name}' in (directory / 'SHA256SUMS').read_text().splitlines()
Path('native-evidence/published-dmg-verification.json').write_text(json.dumps({
    'passed': True, 'source_commit': catalog['sourceCommit'], 'tag': catalog['tag'],
    'version': catalog['version'], 'asset': asset.name, 'sha256': digest,
    'bytes': asset.stat().st_size, 'acquisition': 'authenticated GitHub release download',
    'browser_quarantine_journey': False}, indent=2) + '\n')
print(f'Actual release DMG verified: {asset.name}, {digest}')
