// Run with Array's installed Electron runtime. This exercises real native pipes,
// the production Rust bridge, Array's actual broker and OS-backed safeStorage.
// Account identity and recovery keys are isolated synthetic acceptance inputs.
import fs from 'node:fs/promises';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { spawn, execFileSync } from 'node:child_process';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { app, safeStorage } from 'electron';

const { values: args } = parseArgs({ args: process.argv.slice(2), options: {
  'array-root': { type: 'string' }, 'native-client': { type: 'string' }, output: { type: 'string' },
} });
const output = path.resolve(args.output);
app.setPath('userData', path.join(output, 'electron-profile'));
await app.whenReady();
try {
  assert.equal(safeStorage.isEncryptionAvailable(), true, 'Native OS credential encryption is required');
  const arrayRoot = path.resolve(args['array-root']);
  const { CredentialWallet } = await import(pathToFileURL(path.join(arrayRoot, 'apps/desktop/core/credential-wallet.mjs')));
  const { WalletBridge } = await import(pathToFileURL(path.join(arrayRoot, 'apps/desktop/core/wallet-bridge.mjs')));
  const require = createRequire(new URL('../zk/package.json', import.meta.url));
  const poseidon = await require('circomlibjs').buildPoseidon();
  const identity = { sk: '12345', roleCode: 1, nodeId: 0,
    commitmentHex: BigInt(poseidon.F.toString(poseidon([0x12345n, 1n, 0n]))).toString(16).padStart(64, '0'),
    pseudonymId: null, serverSlug: 'array-contract-acceptance', createdAt: '2026-10-20T00:00:00.000Z' };
  await fs.mkdir(output, { recursive: true });
  const input = path.join(output, 'synthetic-identity.json');
  await fs.writeFile(input, JSON.stringify(identity), { mode: 0o600, flag: 'wx' });
  const profile = path.join(output, 'annex-profile');
  const getIdentity = () => ({ origin: 'https://store.kuykendall.industries', userId: 'isolated-annex-acceptance', epoch: 1, label: 'Synthetic contract account' });
  const makeWallet = () => new CredentialWallet({ root: output, safeStorage, getIdentity });
  let wallet = makeWallet();
  const view = await wallet.list();
  await wallet.pair({ context: view.context, appId: 'annex', paired: true });
  async function run(mode) {
    const receiptFile = path.join(output, `${mode}.json`);
    const session = mode === 'offline' ? null : await wallet.prepareApp('annex');
    const env = { ...process.env };
    if (session) env.KI_ARRAY_WALLET_PROTOCOL = 'stdio-v1';
    else delete env.KI_ARRAY_WALLET_PROTOCOL;
    const child = spawn(path.resolve(args['native-client']), [mode, profile, input, receiptFile],
      { env, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
    // Never retain/log protocol frames or secret-bearing diagnostics.
    child.stderr.resume();
    const connection = session ? new WalletBridge(wallet).attach(session, child) : null;
    if (!session) { child.stdin.end(); child.stdout.resume(); }
    const timer = setTimeout(() => child.kill(), 45000);
    try {
      const code = await new Promise((resolve, reject) => { child.once('exit', resolve); child.once('error', reject); });
      const receipt = JSON.parse(await fs.readFile(receiptFile, 'utf8'));
      assert.equal(code, 0, receipt.error || 'Native contract client failed');
      assert.equal(receipt.passed, true);
      return receipt;
    } finally { clearTimeout(timer); connection?.close(); }
  }
  const saved = await run('save');
  const offline = await run('offline');
  wallet.closeConnections();
  // New broker instance reopens the actual encrypted persisted wallet.
  wallet = makeWallet();
  assert.equal((await wallet.list()).status, 'ready');
  const restored = await run('restore');
  const final = await wallet.list();
  assert.equal(final.apps.find(item => item.id === 'annex').paired, false);
  assert.equal(final.entries.length, 1, 'Disconnect must retain recovery data');
  const arrayCommit = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: arrayRoot, encoding: 'utf8', windowsHide: true }).trim();
  const receipt = { passed: true, platform: process.platform, arch: process.arch, arrayCommit,
    nativeEncryption: safeStorage.getSelectedStorageBackend?.() || 'OS native',
    scenarios: [saved, offline, restored],
    boundary: 'Production bridge and broker; synthetic account and identity; not installed application UI or cloud recovery.' };
  await fs.writeFile(path.join(output, 'receipt.json'), JSON.stringify(receipt, null, 2) + '\n');
  await fs.unlink(input);
  console.log(JSON.stringify(receipt));
  app.exit(0);
} catch (error) {
  console.error(error.message);
  app.exit(1);
}
