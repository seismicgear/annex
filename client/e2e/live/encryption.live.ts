import { expect, test, type Route } from '@playwright/test';
import { baseURL, publicIdentities, read, reload, row, select } from './operator-helpers';
import { bubble, createText, encryptionActor, linkDevice, ready, sendEncrypted, signedEvidence, trust, type EncryptionActor } from './encryption-helpers';
import type { ChannelKeyState, DevicePublicKey, OwnChannelWraps } from '../../src/lib/e2e-v2-protocol';
import type { Message } from '../../src/types';

// Ordinary UI mutations, genuine credentials and independent public-proof and
// ciphertext readback. No fulfilled response mocks or application-store writes.
test('signed encryption, explicit trust, membership, device approval and disable work in real sessions', async ({ browser }) => {
  const actors: EncryptionActor[] = [];
  const evidence: Record<string, unknown> = {};
  const name = `encryption-${Date.now()}`;
  const founder = await encryptionActor(browser); actors.push(founder);
  try {
    await expect(founder.page.locator('.admin-menu-btn'), 'Use a fresh dedicated database').toBeVisible();
    const member = await encryptionActor(browser); actors.push(member);
    await expect(member.page.locator('.admin-menu-btn')).toHaveCount(0);
    const founderId = (await publicIdentities(founder))[0].pseudonymId!;
    const memberId = (await publicIdentities(member))[0].pseudonymId!;
    expect(founderId).not.toBe(memberId);
    const channel = await createText(founder, name);
    const path = `/api/channels/${channel.channel_id}`;
    const firstText = `encrypted-first-${Date.now()}`;
    let firstMessage: Message;
    let currentMember = member;
    const reconcile = async (actor: EncryptionActor) => {
      await select(actor, name);
      const check = actor.page.getByRole('button', { name: 'Check again', exact: true });
      if (await check.isVisible()) await check.click();
      await ready(actor);
    };
    const reconnect = async (actor: EncryptionActor) => {
      const button = actor.page.getByRole('button', { name: 'Reconnect here', exact: true });
      await expect(button).toBeVisible(); await button.click(); await expect(button).toHaveCount(0);
      await select(actor, name);
    };
    const manifestAction = async (actor: EncryptionActor, action: () => Promise<unknown>) => {
      const waiting = actor.page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === `${path}/key-manifest`, { timeout: 30_000 });
      void waiting.catch(() => {}); // Consume a concurrent timeout if the UI action itself fails.
      await action(); expect((await waiting).status()).toBe(200);
    };

    await test.step('Founder signs genesis; member compares and trusts fingerprints before receiving keys', async () => {
      await select(founder, name); await select(member, name);
      await expect(member.page.getByRole('button', { name: /Enable encryption/ })).toHaveCount(0);
      await manifestAction(founder, () => founder.page.getByRole('button', { name: /Enable encryption/ }).click());
      await ready(founder);
      const genesis = await signedEvidence(founder, channel.channel_id);
      expect(genesis.page.events[0].operation.kind).toBe('genesis');
      expect(genesis.page.events[0].signer_pseudonym_id).toBe(founderId);
      await select(member, name);
      const start = member.sent.length;
      await member.page.getByPlaceholder('Type a message...').fill('unverified-must-not-leak');
      await expect(member.page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
      // The button must block the action before a request. Keyboard submission
      // must also fail closed, rather than bypassing the verification state.
      await member.page.getByPlaceholder('Type a message...').press('Enter');
      await expect(member.page.locator('.send-error-bar, .composer-error, .error-message').filter({ hasText: /verif|encrypt|key/i }).first()).toBeVisible();
      expect(member.sent).toHaveLength(start);
      await member.page.getByPlaceholder('Type a message...').fill('');
      await trust(member, founder, channel.channel_id, true);
      await reconcile(founder); await reconcile(member);
      const actual = await signedEvidence(member, channel.channel_id);
      expect(actual.directory.member_keys.map(key => key.pseudonym_id).sort()).toEqual([founderId, memberId].sort());
      expect(new Set(actual.directory.member_keys.map(key => key.device_id)).size).toBe(2);
      expect(actual.own.wraps.length).toBeGreaterThan(0);
      evidence.initialTrust = { ...actual.publicReceipt, canceledTrustRemainedBlocked: true, unverifiedSendEmittedNoFrame: true };
    });

    await test.step('Both browsers send ciphertext and decrypt exact persisted history after reload', async () => {
      firstMessage = await sendEncrypted(founder, channel.channel_id, firstText);
      await expect.poll(() => member.received.find(frame => frame.messageId === firstMessage.message_id)?.content).toBe(firstMessage.content);
      await expect(bubble(member, firstText)).toBeVisible();
      const reply = `encrypted-response-${Date.now()}`;
      await sendEncrypted(member, channel.channel_id, reply); await expect(bubble(founder, reply)).toBeVisible();
      await reload(member); await reconcile(member);
      await expect(bubble(member, firstText)).toBeVisible(); await expect(bubble(member, reply)).toBeVisible();
      await expect(member.page.getByRole('button', { name: 'Verify encryption', exact: true })).toHaveCount(0);
      evidence.exchange = { ciphertextMessageId: firstMessage.message_id, reloadDecrypts: true, trustPinSurvivesReload: true };
    });

    await test.step('Late member cannot read before joining, verifies trust and receives signed history keys', async () => {
      const newcomer = await encryptionActor(browser); actors.push(newcomer);
      for (const suffix of ['/member-keys', '/key-wraps', '/key-status', '/key-manifest', '/messages']) {
        const denied = await newcomer.context.request.get(new URL(path + suffix, baseURL).href, { headers: newcomer.headers });
        expect(denied.status(), `Nonmember GET ${suffix}`).toBe(403);
      }
      // Delay transmission of real key-status requests while the WebSocket
      // remains connected. No response is fulfilled or fabricated: releasing
      // the gate sends every original request to the native server.
      const keyStatusUrl = new URL(`${path}/key-status`, baseURL).href;
      let releaseStatus!: () => void;
      const statusGate = new Promise<void>(resolve => { releaseStatus = resolve; });
      let heldRequests = 0;
      const pendingStatus = new Set<Promise<void>>();
      const delayStatus = async (route: Route) => {
        heldRequests++;
        const pending = statusGate.then(() => route.continue());
        pendingStatus.add(pending);
        try { await pending; } finally { pendingStatus.delete(pending); }
      };
      await newcomer.page.route(keyStatusUrl, delayStatus);
      try {
        await select(newcomer, name);
        await expect.poll(() => heldRequests).toBeGreaterThan(0);
        await expect(newcomer.page.locator('.ws-indicator.connected')).toBeVisible();
        const start = newcomer.sent.length;
        await newcomer.page.getByPlaceholder('Type a message...').fill('unknown-status-must-not-leak');
        await expect(newcomer.page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
        await newcomer.page.getByPlaceholder('Type a message...').press('Enter');
        await expect(newcomer.page.getByRole('region', { name: 'Channel encryption' })).toContainText('Checking channel encryption before sending');
        expect(newcomer.sent).toHaveLength(start);
        await newcomer.page.getByPlaceholder('Type a message...').fill('');
        evidence.unknownStatus = { actualRequestsDelayed: heldRequests, connectedSocketEmittedNoMessage: true };
      } finally {
        releaseStatus();
        // Removing interception can automatically continue an outstanding
        // route. Let every held original request finish continuation first.
        while (pendingStatus.size) await Promise.all([...pendingStatus]);
        await newcomer.page.unroute(keyStatusUrl, delayStatus);
      }
      await trust(newcomer, founder, channel.channel_id);
      await reconcile(founder); await reconcile(newcomer); await reconcile(member);
      await expect(bubble(newcomer, firstText)).toBeVisible();
      const text = `late-encrypted-${Date.now()}`;
      await sendEncrypted(newcomer, channel.channel_id, text); await expect(bubble(member, text)).toBeVisible();
      evidence.lateAdmission = (await signedEvidence(newcomer, channel.channel_id)).publicReceipt;
    });

    await test.step('Unsigned downgrade fails; departure removes access and rejoin restores history with a fresh epoch', async () => {
      for (const actor of [member, founder]) {
        const denied = await actor.context.request.put(new URL(`${path}/e2e`, baseURL).href, { headers: actor.headers, data: { enabled: false } });
        expect(denied.status()).toBe(403);
      }
      const before = await read<ChannelKeyState>(founder, `${path}/key-status`);
      const leaving = member.page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === `${path}/leave`);
      await row(member, name).locator('.leave-btn').click(); expect((await leaving).status()).toBe(200);
      await expect(row(member, name).locator('.join-btn')).toBeVisible();
      for (const suffix of ['/member-keys', '/key-wraps', '/key-status', '/key-manifest', '/messages']) {
        const denied = await member.context.request.get(new URL(path + suffix, baseURL).href, { headers: member.headers });
        expect(denied.status(), `Former member GET ${suffix}`).toBe(403);
      }
      await reconcile(founder);
      expect((await read<ChannelKeyState>(founder, `${path}/key-status`)).current_epoch).toBeGreaterThan(before.current_epoch);
      const absent = `during-absence-${Date.now()}`;
      await sendEncrypted(founder, channel.channel_id, absent); await expect(bubble(member, absent)).toHaveCount(0);
      await select(member, name); await reconcile(founder); await reconcile(member);
      await expect(bubble(member, absent)).toBeVisible(); await expect(bubble(member, firstText)).toBeVisible();
      await sendEncrypted(member, channel.channel_id, `after-rejoin-${Date.now()}`);
      const actual = await signedEvidence(member, channel.channel_id);
      expect(actual.page.events.some(event => event.operation.kind === 'leave' && event.signer_pseudonym_id === memberId)).toBe(true);
      evidence.membership = { ...actual.publicReceipt, departedReadsDenied: true, rejoinedHistoryDecrypts: true };
    });

    await test.step('Original device explicitly approves a linked clean device and shares old and current keys', async () => {
      const linked = await encryptionActor(browser, false); actors.push(linked);
      const originalId = member.deviceId;
      await linkDevice(member, linked);
      expect((await publicIdentities(linked))[0].pseudonymId).toBe(memberId);
      await select(linked, name); await trust(linked, founder, channel.channel_id);
      await expect(linked.page.getByRole('button', { name: 'Recover encryption access', exact: true })).toBeVisible();
      await expect.poll(() => linked.deviceId).toMatch(/^[0-9a-f]{64}$/); expect(linked.deviceId).not.toBe(originalId);
      const directory = await read<{ devices: DevicePublicKey[] }>(founder, `/api/keys/${memberId}`);
      expect(directory.devices.map(device => device.device_id)).toEqual(expect.arrayContaining([originalId, linked.deviceId]));
      await reconnect(member); await ready(member);
      await member.page.getByRole('button', { name: 'Encryption devices', exact: true }).click();
      const dialog = member.page.getByRole('dialog', { name: 'Encryption devices', exact: true });
      const target = dialog.locator('.encryption-device').filter({ has: member.page.locator('code', { hasText: linked.deviceId! }) });
      await expect(target).toContainText('Awaiting approval');
      await manifestAction(member, () => target.getByRole('button', { name: 'Approve device', exact: true }).click());
      await expect(target).toContainText('Approved');
      await dialog.getByRole('button', { name: 'Refresh devices', exact: true }).click(); await expect(target).toContainText('Approved');
      await dialog.getByRole('button', { name: 'Done', exact: true }).click();
      await reconnect(linked); await reconcile(linked); await expect(bubble(linked, firstText)).toBeVisible();
      const text = `linked-encrypted-${Date.now()}`; await sendEncrypted(linked, channel.channel_id, text);
      await expect(bubble(founder, text)).toBeVisible();
      await reload(linked); await reconcile(linked); await expect(bubble(linked, firstText)).toBeVisible();
      evidence.approvedDevice = { ...(await signedEvidence(linked, channel.channel_id)).publicReceipt, distinctDevice: true, historicalDecryptAfterReload: true };
      currentMember = linked;
    });

    await test.step('Removing another device requires confirmation and excludes it from future epoch keys', async () => {
      const before = await read<ChannelKeyState>(founder, `${path}/key-status`);
      await currentMember.page.getByRole('button', { name: 'Encryption devices', exact: true }).click();
      const dialog = currentMember.page.getByRole('dialog', { name: 'Encryption devices', exact: true });
      const oldDevice = dialog.locator('.encryption-device').filter({ has: currentMember.page.locator('code', { hasText: member.deviceId! }) });
      await oldDevice.getByRole('button', { name: 'Remove from channel', exact: true }).click();
      await oldDevice.getByRole('button', { name: 'Cancel removal', exact: true }).click();
      await expect(oldDevice).toContainText('Approved');
      expect((await read<ChannelKeyState>(founder, `${path}/key-status`)).current_epoch).toBe(before.current_epoch);
      await oldDevice.getByRole('button', { name: 'Remove from channel', exact: true }).click();
      await manifestAction(currentMember, () => oldDevice.getByRole('button', { name: 'Confirm removal', exact: true }).click());
      await expect(oldDevice).toContainText('Removed');
      await expect(oldDevice.getByRole('button', { name: 'Approve device', exact: true })).toHaveCount(0);
      await dialog.getByRole('button', { name: 'Done', exact: true }).click();
      await reconcile(founder); await reconcile(currentMember);
      const rotated = await read<ChannelKeyState>(founder, `${path}/key-status`);
      expect(rotated.current_epoch).toBeGreaterThan(before.current_epoch);
      const historical = await read<OwnChannelWraps>(member, `${path}/key-wraps?device_id=${member.deviceId}`);
      expect(historical.wraps.every(wrap => wrap.epoch < rotated.current_epoch)).toBe(true);
      const text = `after-device-removal-${Date.now()}`;
      await sendEncrypted(founder, channel.channel_id, text); await expect(bubble(currentMember, text)).toBeVisible();
      evidence.deviceRemoval = { ...(await signedEvidence(currentMember, channel.channel_id)).publicReceipt,
        cancelPreservedEpoch: true, removedDeviceHasNoFutureKey: true };
    });

    await test.step('Explicit recovery on a fresh device replaces prior devices and recovers available history', async () => {
      const source = currentMember;
      const recovered = await encryptionActor(browser, false); actors.push(recovered);
      await linkDevice(source, recovered);
      expect((await publicIdentities(recovered))[0].pseudonymId).toBe(memberId);
      await select(recovered, name); await trust(recovered, founder, channel.channel_id);
      const before = await read<ChannelKeyState>(founder, `${path}/key-status`);
      const open = recovered.page.getByRole('button', { name: 'Recover encryption access', exact: true });
      await open.click();
      const dialog = recovered.page.getByRole('dialog', { name: 'Recover encryption access', exact: true });
      await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
      await expect(open).toBeVisible();
      expect((await read<ChannelKeyState>(founder, `${path}/key-status`)).current_epoch).toBe(before.current_epoch);
      await open.click();
      await manifestAction(recovered, () => dialog.getByRole('button', { name: 'Recover on this device', exact: true }).click());
      await expect(dialog).toHaveCount(0);
      await reconcile(founder); await reconcile(recovered);
      await expect(bubble(recovered, firstText)).toBeVisible();
      const rotated = await read<ChannelKeyState>(founder, `${path}/key-status`);
      expect(rotated.current_epoch).toBeGreaterThan(before.current_epoch);
      expect(recovered.deviceId).not.toBe(source.deviceId);
      const priorWraps = await read<OwnChannelWraps>(source, `${path}/key-wraps?device_id=${source.deviceId}`);
      expect(priorWraps.wraps.every(wrap => wrap.epoch < rotated.current_epoch)).toBe(true);
      const text = `after-explicit-recovery-${Date.now()}`;
      await sendEncrypted(recovered, channel.channel_id, text); await expect(bubble(founder, text)).toBeVisible();
      await reload(recovered); await reconcile(recovered); await expect(bubble(recovered, firstText)).toBeVisible();
      const receipt = await signedEvidence(recovered, channel.channel_id);
      expect(receipt.page.events.some(event => event.operation.kind === 'recover_device' && event.signer_pseudonym_id === memberId)).toBe(true);
      evidence.explicitRecovery = { ...receipt.publicReceipt, cancelPreservedEpoch: true, oldDeviceHasNoNewEpoch: true, historyAfterReload: true };
      currentMember = recovered;
    });

    await test.step('Moderator cancels and confirms signed disable; retained ciphertext stays readable and reenable rotates', async () => {
      await reconcile(founder);
      await founder.page.getByRole('button', { name: 'Disable encryption', exact: true }).click();
      const dialog = founder.page.getByRole('dialog', { name: 'Disable channel encryption', exact: true });
      await dialog.getByRole('button', { name: 'Keep encryption', exact: true }).click();
      expect((await read<ChannelKeyState>(founder, `${path}/key-status`)).e2e_enabled).toBe(true);
      await founder.page.getByRole('button', { name: 'Disable encryption', exact: true }).click();
      await manifestAction(founder, () => dialog.getByRole('button', { name: 'Disable encryption', exact: true }).click());
      await expect(dialog).toHaveCount(0);
      const disabled = await read<ChannelKeyState>(founder, `${path}/key-status`); expect(disabled.e2e_enabled).toBe(false);
      await select(currentMember, name);
      const plain = `ordinary-after-disable-${Date.now()}`;
      await founder.page.getByPlaceholder('Type a message...').fill(plain); await founder.page.getByRole('button', { name: 'Send', exact: true }).click();
      await expect(bubble(currentMember, plain)).toBeVisible();
      const history = await read<Message[]>(founder, `${path}/messages?limit=100`);
      expect(history.some(message => message.content === plain)).toBe(true);
      expect(history.find(message => message.message_id === firstMessage.message_id)?.content).toMatch(/^e2e2:/);
      await reload(currentMember); await select(currentMember, name);
      await expect(bubble(currentMember, firstText)).toBeVisible(); await expect(bubble(currentMember, plain)).toBeVisible();
      await manifestAction(founder, () => founder.page.getByRole('button', { name: /Enable encryption/ }).click());
      await reconcile(founder); await reconcile(currentMember);
      expect((await read<ChannelKeyState>(founder, `${path}/key-status`)).current_epoch).toBeGreaterThan(disabled.current_epoch);
      await sendEncrypted(founder, channel.channel_id, 'reenabled-encrypted-message');
      await expect(bubble(currentMember, 'reenabled-encrypted-message')).toBeVisible();
      const actual = await signedEvidence(founder, channel.channel_id);
      expect(actual.page.events.filter(event => event.operation.kind === 'set_encryption').map(event => event.operation)).toEqual([
        { kind: 'set_encryption', enabled: false }, { kind: 'set_encryption', enabled: true },
      ]);
      evidence.disableAndReenable = { ...actual.publicReceipt, canceledDisablePreservedEncryption: true, originalCiphertextRetained: true };
    });
  } finally {
    for (const actor of actors) {
      await test.info().attach('actual-message-progress', { body: JSON.stringify({ sent: actor.sent, received: actor.received,
        renderedMessages: await actor.page.locator('.message-content').allTextContents() }), contentType: 'application/json' });
      await test.info().attach('crypto-request-progress', { body: JSON.stringify({ requests: actor.cryptoRequests, workerDiagnostics: actor.workerDiagnostics, workers: actor.page.workers().map(worker => worker.url()) }), contentType: 'application/json' });
      const region = actor.page.getByRole('region', { name: 'Channel encryption', exact: true });
      if (await region.isVisible().catch(() => false)) {
        await test.info().attach('last-encryption-control-state', { body: await region.innerText(), contentType: 'text/plain' });
      }
    }
    await test.info().attach('encryption-acceptance', { body: JSON.stringify(evidence, null, 2), contentType: 'application/json' });
    for (const actor of actors) {
      expect.soft(actor.errors, 'Uncaught browser errors').toEqual([]);
      await actor.context.close();
    }
  }
});
