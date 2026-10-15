import { expect, test } from '@playwright/test';
import { completeStartup } from '../helpers';
import type { VoiceConfigStatus } from '../../src/api/voice';

test('actual absent WebRTC infrastructure blocks call creation and explains the missing setup while policy remains enabled', async ({ browser }) => {
  const url = process.env.ANNEX_UNCONFIGURED_VOICE_URL;
  expect(url, 'Use a separate fresh server with actual empty webrtc.url, api_key and api_secret; no mocked readiness response').toBeTruthy();
  const base = new URL(url!).origin;
  const context = await browser.newContext({ baseURL: base });
  const page = await context.newPage();
  let headers: Record<string, string> = {};
  const browserJoinRequests: string[] = [];
  page.on('request', request => {
    if (new URL(request.url()).origin !== base) return;
    const requestHeaders = request.headers();
    if (requestHeaders.authorization && requestHeaders['x-annex-zk-proof']) headers = Object.fromEntries(
      Object.entries(requestHeaders).filter(([key]) => key === 'authorization' || key.startsWith('x-annex-')));
    if (/\/voice\/join$/.test(new URL(request.url()).pathname)) browserJoinRequests.push(request.method());
  });
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  try {
    const probe = await context.request.get(`${base}/api/voice/config-status`);
    expect(probe.status()).toBe(200);
    const status = await probe.json() as VoiceConfigStatus;
    expect(status, 'A policy-off server or missing captions engine is a different test condition').toMatchObject({
      policy_enabled: true, infrastructure_ready: false, voice_enabled: false, has_public_url: false,
    });
    expect(status.setup_hint).toContain('WebRTC is not configured');
    await completeStartup(page);
    await expect(page.locator('.admin-menu-btn'), 'Dedicated fresh database required').toBeVisible();
    await expect.poll(() => Boolean(headers.authorization)).toBe(true);
    const name = `missing-voice-${Date.now()}`;
    await page.locator('.create-channel-btn').click();
    const dialog = page.getByRole('dialog');
    await dialog.getByPlaceholder('general').fill(name);
    await dialog.locator('select').selectOption('Hybrid');
    const created = page.waitForResponse(response => response.request().method() === 'POST'
      && new URL(response.url()).pathname === '/api/channels');
    await dialog.getByRole('button', { name: 'Create', exact: true }).click();
    expect((await created).status()).toBe(200);
    const row = page.locator('.channel-item').filter({ has: page.locator('.channel-name', { hasText: new RegExp(`^${name}$`) }) });
    if (await row.locator('.join-btn').isVisible()) {
      const joined = page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/join'));
      await row.locator('.join-btn').click(); expect((await joined).status()).toBe(200);
    }
    await row.locator('.channel-select').click();
    await expect(page.locator('.voice-join-btn')).toBeDisabled();
    await expect(page.locator('.voice-join-btn')).toHaveAttribute('title', 'Voice is not set up on this server yet.');
    await expect(page.locator('.voice-error')).toContainText('Voice is not set up on this server yet.');
    await expect(page.locator('.voice-setup-hint')).toHaveText(status.setup_hint);
    expect(browserJoinRequests, 'Disabled creation must not submit a doomed voice join').toEqual([]);

    const listed = await context.request.get(`${base}/api/channels`, { headers }); expect(listed.status()).toBe(200);
    const channel = (await listed.json() as Array<{ name: string; channel_id: string }>).find(item => item.name === name)!;
    expect(channel).toBeDefined();
    const refused = await context.request.post(`${base}/api/channels/${channel.channel_id}/voice/join`, { headers });
    expect(refused.status()).toBe(503);
    expect(await refused.json()).toMatchObject({ error: 'voice_not_configured' });
    const participants = await context.request.get(`${base}/api/channels/${channel.channel_id}/voice/status`, { headers });
    expect(participants.status()).toBe(200); expect(await participants.json()).toMatchObject({ participants: 0, active: false });
    await page.reload(); await expect(page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible();
    await row.locator('.channel-select').click();
    await expect(page.locator('.voice-join-btn')).toBeDisabled();
    await expect(page.locator('.voice-setup-hint')).toHaveText(status.setup_hint);
    expect(errors).toEqual([]);
    await test.info().attach('actual-missing-voice-infrastructure', { contentType: 'application/json',
      body: Buffer.from(JSON.stringify({ publicReadiness: status, disabledControl: true, directJoinStatus: 503,
        noParticipantCreated: true, warningPersistedAfterReload: true })) });
  } finally { await context.close(); }
});
