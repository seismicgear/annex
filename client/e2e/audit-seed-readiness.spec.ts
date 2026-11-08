import { expect, test, type Page } from '@playwright/test';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'vite';
import { nodePolyfills } from 'vite-plugin-node-polyfills';
import { stabilize } from './audit/nav';
import { SEED } from './audit/roles';
import { waitForSeedReadiness, type SeedAuthor } from './audit/seed-readiness';
import { expectDeleteConfirmation } from './message-actions';

// This mandatory functional test mounts the real MessageView with controlled
// history input. Native Date and browser timers are never mocked or advanced.
// The full audit setup still obtains its history from the real server/UI.
test.use({ viewport: { width: 390, height: 844 } });

const require = createRequire(import.meta.url);
const client = fileURLToPath(new URL('..', import.meta.url));
let temporary: string;
let javascript: string;
let stylesheet: string;

test.beforeAll(async () => {
  temporary = mkdtempSync(join(tmpdir(), 'annex-seed-readiness-proof-'));
  const entry = join(temporary, 'probe.js');
  writeFileSync(entry, `
    import React from ${JSON.stringify(require.resolve('react'))};
    import { createRoot } from ${JSON.stringify(require.resolve('react-dom/client'))};
    import { MessageView } from ${JSON.stringify(join(client, 'src/components/MessageView.tsx'))};
    import { useChannelsStore } from ${JSON.stringify(join(client, 'src/stores/channels.ts'))};
    import { useIdentityStore } from ${JSON.stringify(join(client, 'src/stores/identity.ts'))};
    import ${JSON.stringify(join(client, 'src/index.css'))};
    import ${JSON.stringify(join(client, 'src/App.css'))};
    const errors = [];
    const report = error => errors.push(String(error));
    window.addEventListener('error', event => report(event.error ?? event.message));
    window.addEventListener('unhandledrejection', event => report(event.reason));
    const root = createRoot(document.getElementById('root'), { onUncaughtError: report });
    useChannelsStore.setState({ activeChannelId: 'seed-channel', historyLoading: true,
      messages: [], channels: [], hasMoreMessages: false, loadingOlder: false });
    root.render(React.createElement('div', {
      className: 'chat-area', style: { width: '390px', height: '566px' },
    }, React.createElement(MessageView)));
    window.__annexSeedProof = {
      load: async options => {
        useChannelsStore.setState({ historyLoading: true });
        const response = await fetch('/__annex_seed_history__?' + new URLSearchParams(options));
        const history = await response.json();
        // Supply normal identity/history inputs, never canModify, Date or DOM
        // classes. MessageBubble owns its actual 60-second expiration logic.
        useIdentityStore.setState({ identity: { id: 'proof-identity', sk: 'proof',
          pseudonymId: history.identity, sessionToken: 'proof', commitmentHex: 'proof',
          roleCode: 0, nodeId: 'proof', serverSlug: 'proof', leafIndex: 0, createdAt: '' } });
        useChannelsStore.setState({ activeChannelId: history.channel, messages: history.messages,
          channels: [{ channel_id: history.channel, name: history.channel, federation_scope: 'Local' }],
          historyLoading: false, historyError: null, hasMoreMessages: false,
          loadingOlder: false, typingUsers: [], messageActionError: null });
      },
      read: () => ({ errors: [...errors], now: Date.now(),
        historyLoading: useChannelsStore.getState().historyLoading,
        messages: useChannelsStore.getState().messages.map(message => ({
          id: message.message_id, ageMilliseconds: Date.now() - Date.parse(message.created_at),
        })),
      }),
    };
  `);
  const result = await build({
    configFile: false, root: client, logLevel: 'silent',
    // Mirror the application's browser bootstrap for actual MessageView dependencies.
    plugins: [nodePolyfills({ include: ['buffer', 'stream', 'util', 'assert', 'process'] })],
    define: { 'process.env.NODE_ENV': JSON.stringify('production') },
    esbuild: { jsx: 'automatic' },
    resolve: { alias: { '@': join(client, 'src') } },
    build: { write: false, minify: false, lib: { entry, formats: ['iife'], name: 'AnnexSeedProof' } },
  });
  const outputs = (Array.isArray(result) ? result : [result]).flatMap(value => {
    if (!('output' in value)) throw new Error('Seed proof requires a completed bundle');
    return value.output;
  });
  javascript = outputs.filter(value => value.type === 'chunk').map(value => value.code).join('\n');
  stylesheet = outputs.flatMap(value => value.type === 'asset' && value.fileName.endsWith('.css')
    ? [typeof value.source === 'string' ? value.source : Buffer.from(value.source).toString('utf8')] : []).join('\n');
});

