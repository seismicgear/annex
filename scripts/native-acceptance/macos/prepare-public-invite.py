"""Read-only validation of an app-generated invite to an independent native host."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import urlsplit


def validate_invite(invite, expected_slug):
    assert re.fullmatch(r'[A-Za-z0-9_-]{1,128}', expected_slug), 'Invalid independent server slug'
    public = urlsplit(invite)
    assert public.scheme == 'https' and public.hostname in ('monolithannex.com', 'www.monolithannex.com')
    assert public.username is None and public.password is None and public.port is None
    assert not public.query and not public.fragment
    assert re.fullmatch(r'/invite/[A-Za-z0-9_-]{1,8192}', public.path)
    encoded = public.path.removeprefix('/invite/')
    payload = json.loads(base64.b64decode(encoded + '=' * (-len(encoded) % 4), altchars=b'-_', validate=True))
    origin = payload['server']
    assert re.fullmatch(r'https://www\.monolithannex\.com/host/[a-f0-9]{64}', origin)
    assert isinstance(payload.get('code'), str) and 1 <= len(payload['code']) <= 256
    return origin


def main():
    invite = os.environ['ANNEX_PUBLIC_TEST_INVITE']
    slug = os.environ['ANNEX_EXPECTED_SERVER_SLUG']
    origin = validate_invite(invite, slug)
    # Independently verify the endpoint signature and decrypt the actual public
    # response. Ordinary HTTP is deliberately refused by the blind router.
    evidence = Path('native-evidence')
    evidence.mkdir(exist_ok=True)
    try:
        result = subprocess.run(['node', str(Path(__file__).resolve().parents[1] / 'public-host-summary.mjs'), origin], check=True, capture_output=True, text=True, timeout=75)
    except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        detail = error.stderr or str(error)
        if isinstance(detail, bytes):
            detail = detail.decode('utf8', errors='replace')
        (evidence / 'public-probe-error.txt').write_text(detail[-32768:])
        raise
    summary = json.loads(result.stdout)
    assert summary['slug'] == slug and summary['public_url'] == origin
    assert isinstance(summary['label'], str) and summary['label']
    (evidence / 'fixture.json').write_text(json.dumps(dict(
        invite=invite, origin=origin, slug=slug, label=summary['label']), indent=2) + '\n')
    (evidence / 'public-destination-verification.json').write_text(json.dumps(dict(
        destination=origin, server_slug=slug, summary=summary,
        invite_sha256=hashlib.sha256(invite.encode()).hexdigest(),
        independent_host=True, server_provisioned_by_test=False), indent=2) + '\n')
    print('App-generated public invite destination matches the independent native host receipt')


if __name__ == '__main__':
    main()
