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
