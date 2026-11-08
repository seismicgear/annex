import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import https from 'node:https';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

const execute = promisify(execFile);
const source = new URL('../prepare-desktop-assets.mjs', import.meta.url).href;

test('real TLS downloads recover bounded transport failures and preserve integrity failures', async t => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'annex-download-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const key = path.join(root, 'fixture-key.pem');
  const cert = path.join(root, 'fixture-cert.pem');
  const config = path.join(root, 'openssl.cnf');
  await fs.writeFile(config, '[req]\ndistinguished_name=dn\n[dn]\n');
  // A disposable loopback certificate, trusted only by the child test process.
  // Production fetch keeps its ordinary certificate validation unchanged.
  await execute('openssl', ['req', '-config', config, '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
    '-subj', '/CN=localhost', '-addext', 'subjectAltName=IP:127.0.0.1', '-keyout', key, '-out', cert]);
  const bytes = Buffer.from('pinned native resource, served over actual TLS');
  const pin = { bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
  const counts = new Map();
  const server = https.createServer({ key: await fs.readFile(key), cert: await fs.readFile(cert) }, (req, res) => {
    const count = (counts.get(req.url) ?? 0) + 1; counts.set(req.url, count);
    if (req.url === '/unavailable' || (req.url === '/retry-status' && count === 1)) {
      res.writeHead(503).end('temporarily unavailable');
    } else if (req.url === '/missing') {
      res.writeHead(404).end('missing');
    } else if (req.url === '/corrupt') {
      res.writeHead(200).end(Buffer.alloc(bytes.length));
    } else if (req.url === '/retry-socket' && count === 1) {
      res.writeHead(200, { 'Content-Length': bytes.length }); res.write(bytes.subarray(0, 8));
      setTimeout(() => res.destroy(), 30);
    } else if (req.url === '/retry-timeout' && count === 1) {
      // No response headers: the actual fetch deadline, not a simulated error.
    } else if (req.url === '/retry-stalled-body' && count === 1) {
      res.writeHead(200, { 'Content-Length': bytes.length }); res.write(bytes.subarray(0, 8));
    } else {
      res.writeHead(200).end(bytes);
    }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `https://127.0.0.1:${server.address().port}`;
  async function download(name, { fails, requests, scheme = 'https' } = {}) {
    const file = path.join(root, name);
    const code = `const {download}=await import(process.argv[1]); await download(process.argv[2],process.argv[3],JSON.parse(process.argv[4]),{timeoutMs:1000,retryDelayMs:1});`;
    const run = execute(process.execPath, ['--input-type=module', '-e', code, source,
      origin.replace('https:', `${scheme}:`) + '/' + name, file, JSON.stringify(pin)],
    { env: { ...process.env, NODE_EXTRA_CA_CERTS: cert }, timeout: 15_000 });
    if (fails) {
      await assert.rejects(run, fails);
      await assert.rejects(fs.stat(file), { code: 'ENOENT' });
    } else {
      await run;
      assert.deepEqual(await fs.readFile(file), bytes);
    }
    assert.equal(counts.get('/' + name) ?? 0, requests);
    assert.deepEqual((await fs.readdir(root)).filter(name => name.endsWith('.part')), []);
  }
  try {
    await t.test('transient HTTP 503 retries then verifies the real downloaded bytes', () => download('retry-status', { requests: 2 }));
    await t.test('partial response is discarded before a complete retry', () => download('retry-socket', { requests: 2 }));
    await t.test('stalled response headers are bounded and retried', () => download('retry-timeout', { requests: 2 }));
    await t.test('stalled partial body is bounded and discarded before retry', () => download('retry-stalled-body', { requests: 2 }));
    await t.test('transient outage stops after exactly three requests', () => download('unavailable', { requests: 3, fails: /after 3 attempt\(s\).*HTTP 503/s }));
    await t.test('permanent HTTP error fails immediately', () => download('missing', { requests: 1, fails: /after 1 attempt\(s\).*HTTP 404/s }));
    await t.test('hash mismatch is never retried or installed', () => download('corrupt', { requests: 1, fails: /verification failed/ }));
    await t.test('plaintext source is refused before any request', () => download('plaintext', { requests: 0, scheme: 'http', fails: /require HTTPS/ }));
  } finally {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
});

test('Piper archive selection uses the public binary API without weakening verification', async t => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'annex-piper-download-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const key = path.join(root, 'fixture-key.pem');
  const cert = path.join(root, 'fixture-cert.pem');
  const config = path.join(root, 'openssl.cnf');
  await fs.writeFile(config, '[req]\ndistinguished_name=dn\n[dn]\n');
  await execute('openssl', ['req', '-config', config, '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
    '-subj', '/CN=localhost', '-addext', 'subjectAltName=IP:127.0.0.1', '-keyout', key, '-out', cert]);
  const tls = { key: await fs.readFile(key), cert: await fs.readFile(cert) };
  const bytes = Buffer.from('exact Piper archive fixture delivered over real TLS');
  const fixturePin = { bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
  const productionPins = JSON.parse(await fs.readFile(new URL('../desktop-assets.json', import.meta.url), 'utf8'));
  const requests = [];
  function record(req, endpoint) {
    const url = new URL(req.url, 'https://fixture.invalid');
    const name = url.searchParams.get('case') ?? url.pathname.split('/')[2];
    requests.push({ name, endpoint, path: url.pathname, headers: req.headers });
    return name;
  }
  const binaryServer = https.createServer(tls, (req, res) => {
    record(req, 'binary');
    res.writeHead(200, { 'Content-Type': 'application/octet-stream' }).end(bytes);
  });
  const plaintextServer = http.createServer((req, res) => {
    record(req, 'plaintext');
    res.writeHead(200).end(bytes);
  });
  await new Promise(resolve => binaryServer.listen(0, '127.0.0.1', resolve));
  await new Promise(resolve => plaintextServer.listen(0, '127.0.0.1', resolve));
  const binaryOrigin = `https://127.0.0.1:${binaryServer.address().port}`;
  const plaintextOrigin = `http://127.0.0.1:${plaintextServer.address().port}`;
  const apiServer = https.createServer(tls, (req, res) => {
    const name = record(req, 'api');
    if (req.url.startsWith('/release/')) {
      res.writeHead(200).end(bytes);
    } else if (name === 'redirect' || name === 'linux-redirect' || name === 'downgrade') {
      const destination = name === 'downgrade' ? plaintextOrigin : binaryOrigin;
      res.writeHead(302, { Location: `${destination}/binary?case=${name}` }).end();
    } else if (name === 'metadata' || req.headers.accept !== 'application/octet-stream') {
      res.writeHead(200, { 'Content-Type': 'application/json' })
        .end(JSON.stringify({ id: 135529797, name: 'piper_windows_amd64.zip' }));
    } else if (name === 'corrupt') {
      res.writeHead(200).end(Buffer.alloc(bytes.length));
    } else if (name === 'retry' && requests.filter(row => row.name === name).length === 1) {
      res.writeHead(503).end('temporarily unavailable');
    } else {
      res.writeHead(200, { 'Content-Type': 'application/octet-stream' }).end(bytes);
    }
  });
  await new Promise(resolve => apiServer.listen(0, '127.0.0.1', resolve));
  const origin = `https://127.0.0.1:${apiServer.address().port}`;
  async function archive(name, { platform = 'win32-x64', fails, apiRequests = 1,
    binaryRequests = 0, plaintextRequests = 0, cached, legacyFallback = false } = {}) {
    const pins = structuredClone(productionPins);
    // Keep the production platform selection and Accept contract; substitute
    // only the loopback origin and fixture bytes for an actual TLS transfer.
    pins.piper.base_url = `${origin}/release/${name}/`;
    const pin = Object.assign(pins.piper.archives[platform], fixturePin);
    // The fallback remains supported, but neither active archive uses it.
    if (legacyFallback) { delete pin.url; delete pin.accept; }
    if (pin.url) {
      pin.url = `${origin}${new URL(pin.url).pathname}?case=${name}`;
    }
    const cache = path.join(root, name);
    await fs.mkdir(cache);
    const file = path.join(cache, pin.name);
    if (cached !== undefined) await fs.writeFile(file, cached);
    // This is the same archive helper called directly by preparePiper before
    // extraction; no fetch substitute or simulated response is involved.
    const code = `const {downloadPiperArchive}=await import(process.argv[1]); const file=await downloadPiperArchive(JSON.parse(process.argv[2]),process.argv[3],process.argv[4]); process.stdout.write(JSON.stringify({file}));`;
    const run = execute(process.execPath, ['--input-type=module', '-e', code, source,
      JSON.stringify(pins), platform, cache],
    { env: { ...process.env, NODE_EXTRA_CA_CERTS: cert }, timeout: 15_000 });
    if (fails) {
      await assert.rejects(run, fails);
      if (cached === undefined) await assert.rejects(fs.stat(file), { code: 'ENOENT' });
      else assert.deepEqual(await fs.readFile(file), cached);
    } else {
      const result = await run;
      assert.equal(JSON.parse(result.stdout).file, file);
      assert.deepEqual(await fs.readFile(file), bytes);
    }
    const observed = requests.filter(row => row.name === name);
    assert.equal(observed.filter(row => row.endpoint === 'api').length, apiRequests);
    assert.equal(observed.filter(row => row.endpoint === 'binary').length, binaryRequests);
    assert.equal(observed.filter(row => row.endpoint === 'plaintext').length, plaintextRequests);
    for (const row of observed) {
      assert.equal(row.headers.accept, legacyFallback ? '*/*' : 'application/octet-stream');
      assert.equal(row.headers.authorization, undefined);
      assert.equal(row.headers.cookie, undefined);
      if (row.endpoint === 'api') assert.equal(row.path, legacyFallback ? `/release/${name}/${pin.name}`
        : `/repos/rhasspy/piper/releases/assets/${platform === 'win32-x64' ? '135529797' : '135532855'}`);
    }
    const expectedFiles = !fails || cached !== undefined ? [pin.name] : [];
    assert.deepEqual(await fs.readdir(cache), expectedFiles);
  }
  try {
    await t.test('Windows API 200 sends binary Accept and verifies bytes before cache installation', () => archive('direct'));
    await t.test('Windows API 302 follows a different HTTPS origin and verifies the archive', () => archive('redirect', { binaryRequests: 1 }));
    await t.test('API metadata JSON is terminal and leaves no archive or partial file', () => archive('metadata', { fails: /after 1 attempt\(s\).*verification failed/s }));
    await t.test('same-size corrupt API binary is terminal and cleaned up', () => archive('corrupt', { fails: /after 1 attempt\(s\).*verification failed/s }));
    await t.test('HTTPS redirect to plaintext is refused without installing its bytes', () => archive('downgrade', { plaintextRequests: 1, fails: /after 1 attempt\(s\).*retain HTTPS/s }));
    await t.test('API retry retains binary Accept and the same pin', () => archive('retry', { apiRequests: 2 }));
    await t.test('valid cached Windows archive is verified without a request', () => archive('cached', { cached: bytes, apiRequests: 0 }));
    await t.test('corrupt cached Windows archive is refused without replacement or a request', () => archive('cached-corrupt', { cached: Buffer.alloc(bytes.length), apiRequests: 0, fails: /verification failed/ }));
    await t.test('truncated cached Windows archive is refused without replacement or a request', () => archive('cached-short', { cached: bytes.subarray(0, -1), apiRequests: 0, fails: /verification failed/ }));
    await t.test('Linux API 200 sends binary Accept and verifies the pinned archive', () => archive('linux-direct', { platform: 'linux-x64' }));
    await t.test('Linux API 302 follows a different HTTPS origin and verifies the archive', () => archive('linux-redirect', { platform: 'linux-x64', binaryRequests: 1 }));
    await t.test('archive without an override keeps the legacy base URL and default Accept', () => archive('legacy-fallback', { platform: 'linux-x64', legacyFallback: true }));
  } finally {
    for (const server of [apiServer, binaryServer, plaintextServer]) server.closeAllConnections();
    await Promise.all([apiServer, binaryServer, plaintextServer].map(server => new Promise(resolve => server.close(resolve))));
  }
});
