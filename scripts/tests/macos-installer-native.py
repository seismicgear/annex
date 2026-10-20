#!/usr/bin/env python3
"""Real launchd/XPC authentication and atomic replacement checks on macOS."""
import hashlib
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile

assert sys.platform == 'darwin'
root = Path(__file__).resolve().parents[2]
subprocess.run(['clang', '-fblocks', '-Werror', '-fsyntax-only',
                '-mmacosx-version-min=11.0',
                str(root / 'crates/annex-desktop/src/macos_installer.m')], check=True)
subprocess.run(['cargo', 'test', '--locked', '-p', 'annex-macos-updater'], cwd=root, check=True)
subprocess.run(['node', str(root / 'scripts/build-macos-installer.mjs')], cwd=root, check=True)

with tempfile.TemporaryDirectory(prefix='annex-xpc-boundary-') as directory:
    app = Path(directory) / 'Annex.app'
    contents = app / 'Contents'
    binaries = contents / 'MacOS'
    binaries.mkdir(parents=True)
    service = contents / 'XPCServices/com.annex.desktop.installer.xpc'
    shutil.copytree(root / 'assets/macos-installer/com.annex.desktop.installer.xpc', service)
    (contents / 'Info.plist').write_bytes(plistlib.dumps({
        'CFBundleIdentifier': 'com.annex.desktop',
        'CFBundleExecutable': 'annex-desktop',
        'CFBundlePackageType': 'APPL',
        'CFBundleShortVersionString': '0.1.2',
        'CFBundleVersion': '1',
    }))
    for variant, name in enumerate(('annex-desktop', 'untrusted-client')):
        subprocess.run(['clang', '-fblocks', '-Werror', f'-DANNEX_PROBE_VARIANT={variant}',
                        '-framework', 'Foundation', str(root / 'scripts/tests/macos-installer-probe.m'),
                        '-o', str(binaries / name)], check=True)
        subprocess.run(['codesign', '--force', '--sign', '-', '--options', 'runtime',
                        str(binaries / name)], check=True)
    subprocess.run(['codesign', '--force', '--sign', '-', '--options', 'runtime', str(app)], check=True)
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
    original = hashlib.sha256((binaries / 'annex-desktop').read_bytes()).hexdigest()
    # Neither this probe nor its invalid archive is a functional updater test.
    # The full native UI lane must independently replace a signed real app.
    outcomes = {}
    for name, expected in [('annex-desktop', '2'), ('untrusted-client', '1')]:
        result = subprocess.run([str(binaries / name)], check=True, capture_output=True,
                                text=True, timeout=30)
        outcomes[name] = result.stdout.strip()
        assert outcomes[name] == expected, (name, result.stdout, result.stderr)
    assert hashlib.sha256((binaries / 'annex-desktop').read_bytes()).hexdigest() == original
    print(json.dumps({'realXpcCallerAuthentication': True,
                      'unsignedArchiveRejected': True, 'installedBinaryUnchanged': True,
                      'outcomes': outcomes}))
