"""Cache only the actual compiled server, never test identities or server state."""
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys

mode, target = sys.argv[1:]
assert mode in {'record', 'restore'}
assert sys.platform == 'darwin' and os.environ.get('GITHUB_ACTIONS') == 'true'
expected_target = {'arm64': 'aarch64-apple-darwin', 'x86_64': 'x86_64-apple-darwin'}[platform.machine()]
assert target == expected_target
source = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd='subject', text=True).strip()
assert re.fullmatch(r'[a-f0-9]{40}', source) and source == os.environ['SUBJECT_COMMIT']
directory = Path('.native-destination-cache')
cached = directory / 'annex-server'
manifest = directory / 'provenance.json'
built = Path('subject/target') / target / 'release/annex-server'


def digest(path):
    assert path.is_file() and not path.is_symlink() and os.access(path, os.X_OK)
    with path.open('rb') as stream:
        header = stream.read(8)
        assert header[:4] == bytes.fromhex('cffaedfe'), 'Expected a native 64-bit Mach-O executable'
        cpu = int.from_bytes(header[4:8], 'little')
        assert cpu == (0x100000c if target.startswith('aarch64') else 0x1000007)
        stream.seek(0)
        return hashlib.file_digest(stream, 'sha256').hexdigest()


if mode == 'record':
    checksum = digest(built)
    directory.mkdir(exist_ok=False)
    shutil.copy2(built, cached)
    assert digest(cached) == checksum
    manifest.write_text(json.dumps(dict(source_commit=source, target=target, sha256=checksum,
        build_run=os.environ['GITHUB_RUN_ID'], harness_commit=os.environ['GITHUB_SHA']), indent=2) + '\n')
else:
    receipt = json.loads(manifest.read_text())
    assert receipt['source_commit'] == source and receipt['target'] == target
    assert digest(cached) == receipt['sha256'], 'Cached server bytes do not match recorded compilation'
    built.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(cached, built)
    assert digest(built) == receipt['sha256']
Path('native-evidence').mkdir(exist_ok=True)
shutil.copy2(manifest, 'native-evidence/destination-compilation.json')
print(json.dumps(dict(mode=mode, source_commit=source, target=target, sha256=digest(built))))
