import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import https from 'node:https';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const execute = promisify(execFile);
const apiUrl = 'https://api.github.com/repos/EmbarkStudios/cargo-deny/releases/assets/320198486';
const archiveName = 'cargo-deny-0.18.6-x86_64-unknown-linux-musl';
const archiveSha256 = 'a9c3db6db22288045ad8407bd0febbb13c6339b3d948d666ae8ef634fb3f3853';
const sourceFile = new URL('../check-dependency-security.sh', import.meta.url);
const shellQuote = value => `'${value.replaceAll("'", "'\\''")}'`;

test('the dependency gate verifies API archive bytes before extraction and execution', async t => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'annex-dependency-download-'));
  const servers = [];
  t.after(async () => {
    for (const server of servers) server.closeAllConnections();
    await Promise.all(servers.map(server => new Promise(resolve => server.close(resolve))));
    assert.equal(path.dirname(root), path.resolve(os.tmpdir()));
    assert.ok(path.basename(root).startsWith('annex-dependency-download-'));
    await fs.rm(root, { recursive: true, force: true });
  });
  const production = await fs.readFile(sourceFile, 'utf8');
  assert.equal(production.split(apiUrl).length, 2, 'one exact official endpoint');
  assert.equal(production.split(archiveSha256).length, 2, 'one unchanged trusted digest');
  assert.ok(production.includes(`archive_name="${archiveName}"`));
  const tools = {};
  for (const name of ['curl', 'tar', 'sha256sum']) {
    tools[name] = (await execute('bash', ['-c', `command -v ${name}`])).stdout.trim();
    assert.ok(path.isAbsolute(tools[name]), `real ${name} is required`);
  }
  const key = path.join(root, 'fixture-key.pem');
  const cert = path.join(root, 'fixture-cert.pem');
  const config = path.join(root, 'openssl.cnf');
  await fs.writeFile(config, '[req]\ndistinguished_name=dn\n[dn]\n');
  await execute('openssl', ['req', '-config', config, '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
    '-subj', '/CN=localhost', '-addext', 'subjectAltName=IP:127.0.0.1', '-keyout', key, '-out', cert]);

  // Only this locally authored harmless program is executed by the fixture.
  // It proves the production shell's version/audit ordering and exit handling,
  // not the result of a real cargo-deny advisory audit.
  const payload = path.join(root, 'payload');
  await fs.mkdir(path.join(payload, archiveName), { recursive: true });
  const fixtureProgram = `#!/usr/bin/env bash
set -euo pipefail
printf 'deny:%s\\n' "$*" >> "$ANNEX_DOWNLOAD_TRACE"
if [ "$#" -eq 1 ] && [ "$1" = '--version' ]; then
  printf 'cargo-deny 0.18.6\\n'
else
  [ "$#" -eq 5 ] && [ "$1" = '--all-features' ] && [ "$2" = '--locked' ]
  [ "$3" = 'check' ] && [ "$4" = '--metadata-path' ] && [ -s "$5" ]
  exit "\${ANNEX_FIXTURE_AUDIT_EXIT:-0}"
fi
`;
  await fs.writeFile(path.join(payload, archiveName, 'cargo-deny'), fixtureProgram, { mode: 0o755 });
  const archive = path.join(root, 'fixture.tar.gz');
  await execute(tools.tar, ['-czf', archive, '-C', payload, archiveName]);
  const bytes = await fs.readFile(archive);
  const fixtureHash = createHash('sha256').update(bytes).digest('hex');
  const tls = { key: await fs.readFile(key), cert: await fs.readFile(cert) };
  const requests = [];
  function observe(req, endpoint) {
    const url = new URL(req.url, 'https://fixture.invalid');
    const name = url.searchParams.get('case');
    requests.push({ name, endpoint, path: url.pathname, headers: req.headers });
    return name;
  }
  const binaryServer = https.createServer(tls, (req, res) => {
    observe(req, 'binary');
    res.writeHead(200, { 'Content-Type': 'application/octet-stream' }).end(bytes);
  });
  servers.push(binaryServer);
  await new Promise(resolve => binaryServer.listen(0, '127.0.0.1', resolve));
  const binaryOrigin = `https://127.0.0.1:${binaryServer.address().port}`;
  const apiServer = https.createServer(tls, (req, res) => {
    const name = observe(req, 'api');
    if (name === 'redirect') {
      res.writeHead(302, { Location: `${binaryOrigin}/binary?case=${name}` }).end();
    } else if (name === 'missing') {
      res.writeHead(404).end('missing');
    } else if (name === 'unavailable' || (name === 'retry' && requests.filter(r => r.name === name).length === 1)) {
      res.writeHead(504).end('temporary failure');
    } else if (name === 'metadata' || req.headers.accept !== 'application/octet-stream') {
      res.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify({ id: 320198486, name: `${archiveName}.tar.gz` }));
    } else if (name === 'corrupt') {
      res.writeHead(200).end(Buffer.alloc(bytes.length));
    } else {
      res.writeHead(200, { 'Content-Type': 'application/octet-stream' }).end(bytes);
    }
  });
  servers.push(apiServer);
  await new Promise(resolve => apiServer.listen(0, '127.0.0.1', resolve));
  const origin = `https://127.0.0.1:${apiServer.address().port}`;

  async function scenario(name, { exitCode = 0, apiRequests = 1, binaryRequests = 0,
    downloadFails = false, verificationFails = false, auditExit = 0 } = {}) {
    const folder = path.join(root, name);
    const fixtureRepo = path.join(folder, 'fixture repo');
    const bin = path.join(folder, 'bin');
    const temporary = path.join(folder, 'temporary');
    const curlHome = path.join(folder, 'curl-home');
    for (const dir of [path.join(fixtureRepo, 'scripts'), bin, temporary, curlHome]) await fs.mkdir(dir, { recursive: true });
    await fs.writeFile(path.join(curlHome, '.curlrc'), '');
    const trace = path.join(folder, 'trace.txt');
    const curlArgsFile = path.join(folder, 'curl-args.txt');
    const localUrl = `${origin}${new URL(apiUrl).pathname}?case=${name}`;
    // The complete production shell is executed. Its only test substitutions
    // are the origin/case and the digest of our locally authored archive.
    const script = production.replace(apiUrl, localUrl).replace(archiveSha256, fixtureHash);
    assert.equal(script.replace(localUrl, apiUrl).replace(fixtureHash, archiveSha256), production);
    const scriptPath = path.join(fixtureRepo, 'scripts/check-dependency-security.sh');
    await fs.writeFile(scriptPath, script);
    async function stub(name, body) {
      await fs.writeFile(path.join(bin, name), `#!/usr/bin/env bash\nset -euo pipefail\n${body}\n`, { mode: 0o755 });
    }
    // Pre-download source/runtime gates are recorded stubs, not claims that
    // those expensive gates were executed by this transport regression.
    await stub('cargo', `printf 'cargo:%s\\n' "$*" >> "$ANNEX_DOWNLOAD_TRACE"\nif [ "$1" = metadata ]; then printf '{"packages":[],"resolve":{"nodes":[]}}\\n'; fi`);
    await stub('node', `printf 'node:%s\\n' "$*" >> "$ANNEX_DOWNLOAD_TRACE"`);
    await stub('dbus-run-session', `printf 'dbus\\n' >> "$ANNEX_DOWNLOAD_TRACE"\nexec "$@"`);
    await stub('curl', `printf 'curl\\n' >> "$ANNEX_DOWNLOAD_TRACE"\nprintf '%s\\n' "$@" > "$ANNEX_CURL_ARGS"\nexec ${shellQuote(tools.curl)} "$@"`);
    await stub('sha256sum', `printf 'hash\\n' >> "$ANNEX_DOWNLOAD_TRACE"\nexec ${shellQuote(tools.sha256sum)} "$@"`);
    await stub('tar', `printf 'tar\\n' >> "$ANNEX_DOWNLOAD_TRACE"\nexec ${shellQuote(tools.tar)} "$@"`);
    const env = { ...process.env, PATH: `${bin}${path.delimiter}${process.env.PATH}`,
      TMPDIR: temporary, CURL_HOME: curlHome, CURL_CA_BUNDLE: cert,
      NO_PROXY: '127.0.0.1,localhost', no_proxy: '127.0.0.1,localhost',
      ANNEX_DOWNLOAD_TRACE: trace, ANNEX_CURL_ARGS: curlArgsFile, ANNEX_FIXTURE_AUDIT_EXIT: String(auditExit) };
    let result;
    try {
      result = { ...(await execute('bash', [scriptPath], { env, timeout: 25_000 })), code: 0 };
    } catch (error) {
      assert.equal(error.killed, false, 'the production shell must finish, not hit the test timeout');
      result = error;
    }
    assert.equal(result.code, exitCode, result.stderr);
    const calls = (await fs.readFile(trace, 'utf8')).trim().split('\n');
    const curlArgs = (await fs.readFile(curlArgsFile, 'utf8')).trim().split('\n');
    const downloadedArchive = curlArgs.at(-1);
    assert.equal(path.basename(downloadedArchive), `${archiveName}.tar.gz`);
    assert.equal(path.dirname(path.dirname(downloadedArchive)), temporary);
    assert.deepEqual(curlArgs, ['--fail', '--location', '--retry', '3', '--max-time', '120',
      '--header', 'Accept: application/octet-stream', localUrl, '--output', downloadedArchive]);
    assert.equal(calls.filter(c => c === 'curl').length, 1, 'curl owns its unchanged retry budget');
    assert.equal(calls.filter(c => c.startsWith('cargo:')).length, 5);
    assert.equal(calls.filter(c => c.startsWith('node:')).length, 3);
    assert.equal(calls.filter(c => c === 'dbus').length, 1);
    const transportCalls = calls.filter(c => c === 'curl' || c === 'hash' || c === 'tar' || c.startsWith('deny:'));
    if (downloadFails) {
      assert.deepEqual(transportCalls, ['curl']);
      assert.match(result.stderr, /curl: \(22\)/);
    } else if (verificationFails) {
      assert.deepEqual(transportCalls, ['curl', 'hash']);
      assert.match(result.stdout, /: FAILED/);
    } else {
      assert.deepEqual(transportCalls, ['curl', 'hash', 'tar', 'deny:--version',
        `deny:--all-features --locked check --metadata-path ${path.join(path.dirname(downloadedArchive), 'metadata.json')}`]);
      assert.match(result.stdout, /: OK\n/);
      assert.match(result.stdout, /cargo-deny 0\.18\.6/);
    }
    assert.deepEqual(await fs.readdir(temporary), [], 'the actual EXIT trap removes every gate file');
    await assert.rejects(fs.stat(path.dirname(downloadedArchive)), { code: 'ENOENT' });
    const observed = requests.filter(r => r.name === name);
    assert.equal(observed.filter(r => r.endpoint === 'api').length, apiRequests);
    assert.equal(observed.filter(r => r.endpoint === 'binary').length, binaryRequests);
    for (const row of observed) {
      assert.equal(row.headers.accept, 'application/octet-stream');
      assert.equal(row.headers.authorization, undefined);
      assert.equal(row.headers.cookie, undefined);
      if (row.endpoint === 'api') assert.equal(row.path, new URL(apiUrl).pathname);
    }
  }
  await t.test('API 200 verifies the hash before real extraction and fixture execution', () => scenario('direct'));
  await t.test('API 302 follows another HTTPS origin with binary Accept', () => scenario('redirect', { binaryRequests: 1 }));
  await t.test('a transient HTTP failure retries with the same pin and Accept', () => scenario('retry', { apiRequests: 2 }));
  await t.test('HTTP 404 fails immediately without hashing, extracting or executing', () => scenario('missing', { exitCode: 22, downloadFails: true }));
  await t.test('HTTP 504 exhausts exactly four requests and preserves curl exit 22', () => scenario('unavailable', { exitCode: 22, apiRequests: 4, downloadFails: true }));
  await t.test('API metadata JSON fails the real hash check before extraction or execution', () => scenario('metadata', { exitCode: 1, verificationFails: true }));
  await t.test('same-size corrupt bytes fail the real hash check before extraction or execution', () => scenario('corrupt', { exitCode: 1, verificationFails: true }));
  await t.test('the audit program failure remains the gate exit status after cleanup', () => scenario('audit-failure', { exitCode: 37, auditExit: 37 }));
});
