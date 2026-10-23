#!/usr/bin/env python3
"""Real launchd/XPC authentication and atomic replacement checks on macOS."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import tarfile

assert sys.platform == 'darwin'
root = Path(__file__).resolve().parents[2]
subprocess.run(['clang', '-fblocks', '-Werror', '-fsyntax-only',
                '-mmacosx-version-min=11.0',
                str(root / 'crates/annex-desktop/src/macos_installer.m')], check=True)
subprocess.run(['cargo', 'test', '--locked', '-p', 'annex-macos-updater'], cwd=root, check=True)
with tempfile.TemporaryDirectory(prefix='annex-xpc-boundary-') as directory:
    signer = root / 'scripts/tests/macos-installer-signing-fixture.mjs'
    key = Path(directory) / 'disposable-key.json'
    public_key = subprocess.check_output(['node', str(signer), 'create', str(key)], text=True).strip()
    subprocess.run(['node', str(root / 'scripts/build-macos-installer.mjs')], cwd=root, check=True,
                   env={**os.environ, 'TAURI_SIGNING_PUBLIC_KEY': public_key})
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
                        '-framework', 'Foundation', '-framework', 'Security',
                        str(root / 'scripts/tests/macos-installer-probe.m'),
                        str(root / 'crates/annex-desktop/src/macos_installer.m'),
                        '-o', str(binaries / name)], check=True)
        subprocess.run(['codesign', '--force', '--sign', '-', '--options', 'runtime',
                        str(binaries / name)], check=True)
    subprocess.run(['codesign', '--force', '--sign', '-', '--options', 'runtime', str(app)], check=True)
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
    original = hashlib.sha256((binaries / 'annex-desktop').read_bytes()).hexdigest()
    # Component fixtures cannot qualify the shipped app. The full native UI
    # lane must independently replace and restart the signed release candidate.
    outcomes = {}
    for name, expected in [('annex-desktop', '2'), ('untrusted-client', '1')]:
        result = subprocess.run([str(binaries / name)], check=True, capture_output=True,
                                text=True, timeout=30)
        outcomes[name] = result.stdout.strip()
        assert outcomes[name] == expected, (name, result.stdout, result.stderr)
    assert hashlib.sha256((binaries / 'annex-desktop').read_bytes()).hexdigest() == original

    # Reproduce the post-swap reply validation with the real desktop adapter,
    # helper, launchd and signed files. Losing the old running helper's code
    # before reply authentication must not be mistaken for failed installation.
    candidate = Path(directory) / 'candidate/Annex.app'
    shutil.copytree(app, candidate)
    candidate_binary = candidate / 'Contents/MacOS/annex-desktop'
    shutil.copyfile(binaries / 'untrusted-client', candidate_binary)
    info = candidate / 'Contents/Info.plist'
    metadata = plistlib.loads(info.read_bytes())
    metadata['CFBundleShortVersionString'] = '0.1.3'
    metadata['CFBundleVersion'] = '2'
    info.write_bytes(plistlib.dumps(metadata))
    subprocess.run(['codesign', '--force', '--sign', '-', '--options', 'runtime', str(candidate)], check=True)
    expected = hashlib.sha256(candidate_binary.read_bytes()).hexdigest()
    assert expected != original
    archive = Path(directory) / 'candidate.tar.gz'
    with tarfile.open(archive, 'w:gz') as package:
        package.add(candidate, arcname='Annex.app')
    signature = Path(directory) / 'candidate.sig'
    subprocess.run(['node', str(signer), 'sign', str(key), str(archive), str(signature)], check=True)
    result = subprocess.run([str(binaries / 'annex-desktop'), str(archive), str(signature), '0.1.3'],
                            check=True, capture_output=True, text=True, timeout=45)
    assert result.stdout.strip() == '0', (result.stdout, result.stderr)
    assert hashlib.sha256((binaries / 'annex-desktop').read_bytes()).hexdigest() == expected
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
    print(json.dumps({'realXpcCallerAuthentication': True,
                      'unsignedArchiveRejected': True, 'invalidPayloadKeptInstalledBinary': True,
                      'signedComponentBundleReplaced': True, 'postSwapReplyAuthenticated': True,
                      'outcomes': outcomes}))
