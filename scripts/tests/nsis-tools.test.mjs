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
const digest = (bytes, algorithm = 'sha256') => createHash(algorithm).update(bytes).digest('hex');
const pins = JSON.parse(await fs.readFile(new URL('../nsis-tools.json', import.meta.url), 'utf8'));
const source = new URL('../prepare-nsis-tools.mjs', import.meta.url).href;
const downloader = new URL('../prepare-desktop-assets.mjs', import.meta.url).href;

test('NSIS compatibility receipt binds the existing CLI and exact upstream required files', async () => {
  const cli = JSON.parse(await fs.readFile(new URL('../../tools/tauri-cli/package.json', import.meta.url), 'utf8'));
  const config = JSON.parse(await fs.readFile(new URL('../../crates/annex-desktop/tauri.conf.json', import.meta.url), 'utf8'));
  assert.equal(cli.dependencies['@tauri-apps/cli'], pins.tauri.cliVersion);
  assert.equal(pins.tauri.cliVersion, '2.11.2');
  assert.equal(pins.tauri.bundlerVersion, '2.9.2');
  assert.equal(pins.tauri.sourceCommit, '499df79be65ef8c0670abc0207cd9e37b55d8491');
  assert.equal(config.bundle.useLocalToolsDir ?? false, false);
  assert.equal(config.build.beforeBundleCommand, undefined);
  assert.equal(pins.toolset.url, 'https://api.github.com/repos/tauri-apps/binary-releases/releases/assets/317051073');
  assert.equal(pins.plugin.url, 'https://api.github.com/repos/tauri-apps/nsis-tauri-utils/releases/assets/345882097');
  for (const pin of [pins.toolset, pins.plugin]) {
    assert.equal(pin.accept, 'application/octet-stream');
    assert.match(pin.sha256, /^[a-f0-9]{64}$/);
    assert.match(pin.upstreamSha1, /^[a-f0-9]{40}$/);
    assert.ok(pin.bytes > 0);
  }
  assert.deepEqual(pins.requiredFiles, [
    'makensis.exe', 'Bin/makensis.exe', 'Stubs/lzma-x86-unicode', 'Stubs/lzma_solid-x86-unicode',
    'Plugins/x86-unicode/additional/nsis_tauri_utils.dll', 'Include/MUI2.nsh', 'Include/FileFunc.nsh',
    'Include/x64.nsh', 'Include/nsDialogs.nsh', 'Include/WinMessages.nsh', 'Include/Win/COM.nsh',
    'Include/Win/Propkey.nsh', 'Include/Win/RestartManager.nsh',
  ]);
  for (const workflow of ['ci.yml', 'package-proof.yml']) {
    const text = await fs.readFile(new URL(`../../.github/workflows/${workflow}`, import.meta.url), 'utf8');
    const versions = [...text.matchAll(/cargo install tauri-cli@([^\s]+)/g)].map(match => match[1]);
    assert.ok(versions.length > 0);
    assert.ok(versions.every(version => version === pins.tauri.cliVersion));
  }
});