test.afterAll(() => { if (temporary) rmSync(temporary, { recursive: true, force: true }); });

interface SeedReading {
  errors: string[];
  now: number;
  historyLoading: boolean;
  messages: { id: string; ageMilliseconds: number }[];
}
type ProofWindow = Window & { __annexSeedProof?: {
  load: (options: Record<string, string>) => Promise<void>;
  read: () => SeedReading;
} };

async function openProbe(page: Page) {
  await page.route('**/api/usernames/visible', route => route.fulfill({
    json: { usernames: { founder: 'Founder', 'second-member': 'Second member' } },
  }));
  await page.route('**/__annex_seed_history__*', route => {
    const options = new URL(route.request().url()).searchParams;
    const created = new Date(Date.now() - Number(options.get('age') ?? 0)).toISOString();
    const texts = [SEED.messages.plain, SEED.messages.long, SEED.messages.replyParent,
      SEED.messages.editedAfter, SEED.messages.deleted, SEED.messages.reply, SEED.messages.fromOther];
    const messages = texts.map((content, index) => ({
      message_id: `seed-${index}`, channel_id: 'seed-channel', content,
      sender_pseudonym: index === 6 ? 'second-member' : 'founder', created_at: created,
      edited_at: index === 3 ? created : null, deleted_at: index === 4 ? created : null,
      reply_to_message_id: index === 5 ? 'seed-2' : null,
    }));
    const fresh = options.get('fresh') === 'yes';
    return route.fulfill({ json: {
      identity: options.get('author') ?? 'founder', channel: fresh ? SEED.channels.scratch : 'seed-channel',
      messages: fresh ? [{ ...messages[0], message_id: 'fresh-scratch', channel_id: SEED.channels.scratch,
        content: 'A new scratch message remains editable', sender_pseudonym: options.get('author') ?? 'founder',
        created_at: new Date().toISOString() }]
        : options.get('missing') === 'yes' ? messages.slice(0, 6) : messages,
    } });
  });
  await page.route('**/__annex_seed_probe__*', route => {
    const url = route.request().url();
    if (url.endsWith('.js')) return route.fulfill({ contentType: 'application/javascript; charset=utf-8', body: javascript });
    if (url.endsWith('.css')) return route.fulfill({ contentType: 'text/css; charset=utf-8', body: stylesheet });
    return route.fulfill({ contentType: 'text/html; charset=utf-8', body:
      '<!doctype html><meta charset="utf-8"><link rel="stylesheet" href="/__annex_seed_probe__.css">' +
      '<div id="root"></div><script src="/__annex_seed_probe__.js"></script>' });
  });
  await page.goto('/__annex_seed_probe__');
  await expect.poll(() => page.evaluate(() => Boolean((window as ProofWindow).__annexSeedProof))).toBe(true);
  await page.evaluate(() => document.fonts.ready);
}

const loadHistory = (page: Page, options: Record<string, string>) =>
  page.evaluate(options => (window as ProofWindow).__annexSeedProof!.load(options), options);
const readHistory = (page: Page) => page.evaluate(() => (window as ProofWindow).__annexSeedProof!.read());

test.afterEach(async ({ page }) => {
  const state = await readHistory(page);
  await test.info().attach('seed-proof-final-state', { body: JSON.stringify(state, null, 2), contentType: 'application/json' });
  expect(state.errors).toEqual([]);
});

