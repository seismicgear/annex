import { expect, test } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import type { FederationPeer, Message, ServerSummary } from '../../src/types';
import type { OutboxPage } from '../../src/api/admin';
import { actor, auth, Companion, delivery, explore, get, type Actor, type RecordFile } from './federation-ui-helpers';

test('actual peer outage, failed join, queued retry and sever controls agree with both live servers', async ({ browser }) => {
  const files = [process.env.ANNEX_FEDERATION_UI_HOST_SESSION, process.env.ANNEX_FEDERATION_UI_PEER_SESSION];
  expect(files.every(Boolean), 'Two explicit fresh captured Windows session records are required').toBe(true);
  const records = await Promise.all(files.map(async file => JSON.parse((await readFile(file!, 'utf8')).replace(/^\uFEFF/, '')) as RecordFile));
  const founders = [await actor(browser, records[0].url), await actor(browser, records[1].url)];
  const companion = new Companion(files as string[]); const receipts: Record<string, unknown> = {};
  let visitor: Actor | undefined;
  const cdp = await founders[0].context.newCDPSession(founders[0].page); await cdp.send('Network.enable');
  try {
    for (const founder of founders) await expect(founder.page.locator('.admin-menu-btn'), 'Each browser must genuinely be the fresh server founder').toBeVisible();
    const started = await companion.command('start', { headers: await Promise.all(founders.map(auth)) });
    const channelId = started.channelId!; expect(channelId).toBeTruthy(); receipts.setup = started;
    const peers = await get<{ peers: FederationPeer[] }>(founders[0], '/api/public/federation/peers');
    const peer = peers.peers.find(item => item.base_url === records[1].url)!; expect(peer).toBeDefined();
    const marker = `federation-ui-before-${Date.now()}`;
    const baseline = await companion.command('publish', { source: 0, text: marker, delivery: true });
    const persisted = await companion.command('received', { peer: 1, messageId: baseline.messageId });
    expect(persisted.matching).toHaveLength(1); expect(persisted.matching![0].content).toBe(marker);

    await test.step('Real peer summary transport failure and failed Join expose their own Retry controls', async () => {
      visitor = await actor(browser, records[0].url); const session = await visitor.context.newCDPSession(visitor.page);
      await session.send('Network.enable');
      const summaryURL = `${records[1].url}/api/public/server/summary`;
      await session.send('Network.setBlockedURLs', { urls: [summaryURL] });
      const dialog = await explore(visitor, peer);
      await expect(dialog.getByRole('alert')).toContainText('Could not reach server');
      await expect(dialog.getByRole('button', { name: 'Join this Server', exact: true })).toHaveCount(0);
      await session.send('Network.setBlockedURLs', { urls: [] });
      await dialog.getByRole('button', { name: 'Retry', exact: true }).click();
      const actual = await visitor.context.request.get(summaryURL); expect(actual.status()).toBe(200);
      const summary = await actual.json() as ServerSummary;
      await expect(dialog.locator('.peer-detail-slug')).toHaveText(summary.slug);
      await session.send('Network.setBlockedURLs', { urls: [summaryURL] });
      await dialog.getByRole('button', { name: 'Join this Server', exact: true }).click();
      await expect(dialog.getByRole('alert')).toBeVisible();
      await expect(dialog.getByRole('button', { name: 'Retry', exact: true })).toBeVisible();
      await session.send('Network.setBlockedURLs', { urls: [] });
      await dialog.getByRole('button', { name: 'Retry', exact: true }).click();
      await expect(visitor.page.locator('.status-bar .server-slug')).toHaveText(summary.slug, { timeout: 150_000 });
      await expect(visitor.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible();
      expect(visitor.credentials.has(new URL(records[1].url).origin), 'Remote registration must produce its own real proof/session').toBe(true);
      await session.detach(); receipts.peerRecovery = { actualRemoteSlug: summary.slug, genuineRegistration: true };
    });

    await test.step('A stopped peer creates a real failed delivery, UI Retry reports transport failure then commits exactly once', async () => {
      await companion.command('offline', { peer: 1 });
      const text = `federation-ui-outage-${Date.now()}`;
      const sent = await companion.command('publish', { source: 0, text, delivery: false });
      let failed: OutboxPage['entries'][number] | undefined;
      await expect.poll(async () => { failed = (await get<OutboxPage>(founders[0], '/api/admin/federation/outbox?status=failed&limit=100')).entries
        // Three actual attempts include the production 60s and 120s backoffs,
        // plus the worker tick and transport time. Observe that full lifecycle.
        .find(item => item.message_id === sent.messageId); return Boolean(failed); }, { timeout: 240_000, intervals: [1000] }).toBe(true);
      expect(failed!.attempts).toBeGreaterThan(0); expect(failed!.last_error).toBeTruthy();
      await delivery(founders[0]); const queue = founders[0].page.locator('.federation-outbox');
      await queue.getByRole('combobox', { name: /^Show/ }).selectOption('failed');
      const row = queue.locator('.outbox-row').filter({ hasText: sent.messageId! });
      await expect(row).toContainText(failed!.last_error!); await expect(row.locator('.outbox-status')).toHaveText('failed');
      const retryPath = `/api/admin/federation/outbox/${failed!.id}/retry`;
      await cdp.send('Network.setBlockedURLs', { urls: [`${records[0].url}${retryPath}`] });
      await row.getByRole('button', { name: 'Retry', exact: true }).click();
      await expect(queue.getByRole('alert')).toContainText('Could not retry that delivery');
      expect((await get<OutboxPage>(founders[0], '/api/admin/federation/outbox?status=failed&limit=100')).entries
        .find(item => item.id === failed!.id)?.status).toBe('failed');
      await cdp.send('Network.setBlockedURLs', { urls: [] }); await companion.command('online', { peer: 1 });
      const retried = founders[0].page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === retryPath);
      await row.getByRole('button', { name: 'Retry', exact: true }).click(); expect((await retried).status()).toBe(200);
      await expect(queue.getByRole('status')).toContainText(`${sent.messageId} is back in the retry rotation`);
      await expect.poll(async () => (await get<OutboxPage>(founders[0], '/api/admin/federation/outbox?status=delivered&limit=100')).entries
        .some(item => item.id === failed!.id), { timeout: 60_000 }).toBe(true);
      await queue.getByRole('combobox', { name: /^Show/ }).selectOption('delivered');
      await queue.getByRole('button', { name: 'Refresh delivery queue', exact: true }).click();
      await expect(queue.locator('.outbox-row').filter({ hasText: sent.messageId! }).locator('.outbox-status')).toHaveText('delivered');
      const actual = await companion.command('received', { peer: 1, messageId: sent.messageId });
      expect(actual.matching).toHaveLength(1); expect(actual.matching![0].content).toBe(text);
      receipts.deliveryRetry = { messageId: sent.messageId, rowId: failed!.id, failedAttempts: failed!.attempts, independentlyPersistedCopies: 1 };
    });

    await test.step('Real agreements survive Cancel and failed sever; successful confirmation stops subsequent delivery in both directions', async () => {
      const panel = founders[0].page.locator('.federation-agreements');
      const row = panel.locator('.agreement-row').filter({ hasText: records[1].url });
      await expect(row.locator('.agreement-label')).toHaveText(peer.label);
      await expect(row.locator('.agreement-alignment')).toHaveText(peer.alignment_status);
      await expect(row.locator('.agreement-scope')).toHaveText(peer.transfer_scope);
      await row.getByRole('button', { name: 'Sever', exact: true }).click();
      const dialog = founders[0].page.getByRole('dialog', { name: `Sever federation with ${peer.label}?`, exact: true });
      await dialog.getByRole('button', { name: 'Cancel', exact: true }).click(); await expect(dialog).toHaveCount(0);
      expect((await get<{ peers: FederationPeer[] }>(founders[0], '/api/public/federation/peers')).peers.some(item => item.agreement_id === peer.agreement_id)).toBe(true);
      await row.getByRole('button', { name: 'Sever', exact: true }).click(); await founders[0].page.keyboard.press('Escape'); await expect(dialog).toHaveCount(0);
      await row.getByRole('button', { name: 'Sever', exact: true }).click();
      const revokePath = `/api/admin/federation/${peer.agreement_id}`;
      await cdp.send('Network.setBlockedURLs', { urls: [`${records[0].url}${revokePath}`] });
      await dialog.getByRole('button', { name: 'Sever agreement', exact: true }).click();
      await expect(dialog.getByRole('alert')).toContainText('Could not sever the agreement');
      expect((await get<{ peers: FederationPeer[] }>(founders[0], '/api/public/federation/peers')).peers.some(item => item.agreement_id === peer.agreement_id)).toBe(true);
      await cdp.send('Network.setBlockedURLs', { urls: [] });
      const revoked = founders[0].page.waitForResponse(response => response.request().method() === 'DELETE' && new URL(response.url()).pathname === revokePath);
      await dialog.getByRole('button', { name: 'Sever agreement', exact: true }).click(); expect((await revoked).status()).toBe(200);
      await expect(dialog).toHaveCount(0); await expect(panel.getByRole('status')).toContainText('severed'); await expect(row).toHaveCount(0);
      expect((await get<{ peers: FederationPeer[] }>(founders[0], '/api/public/federation/peers')).peers.some(item => item.agreement_id === peer.agreement_id)).toBe(false);
      const withheld = [];
      for (const source of [0, 1]) withheld.push({ source, ...await companion.command('publish', { source, text: `after-real-sever-${source}-${Date.now()}`, delivery: false }) });
      await new Promise(resolve => setTimeout(resolve, 5000));
      for (const item of withheld) expect((await companion.command('received', { peer: 1 - item.source, messageId: item.messageId })).matching).toEqual([]);
      const retained = await companion.command('received', { peer: 1, messageId: baseline.messageId }); expect(retained.matching).toHaveLength(1);
      receipts.sever = { agreementId: peer.agreement_id, blockedMessageIds: withheld.map(item => item.messageId), priorDeliveryRetained: true };
    });
    for (const founder of founders) expect(founder.errors).toEqual([]);
    await test.info().attach('actual-federation-ui', { contentType: 'application/json', body: Buffer.from(JSON.stringify({ complete: true, receipts })) });
    const original = await get<Message[]>(founders[0], `/api/channels/${channelId}/messages?limit=100`);
    expect(original.some(item => item.message_id === baseline.messageId)).toBe(true);
  } finally {
    await cdp.send('Network.setBlockedURLs', { urls: [] }); await cdp.detach();
    await companion.close(); await visitor?.context.close(); for (const founder of founders) await founder.context.close();
  }
});