test('complete NSIS cache uses actual TLS, verified ZIP bytes and missing-only publication', async t => {
  const temporary = await fs.mkdtemp(path.join(os.tmpdir(), 'annex-nsis-proof-'));
  t.after(() => fs.rm(temporary, { recursive: true, force: true }));
  const key = path.join(temporary, 'key.pem'), cert = path.join(temporary, 'cert.pem');
  const openssl = path.join(temporary, 'openssl.cnf');
  await fs.writeFile(openssl, '[req]\ndistinguished_name=dn\n[dn]\n');
  await execute('openssl', ['req', '-config', openssl, '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
    '-subj', '/CN=localhost', '-addext', 'subjectAltName=IP:127.0.0.1', '-keyout', key, '-out', cert]);
  // Harmless archive members, independently encoded by Python's standard ZIP
  // library. No downloaded Windows executable is launched by these fixtures.
  const entries = pins.requiredFiles.filter(file => file !== pins.plugin.path)
    .map(file => ({ name: `${pins.toolset.root}/${file}`, value: `fixture tool: ${file}\n` }));
  entries.push({ name: `${pins.toolset.root}/Contrib/readme.txt`, value: 'additional pinned file\n', stored: true });
  const plugin = Buffer.from('harmless NSIS plugin fixture\n');
  let zipIndex = 0;
  async function makeZip(rows) {
    const output = path.join(temporary, `fixture-${zipIndex++}.zip`);
    const script = `import json,sys,zipfile\nrows=json.loads(sys.argv[2])\nwith zipfile.ZipFile(sys.argv[1],'w') as z:\n for row in rows:\n  i=zipfile.ZipInfo(row['name']);i.create_system=3;i.external_attr=(row.get('mode',0o100644)<<16);i.compress_type=zipfile.ZIP_STORED if row.get('stored') else zipfile.ZIP_DEFLATED\n  z.writestr(i,row.get('value','').encode())\n`;
    await execute('python3', ['-c', script, output, JSON.stringify(rows)]);
    return fs.readFile(output);
  }
  const archive = await makeZip(entries);
  const expected = new Map(entries.map(row => [row.name.slice(pins.toolset.root.length + 1), Buffer.from(row.value)]));
  expected.set(pins.plugin.path, plugin);
  const configurations = new Map();
  const requests = [];
  const tls = { key: await fs.readFile(key), cert: await fs.readFile(cert) };
  function request(req, endpoint) {
    const url = new URL(req.url, 'https://fixture.invalid');
    const name = url.searchParams.get('case');
    const kind = url.searchParams.get('kind');
    requests.push({ name, kind, endpoint, path: url.pathname, accept: req.headers.accept,
      authorization: req.headers.authorization, cookie: req.headers.cookie });
    return { name, kind, config: configurations.get(name) };
  }
  const binaries = https.createServer(tls, (req, res) => {
    const { kind, config } = request(req, 'binary');
    res.writeHead(200, { 'Content-Type': 'application/octet-stream' }).end(config[kind]);
  });
  await new Promise(resolve => binaries.listen(0, '127.0.0.1', resolve));
  const binaryOrigin = `https://127.0.0.1:${binaries.address().port}`;
  const api = https.createServer(tls, (req, res) => {
    const { name, kind, config } = request(req, 'api');
    if (config.mode === 'redirect') {
      res.writeHead(302, { Location: `${binaryOrigin}/binary?case=${name}&kind=${kind}` }).end();
    } else if (config.mode === 'missing' && kind === 'toolset') res.writeHead(404).end('missing');
    else if (config.mode === 'unavailable' || (config.mode === 'retry' && kind === 'toolset' && requests.filter(r => r.name === name && r.kind === kind).length === 1)) res.writeHead(504).end('gateway timeout');
    else if (config.mode === 'metadata' && kind === 'toolset') res.writeHead(200).end('{"id":317051073}');
    else if (config.mode === 'corrupt' && kind === 'plugin') res.writeHead(200).end(Buffer.alloc(plugin.length));
    else res.writeHead(200, { 'Content-Type': 'application/octet-stream' }).end(config[kind]);
  });
  await new Promise(resolve => api.listen(0, '127.0.0.1', resolve));
  const origin = `https://127.0.0.1:${api.address().port}`;
  async function writeCache(directory, values) {
    for (const [relative, value] of values) {
      const file = path.join(directory, relative);
      await fs.mkdir(path.dirname(file), { recursive: true }); await fs.writeFile(file, value);
    }
  }
  async function inventory(directory) {
    const result = {};
    async function walk(current, prefix = '') {
      let rows; try { rows = await fs.readdir(current, { withFileTypes: true }); } catch (error) { if (error.code === 'ENOENT') return; throw error; }
      for (const row of rows) {
        const file = path.join(current, row.name), relative = prefix + row.name;
        if (row.isSymbolicLink()) result[relative] = `link:${await fs.readlink(file)}`;
        else if (row.isDirectory()) await walk(file, relative + '/');
        else result[relative] = digest(await fs.readFile(file));
      }
    }
    await walk(directory); return result;
  }
  async function run(name, options = {}) {
    const config = { toolset: options.archive ?? archive, plugin, mode: options.mode ?? 'direct' };
    configurations.set(name, config);
    const base = path.join(temporary, name), tools = path.join(base, 'tauri'), cache = path.join(base, 'downloads');
    await fs.mkdir(base, { recursive: true });
    const fixturePins = structuredClone(pins);
    for (const kind of ['toolset', 'plugin']) {
      const data = config[kind], pin = fixturePins[kind];
      pin.url = `${origin}${new URL(pin.url).pathname}?case=${name}&kind=${kind}`;
      Object.assign(pin, { bytes: data.length, sha256: digest(data), upstreamSha1: digest(data, 'sha1') });
    }
    if (options.wrongSha1) fixturePins.plugin.upstreamSha1 = '0'.repeat(40);
    if (options.prepare) await options.prepare({ base, tools, cache, fixturePins });
    const before = await inventory(tools);
    const code = `const {prepareNsisCache}=await import(process.argv[1]); const {download}=await import(process.argv[2]); await prepareNsisCache({pins:JSON.parse(process.argv[3]),toolsDirectory:process.argv[4],archiveDirectory:process.argv[5],download:(...args)=>download(...args,{timeoutMs:1000,retryDelayMs:1})});`;
    const command = () => execute(process.execPath, ['--input-type=module', '-e', code, source, downloader,
      JSON.stringify(fixturePins), tools, cache], { env: { ...process.env, NODE_EXTRA_CA_CERTS: cert }, timeout: 15000 });
    if (options.fails) {
      await assert.rejects(command(), options.fails);
      assert.deepEqual(await inventory(tools), before, 'failed preparation changed existing tools');
    } else {
      await command();
      for (const [relative, bytes] of expected) assert.deepEqual(await fs.readFile(path.join(tools, 'NSIS', relative)), bytes);
      for (const [relative, sha] of Object.entries(before)) assert.equal((await inventory(tools))[relative], sha, `existing cache entry changed: ${relative}`);
      const receipts = (await fs.readdir(cache)).filter(file => /^verified-[a-f0-9]{64}\.json$/.test(file));
      assert.equal(receipts.length, 1);
      const receipt = JSON.parse(await fs.readFile(path.join(cache, receipts[0]), 'utf8'));
      assert.equal(receipt.files.length, expected.size);
      assert.deepEqual(receipt.tauri, pins.tauri);
      assert.deepEqual(receipt.files.map(row => row.path).sort(), [...expected.keys()].sort());
      for (const row of receipt.files) assert.equal(row.sha256, digest(expected.get(row.path)));
      if (options.repeat) {
        const requestCount = requests.length, firstInventory = await inventory(tools);
        await command();
        assert.equal(requests.length, requestCount, 'verified cached archives should avoid new transfers');
        assert.deepEqual(await inventory(tools), firstInventory);
      }
    }
    const observed = requests.filter(row => row.name === name);
    if (options.requestCount !== undefined) assert.equal(observed.length, options.requestCount);
    for (const row of observed) {
      assert.equal(row.accept, 'application/octet-stream'); assert.equal(row.authorization, undefined); assert.equal(row.cookie, undefined);
      if (row.endpoint === 'api') assert.equal(row.path, new URL(pins[row.kind].url).pathname);
    }
    for (const directory of [tools, cache]) {
      const names = await fs.readdir(directory).catch(error => error.code === 'ENOENT' ? [] : Promise.reject(error));
      assert.ok(names.every(file => !file.startsWith('.annex-nsis-stage-') && !file.endsWith('.part')));
    }
  }
  try {
    await t.test('fresh API200 stages every ZIP member and plugin; verified reuse needs no network', () => run('fresh', { requestCount: 2, repeat: true }));
    await t.test('API302 across HTTPS origins preserves Accept and pinned contents', () => run('redirect', { mode: 'redirect', requestCount: 4 }));
    await t.test('partial upstream cache from plugin failure is completed without replacing any tool or unrelated file', () => run('partial', { requestCount: 2,
      prepare: async ({ tools }) => { const partial = new Map(expected); partial.delete(pins.plugin.path); await writeCache(path.join(tools, 'NSIS'), partial); await writeCache(tools, new Map([['unrelated.txt', Buffer.from('retain')], ['NSIS/custom.txt', Buffer.from('retain custom')]])); } }));
    await t.test('transient504 is retried with unchanged binary headers and pins', () => run('retry', { mode: 'retry', requestCount: 3 }));
    await t.test('persistent504 stops after the existing three attempts without installing tools', () => run('unavailable', { mode: 'unavailable', requestCount: 3, fails: /after 3 attempt\(s\).*HTTP 504/s }));
    await t.test('404 is terminal before extraction', () => run('missing', { mode: 'missing', requestCount: 1, fails: /after 1 attempt\(s\).*HTTP 404/s }));
    await t.test('metadata JSON is rejected by the original archive pin', () => run('metadata', { mode: 'metadata', requestCount: 1, fails: /verification failed/ }));
    await t.test('same-size corrupt plugin leaves the complete Tauri cache untouched', () => run('corrupt', { mode: 'corrupt', requestCount: 2, fails: /verification failed/ }));
    await t.test('upstream plugin SHA1 must also match the Tauri source contract', () => run('sha1', { wrongSha1: true, requestCount: 2, fails: /Pinned NSIS plugin integrity failed/ }));
    await t.test('different existing tool bytes are rejected before any missing file is published', () => run('cache-mismatch', { requestCount: 2, fails: /cache differs from pinned toolset/,
      prepare: ({ tools }) => writeCache(path.join(tools, 'NSIS'), new Map([['makensis.exe', Buffer.from('different existing tool')], ['unrelated.txt', Buffer.from('retain')]])) }));
    await t.test('bad cached archive is rejected without redownloading or replacing it', () => run('bad-archive-cache', { requestCount: 0, fails: /verification failed/,
      prepare: async ({ cache }) => { await fs.mkdir(cache); await fs.writeFile(path.join(cache, pins.toolset.name), Buffer.alloc(archive.length)); } }));
    await t.test('existing cache directory symlink is rejected before network access', () => run('directory-link', { requestCount: 0, fails: /Unsafe NSIS cache directory/,
      prepare: async ({ base, tools }) => { const outside = path.join(base, 'outside'); await fs.mkdir(outside); await fs.symlink(outside, tools); } }));
    await t.test('existing tool symlink is rejected without changing its target', () => run('file-link', { requestCount: 2, fails: /Unsafe NSIS cache link/,
      prepare: async ({ base, tools }) => { const outside = path.join(base, 'outside.txt'); await fs.writeFile(outside, 'retain'); await fs.mkdir(path.join(tools, 'NSIS'), { recursive: true }); await fs.symlink(outside, path.join(tools, 'NSIS', 'makensis.exe')); } }));
    const unsafe = [
      ['traversal', { name: `${pins.toolset.root}/../../outside.txt`, value: 'escape' }],
      ['absolute', { name: '/outside.txt', value: 'escape' }],
      ['backslash', { name: `${pins.toolset.root}/..\\outside.txt`, value: 'escape' }],
      ['alternate-stream', { name: `${pins.toolset.root}/bad:stream`, value: 'escape' }],
      ['symlink-entry', { name: `${pins.toolset.root}/link`, value: '../../outside', mode: 0o120777 }],
      ['case-alias', { name: `${pins.toolset.root}/MAKENSIS.EXE`, value: 'alias' }],
      ['wrong-root', { name: 'other-root/file', value: 'wrong' }],
    ];
    for (const [name, entry] of unsafe) await t.test(`verified archive with ${name} is rejected before cache publication`, async () => run(name, {
      archive: await makeZip([...entries, entry]), requestCount: 2, fails: /Unsafe NSIS path|links or special|case-aliased|Unexpected NSIS ZIP root/ }));
    await t.test('missing upstream required file is rejected as an incomplete toolset', async () => run('missing-required', {
      archive: await makeZip(entries.filter(row => !row.name.endsWith('/Include/MUI2.nsh'))), requestCount: 2, fails: /Incomplete pinned NSIS toolset/ }));
    const badCrc = Buffer.from(archive);
    const central = badCrc.indexOf(Buffer.from([0x50, 0x4b, 0x01, 0x02]));
    badCrc.writeUInt32LE((badCrc.readUInt32LE(central + 16) ^ 1) >>> 0, central + 16);
    badCrc.writeUInt32LE(badCrc.readUInt32LE(central + 16), badCrc.readUInt32LE(central + 42) + 14);
    await t.test('ZIP member CRC is independently checked after archive verification', () => run('bad-crc', { archive: badCrc, requestCount: 2, fails: /ZIP entry integrity failed/ }));
  } finally {
    for (const server of [api, binaries]) server.closeAllConnections();
    await Promise.all([api, binaries].map(server => new Promise(resolve => server.close(resolve))));
  }
});
