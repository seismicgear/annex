#!/usr/bin/env python3
"""Fail closed on unsealed apps or unsigned embedded Mach-O code.

Ad-hoc integrity signatures are deliberately distinct from Developer ID and
notarization. This check runs on the completed app before release upload.
"""
import argparse
import hashlib
import json
import os
import pathlib
import plistlib
import re
import subprocess
import sys

MACHO = {bytes.fromhex(value) for value in (
    'feedface', 'cefaedfe', 'feedfacf', 'cffaedfe',
    'cafebabe', 'bebafeca', 'cafebabf', 'bfbafeca',
)}

REQUIRED_ENTITLEMENTS = {
    'com.apple.security.app-sandbox': True,
    'com.apple.security.network.client': True,
    'com.apple.security.network.server': True,
    'com.apple.security.device.camera': True,
    'com.apple.security.device.audio-input': True,
    'com.apple.security.personal-information.photos-library': False,
}
FORBIDDEN_ENTITLEMENTS = (
    'com.apple.security.get-task-allow',
    'get-task-allow',
    'com.apple.security.cs.disable-library-validation',
    'com.apple.security.cs.allow-unsigned-executable-memory',
    'com.apple.security.cs.allow-dyld-environment-variables',
    'com.apple.private.security.no-sandbox',
)


def verify_entitlements(data):
    """Inspect the signed executable's claims, not its build configuration."""
    try:
        entitlements = plistlib.loads(data)
    except (ValueError, plistlib.InvalidFileException) as error:
        raise RuntimeError('Signed entitlements are not a valid property list') from error
    if not isinstance(entitlements, dict):
        raise RuntimeError('Signed entitlements must be a dictionary')
    for key, expected in REQUIRED_ENTITLEMENTS.items():
        if entitlements.get(key) is not expected:
            raise RuntimeError(f'Signed application entitlement changed: {key}')
    for key in FORBIDDEN_ENTITLEMENTS:
        if key in entitlements and entitlements[key] is not False:
            raise RuntimeError(f'Unsafe signed application entitlement: {key}')
    return entitlements


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
    signed_entitlements = subprocess.run(
        ['codesign', '--display', '--entitlements', '-', '--xml', str(app)],
        check=True, capture_output=True).stdout
    entitlements = verify_entitlements(signed_entitlements)
    if '(runtime)' not in detail:
        raise RuntimeError('Application hardened runtime is missing')
    notarization = 'not established by integrity verification'
    signing = 'ad-hoc' if 'Signature=adhoc' in detail else 'certificate'
    if os.environ.get('ANNEX_NATIVE_SIGNING') == 'true':
        team = os.environ.get('APPLE_TEAM_ID', '')
        if not re.fullmatch(r'[A-Z0-9]{10}', team) or (
                f'TeamIdentifier={team}' not in detail.splitlines()
                or 'Authority=Developer ID Application: ' not in detail
                or '(runtime)' not in detail):
            raise RuntimeError('Application is not signed by the approved Developer ID with hardened runtime')
        subprocess.run(['xcrun', 'stapler', 'validate', str(app)], check=True)
        subprocess.run(['spctl', '--assess', '--type', 'execute', '--verbose=4', str(app)], check=True)
        signing = 'developer-id'
        notarization = 'stapled ticket and Gatekeeper verified'
    return {'bundle_identifier': info['CFBundleIdentifier'],
            'version': info['CFBundleShortVersionString'],
            'resource_envelope_sha256': hashlib.sha256(envelope.read_bytes()).hexdigest(),
            'strict_bundle_verification': True, 'native_files': native,
            'signed_entitlements': entitlements, 'sandbox_verified': True,
            'hardened_runtime_verified': True,
            'signing': signing,
            'signature_details': detail,
            'notarization': notarization}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', type=pathlib.Path)
    parser.add_argument('--output', required=True, type=pathlib.Path)
    args = parser.parse_args()
    receipt = check_bundle(args.app)
    args.output.write_text(json.dumps(receipt, indent=2) + '\n')
    print(f"Verified sealed bundle and {len(receipt['native_files'])} native files ({receipt['signing']}).")
