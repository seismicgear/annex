#!/usr/bin/env python3
"""Fail closed on unsealed apps or unsigned embedded Mach-O code.

Ad-hoc integrity signatures are deliberately distinct from Developer ID and
notarization. This check runs on the completed app before release upload.
"""
import argparse
import hashlib
import json
import pathlib
import plistlib
import subprocess
import sys

MACHO = {bytes.fromhex(value) for value in (
    'feedface', 'cefaedfe', 'feedfacf', 'cffaedfe',
    'cafebabe', 'bebafeca', 'cafebabf', 'bfbafeca',
)}


def check_bundle(app):
    app = app.resolve(strict=True)
    if sys.platform != 'darwin' or app.suffix != '.app':
        raise RuntimeError('Verification requires a real macOS application bundle')
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    if info.get('CFBundleIdentifier') != 'com.annex.desktop':
        raise RuntimeError('Unexpected application identity')
    envelope = app / 'Contents/_CodeSignature/CodeResources'
    if not envelope.is_file():
        raise RuntimeError('Application resource envelope is missing')
    subprocess.run(['codesign', '--verify', '--deep', '--strict', '--verbose=2', str(app)], check=True)
    native = []
    for item in sorted(app.rglob('*')):
        if item.is_symlink() and not item.resolve(strict=True).is_relative_to(app):
            raise RuntimeError(f'Bundle link escapes application: {item.relative_to(app)}')
        if not item.is_file() or item.is_symlink():
            continue
        with item.open('rb') as stream:
            if stream.read(4) not in MACHO:
                continue
        subprocess.run(['codesign', '--verify', '--strict', str(item)], check=True)
        with item.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        native.append({'path': str(item.relative_to(app)), 'sha256': digest})
    if not any(row['path'] == 'Contents/MacOS/annex-desktop' for row in native):
        raise RuntimeError('Main native executable was not verified')
    detail = subprocess.run(['codesign', '-dv', '--verbose=4', str(app)], check=True,
                            capture_output=True, text=True).stderr
    return {'bundle_identifier': info['CFBundleIdentifier'],
            'version': info['CFBundleShortVersionString'],
            'resource_envelope_sha256': hashlib.sha256(envelope.read_bytes()).hexdigest(),
            'strict_bundle_verification': True, 'native_files': native,
            'signing': 'ad-hoc' if 'Signature=adhoc' in detail else 'certificate',
            'signature_details': detail,
            'notarization': 'not established by integrity verification'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', type=pathlib.Path)
    parser.add_argument('--output', required=True, type=pathlib.Path)
    args = parser.parse_args()
    receipt = check_bundle(args.app)
    args.output.write_text(json.dumps(receipt, indent=2) + '\n')
    print(f"Verified sealed bundle and {len(receipt['native_files'])} native files ({receipt['signing']}).")
