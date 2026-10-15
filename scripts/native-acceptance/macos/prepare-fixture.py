"""Prepare a real, isolated native server; never create identities or invites."""
import datetime
import hashlib
import json
import os
import pathlib
import platform
import secrets
import socket
import ssl
import subprocess
import sys
import time
import urllib.request

if sys.platform != 'darwin' or os.environ.get('GITHUB_ACTIONS') != 'true':
    raise SystemExit('This fixture is restricted to the ephemeral macOS runner')
source = pathlib.Path(sys.argv[1]).resolve(strict=True)
server = pathlib.Path(sys.argv[2]).resolve(strict=True)
out = pathlib.Path('native-evidence').resolve()
base = pathlib.Path(os.environ['RUNNER_TEMP']) / 'annex-native-destination'
base.mkdir(mode=0o700, exist_ok=False)
out.mkdir(exist_ok=True)
hostname = 'annex-native.localtest.me'
origin = f'https://{hostname}:3480'
label = 'Annex Mac Native Target ' + platform.machine()
if socket.gethostbyname(hostname) != '127.0.0.1':
    raise RuntimeError('Destination DNS must resolve to loopback')
for port in (3080, 3480):
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', port))


def execute(*args):
    with (out / 'fixture-setup.log').open('ab') as log:
        subprocess.run(args, check=True, stdout=log, stderr=subprocess.STDOUT)


tls = base / 'tls'
tls.mkdir(mode=0o700)
execute('openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-sha256',
        '-days', '2', '-subj', '/CN=Annex Ephemeral Mac Native Test CA',
        '-addext', 'basicConstraints=critical,CA:TRUE,pathlen:0',
        '-addext', 'keyUsage=critical,keyCertSign,cRLSign',
        '-keyout', str(tls / 'ca-private.key'), '-out', str(tls / 'ca.crt'))
execute('openssl', 'req', '-new', '-newkey', 'rsa:2048', '-nodes', '-sha256',
        '-subj', f'/CN={hostname}', '-keyout', str(tls / 'server-private.key'),
        '-out', str(tls / 'server.csr'))
(tls / 'extensions.cnf').write_text(
    f'subjectAltName=DNS:{hostname}\nbasicConstraints=critical,CA:FALSE\n'
    'keyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n')
execute('openssl', 'x509', '-req', '-in', str(tls / 'server.csr'), '-CA', str(tls / 'ca.crt'),
        '-CAkey', str(tls / 'ca-private.key'), '-CAcreateserial', '-days', '2', '-sha256',
        '-extfile', str(tls / 'extensions.cnf'), '-out', str(tls / 'server.crt'))
# Only this disposable runner's test CA; no validation bypass or browser flag.
execute('sudo', 'security', 'add-trusted-cert', '-d', '-r', 'trustRoot',
        '-k', '/Library/Keychains/System.keychain', str(tls / 'ca.crt'))
origins = [origin, 'tauri://localhost', 'https://tauri.localhost', 'http://tauri.localhost']
config = base / 'config.toml'
database = base / 'annex.db'
config.write_text(f"[server]\nhost='127.0.0.1'\nport=3080\npublic_url='{origin}'\n"
                  f"[database]\npath='{database}'\n[security]\nenforce_zk_proofs=true\n"
                  f"[cors]\nallowed_origins={origins!r}\n")
model = list(pathlib.Path('/Applications/Annex.app/Contents/Resources').rglob('model.safetensors'))
if len(model) != 1:
    raise RuntimeError('Expected the real packaged alignment model exactly once')
env = {key: value for key, value in os.environ.items() if not key.startswith('ANNEX_')}
env.update(ANNEX_BUILD_PROFILE='production', ANNEX_ENFORCE_ZK_PROOFS='true',
           ANNEX_SIGNING_KEY=secrets.token_hex(32), ANNEX_CLIENT_DIR=str(source / 'client/dist'),
           ANNEX_EMBEDDING_MODEL_DIR=str(model[0].parent), ANNEX_UPLOAD_DIR=str(base / 'uploads'),
           ANNEX_OPEN_BROWSER='false', ANNEX_CORS_ORIGINS=','.join(origins), ANNEX_SERVER_LABEL=label)
children = []


def start(args, name):
    with (out / (name + '.log')).open('wb') as log:
        process = subprocess.Popen(args, cwd=source, env=env, stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
    children.append(process)
    return process.pid


def healthy(url, context=None):
    for _ in range(120):
        if any(child.poll() is not None for child in children):
            raise RuntimeError('Isolated destination process exited')
        try:
            with urllib.request.urlopen(url, context=context, timeout=2) as response:
                if json.load(response)['status'] == 'ok':
                    return
        except (OSError, ValueError):
            pass
        time.sleep(.5)
    raise RuntimeError('Destination did not become healthy')


server_pid = start([str(server), str(config)], 'target-server')
healthy('http://127.0.0.1:3080/health')
proxy_config = base / 'proxy-private.json'
proxy_config.write_text(json.dumps(dict(listenHost='127.0.0.1', listenPort=3480, upstreamPort=3080,
    hostname=hostname, certificate=str(tls / 'server.crt'), privateKey=str(tls / 'server-private.key'),
    runtimeReceipt=str(out / 'proxy-runtime.json'), passiveEvidenceDirectory=str(out))))
scripts = pathlib.Path(__file__).resolve().parent
proxy_pid = start(['node', str(scripts / 'proxy.mjs'), str(proxy_config)], 'target-proxy')
healthy(origin + '/health', ssl.create_default_context(cafile=str(tls / 'ca.crt')))
# curl uses the runner's normal system certificate trust, independently of Python's explicit CA.
execute('curl', '--fail', '--silent', origin + '/health')
observer_pid = start(['node', str(scripts / 'passive-observer.mjs'), str(database), str(out)], 'target-observer')
with urllib.request.urlopen(origin + '/api/public/server/summary',
        context=ssl.create_default_context(cafile=str(tls / 'ca.crt'))) as response:
    summary = json.load(response)
with server.open('rb') as stream:
    server_sha = hashlib.file_digest(stream, 'sha256').hexdigest()
fixture = dict(created_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
    origin=origin, label=label, slug=summary['slug'], database=str(database),
    source_commit=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=source, text=True).strip(),
    server_sha256=server_sha, server_pid=server_pid, proxy_pid=proxy_pid, observer_pid=observer_pid,
    listen_addresses=['127.0.0.1:3080', '127.0.0.1:3480'], enforce_zk_proofs=True,
    system_tls_validation=True, identities_created=False, invitations_created=False)
(out / 'fixture.json').write_text(json.dumps(fixture, indent=2) + '\n')
print(json.dumps(fixture, indent=2))