for (const author of ['founder', 'second-member'] as const satisfies readonly SeedAuthor[]) {
  test(`${author} waits for real loaded seed expiration; fresh scratch actions remain available`, async ({ page }) => {
    await openProbe(page);
    await expect(page.getByText('Loading channel history...', { exact: true })).toBeVisible();
    let completed = false;
    const pendingReadiness = waitForSeedReadiness(page, author).then(value => { completed = true; return value; });
    await page.waitForTimeout(150); // A loading/empty history must not satisfy readiness.
    expect(completed).toBe(false);
    // Model records near the end of their real lifetime. Only input history
    // timestamps are supplied; native Date, timers and canModify remain real.
    await loadHistory(page, { author, age: '45000' });
    const own = page.locator('.message-view .message.self:not(.deleted)');
    await expect(own.locator('.message-actions')).toHaveCount(author === 'founder' ? 5 : 1);
    await stabilize(page);
    // This reply header is the actual mobile row that gained a line while
    // the invisible edit/delete controls still reserved their flex width.
    const replyHeader = page.locator('.message', { has: page.locator('.reply-context') }).locator('.message-header');
    const before = await replyHeader.boundingBox();
    const beforeState = await readHistory(page);
    await page.screenshot({ path: test.info().outputPath('before-expiration.png'), animations: 'disabled' });
    const readiness = await pendingReadiness;
    const afterState = await readHistory(page);
    expect(afterState.messages.every(message => message.ageMilliseconds >= 60_000)).toBe(true);
    expect(readiness.actionGroupsBefore).toBe(author === 'founder' ? 5 : 1);
    await expect(own.locator('.message-actions')).toHaveCount(0);
    await stabilize(page);
    const after = await replyHeader.boundingBox();
    if (author === 'founder') expect(before!.height).toBeGreaterThan(after!.height);
    await page.screenshot({ path: test.info().outputPath('after-expiration.png'), animations: 'disabled' });
    await test.info().attach('real-expiration', {
      body: JSON.stringify({ readiness, beforeState, afterState, headerBefore: before, headerAfter: after }, null, 2),
      contentType: 'application/json',
    });

    // The setup wait never suppresses product controls. Fresh scratch records
    // still expose the real edit/cancel and armed-delete/focus behavior; the
    // existing real-server edit/delete tests remain unchanged and mandatory.
    await loadHistory(page, { author, fresh: 'yes' });
    const fresh = page.locator('.message', { hasText: 'A new scratch message remains editable' });
    await fresh.hover();
    await fresh.locator('.edit-btn').click();
    await expect(fresh.locator('.message-edit-input')).toHaveValue('A new scratch message remains editable');
    await fresh.getByRole('button', { name: 'Cancel', exact: true }).click();
    await fresh.locator('.delete-btn').click();
    await expectDeleteConfirmation(page, fresh);
    await fresh.locator('.reply-btn').focus();
    await expect(fresh.locator('.delete-btn.confirming')).toHaveCount(0);
  });
}

test('already expired complete history passes without requiring initial actions', async ({ page }) => {
  await openProbe(page);
  await loadHistory(page, { author: 'founder', age: '120000' });
  const readiness = await waitForSeedReadiness(page, 'founder');
  expect(readiness).toMatchObject({ messages: 7, ownMessages: 6, ownLiveMessages: 5,
    actionGroupsBefore: 0, actionGroupsAfter: 0 });
});

test('incomplete history and the wrong author cannot pass by having no action nodes', async ({ page }) => {
  await openProbe(page);
  await loadHistory(page, { author: 'founder', age: '120000', missing: 'yes' });
  await expect(page.locator('.message-actions')).toHaveCount(0);
  await expect(waitForSeedReadiness(page, 'founder')).rejects.toThrow('Seed history must contain all seven');
  await loadHistory(page, { author: 'member', age: '120000' });
  await expect(page.locator('.message')).toHaveCount(7);
  await expect(page.locator('.message-actions')).toHaveCount(0);
  await expect(waitForSeedReadiness(page, 'founder')).rejects.toThrow('Seed history must belong to founder');
});
