import { expect, test } from '@playwright/test';
import type { Message } from '../../src/types';
import type { ChannelKeyState } from '../../src/lib/e2e-v2-protocol';
import { baseURL, read, reload, select } from './operator-helpers';
import { bubble, createText, encryptionActor, ready, sendEncrypted, signedEvidence, trust } from './encryption-helpers';

// A separate fresh-server lane. The real browser drops selected transport
// requests; no server reply, cryptographic material or app state is invented.
test('failed signed enable and failed key refresh remain blocked until real UI retry succeeds', async ({ browser }, info) => {
  const founder = await encryptionActor(browser);
  const member = await encryptionActor(browser);
  const cdp = await founder.context.newCDPSession(founder.page);
  const memberCdp = await member.context.newCDPSession(member.page);
  const receipts: Record<string, unknown> = {};
  try {
    await expect(founder.page.locator('.admin-menu-btn'), 'Fresh dedicated database required').toBeVisible();
    const channel = await createText(founder, `encryption-errors-${Date.now()}`);
    const path = `/api/channels/${channel.channel_id}`;
    await reload(member); await select(founder, channel.name); await select(member, channel.name);
    await cdp.send('Network.enable'); await memberCdp.send('Network.enable');
    await test.step('Dropped actual signed-enable POST displays failure and does not enable the server', async () => {
      await cdp.send('Fetch.enable', { patterns: [{ urlPattern: `${baseURL}${path}/key-manifest`, requestStage: 'Request' }] });
      cdp.on('Fetch.requestPaused', async event => {
        if (event.request.method === 'POST') await cdp.send('Fetch.failRequest', { requestId: event.requestId, errorReason: 'Failed' });
        else await cdp.send('Fetch.continueRequest', { requestId: event.requestId });
      });
      const failed = founder.page.waitForEvent('requestfailed', { predicate: request => request.method() === 'POST'
        && new URL(request.url()).pathname === `${path}/key-manifest` });
      await founder.page.getByRole('button', { name: /Enable encryption/ }).click(); await failed;
      await expect(founder.page.locator('.channel-encryption-error')).toBeVisible();
      const actual = await read<ChannelKeyState>(member, `${path}/key-status`);
      expect(actual.e2e_enabled).toBe(false);
      const manifest = await read<{ head_seq: number }>(member, `${path}/key-manifest`);
      expect(manifest.head_seq).toBe(0);
      receipts.refusedEnable = { channelId: channel.channel_id, persistedEnabled: actual.e2e_enabled, manifestSequence: manifest.head_seq };
      await cdp.send('Fetch.disable');
      const accepted = founder.page.waitForResponse(response => response.request().method() === 'POST'
        && new URL(response.url()).pathname === `${path}/key-manifest`);
      await founder.page.getByRole('button', { name: /Enable encryption/ }).click();
      expect((await accepted).status()).toBe(200); await ready(founder);
      await expect(founder.page.locator('.channel-encryption-error')).toBeHidden();
      await trust(member, founder, channel.channel_id);
      await select(founder, channel.name); await ready(founder); await select(member, channel.name); await ready(member);
      receipts.recoveredEnable = (await signedEvidence(member, channel.channel_id)).publicReceipt;
    });
    const original = `before-real-key-outage-${Date.now()}`;
    const persisted = await sendEncrypted(founder, channel.channel_id, original);
    await expect(bubble(member, original)).toBeVisible();
    await test.step('Actual key-status outage after reload cannot send plaintext and Retry restores signed encrypted exchange', async () => {
      await memberCdp.send('Network.setBlockedURLs', { urls: [`${baseURL}${path}/key-status`] });
      await reload(member); await select(member, channel.name);
      const region = member.page.getByRole('region', { name: 'Channel encryption', exact: true });
      await expect(region.getByRole('alert')).toContainText(/could not be loaded|could not be verified/);
      await expect(region).toContainText('Sending remains blocked.');
      const sentBefore = member.sent.length;
      const withheld = `cannot-leak-through-key-error-${Date.now()}`;
      await member.page.getByPlaceholder('Type a message...').fill(withheld);
      await expect(member.page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
      await member.page.getByPlaceholder('Type a message...').press('Enter');
      await expect(member.page.locator('.send-error-bar, .composer-error, .error-message').filter({ hasText: /verif|encrypt|key/i }).first()).toBeVisible();
      expect(member.sent.length).toBe(sentBefore);
      expect((await read<Message[]>(founder, `${path}/messages`)).some(item => item.content.includes(withheld))).toBe(false);
      await member.page.getByPlaceholder('Type a message...').fill('');
      await memberCdp.send('Network.setBlockedURLs', { urls: [] });
      const status = member.page.waitForResponse(response => response.request().method() === 'GET'
        && new URL(response.url()).pathname === `${path}/key-status`);
      await region.getByRole('button', { name: 'Retry', exact: true }).click();
      expect((await status).status()).toBe(200); await ready(member);
      await expect(region.getByRole('alert')).toHaveCount(0);
      await expect(bubble(member, original)).toBeVisible();
      const recovered = `after-real-key-retry-${Date.now()}`;
      const reply = await sendEncrypted(member, channel.channel_id, recovered);
      await expect(bubble(founder, recovered)).toBeVisible();
      receipts.keyRetry = { originalMessageId: persisted.message_id, recoveredMessageId: reply.message_id,
        failedStateSentFrames: member.sent.slice(sentBefore).filter(frame => frame.content === withheld).length,
        publicProofs: (await signedEvidence(member, channel.channel_id)).publicReceipt };
    });
    expect(founder.errors).toEqual([]); expect(member.errors).toEqual([]);
    await info.attach('real-encryption-errors', { contentType: 'application/json', body: Buffer.from(JSON.stringify({ complete: true, receipts })) });
  } finally {
    await cdp.send('Fetch.disable');
    await cdp.send('Network.setBlockedURLs', { urls: [] }); await memberCdp.send('Network.setBlockedURLs', { urls: [] });
    await cdp.detach(); await memberCdp.detach(); await member.context.close(); await founder.context.close();
  }
});
