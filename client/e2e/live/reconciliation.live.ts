import { expect, test, type Locator } from '@playwright/test';
import { readFile, realpath } from 'node:fs/promises';
import { dirname, isAbsolute, join, relative } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { actor, auth, Companion, get, type Actor, type RecordFile } from './federation-ui-helpers';
import type { Message } from '../../src/types';
import type { OutboxPage } from '../../src/api/admin';
import type { AuthorityPeer, ChannelAuthority } from '../../src/api/channel-authority';
import type { ReconciliationPreview, ReconciliationRequest, ReconciliationResult } from '../../src/api/authority-reconciliation';

type Saved = { request: ReconciliationRequest; result: ReconciliationResult };
type SessionRecord = RecordFile & { data: string };
const authorityPath = (channel: string) => `/api/channels/${channel}/authority`;
async function select(who: Actor, channel: string) {
  await who.page.getByRole('button', { name: 'Chat', exact: true }).click();
  const row = who.page.locator('.channel-item').filter({ has: who.page.locator('.channel-name', { hasText: new RegExp(`^${channel}$`) }) });
  await expect(row).toBeVisible(); await expect(row.locator('.join-btn')).toHaveCount(0);
  await row.locator('.channel-select').click(); return row;
}
async function dialog(who: Actor, channel: string) {
  await (await select(who, channel)).getByTitle('Channel authority', { exact: true }).click();
  return who.page.getByRole('dialog', { name: 'Channel authority', exact: true });
}
async function initialize(who: Actor, channel: string, fingerprint: string) {
  const view = await dialog(who, channel); await expect(view).toContainText(fingerprint);
  await view.getByRole('button', { name: 'Initialize channel authority', exact: true }).click();
  await view.getByRole('region', { name: 'Confirm channel initialization', exact: true })
    .getByRole('button', { name: 'Confirm initialization', exact: true }).click();
  await expect.poll(async () => (await get<ChannelAuthority>(who, authorityPath(channel))).initialized).toBe(true);
  await view.getByRole('button', { name: 'Done', exact: true }).click();
}
async function history(who: Actor, channel: string) {
  return get<Message[]>(who, `/api/channels/${channel}/messages?limit=100`);
}
function bubble(who: Actor, content: string) {
  return who.page.locator('.message').filter({ has: who.page.locator('.message-content', { hasText: content }) });
}
async function send(who: Actor, channel: string, content: string) {
  await select(who, channel); await who.page.getByPlaceholder('Type a message...').fill(content);
  await who.page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(bubble(who, content).locator('.reply-btn')).toBeVisible();
  let found: Message | undefined;
  await expect.poll(async () => { found = (await history(who, channel)).find(message => message.content === content); return Boolean(found); }).toBe(true);
  return found!;
}
async function review(who: Actor, source: Actor, channel: string, fingerprint: string) {
  const view = await dialog(who, channel);
  await view.getByRole('button', { name: 'Review partner reconciliation', exact: true }).click();
  const partners = await get<{ peers: AuthorityPeer[] }>(who, '/api/federation/authority-peers');
  const partner = partners.peers.find(value => value.base_url === source.url)!;
  expect(partner.authority_identity).toBe(fingerprint);
  await view.getByRole('combobox', { name: 'Reconciliation partner', exact: true }).selectOption(String(partner.peer_id));
  await view.getByRole('button', { name: 'Load matching partner channel', exact: true }).click();
  await view.getByRole('button', { name: 'Review retained local history', exact: true }).click();
  const confirmation = view.getByRole('region', { name: 'Confirm history reconciliation', exact: true });
  await expect(confirmation).toContainText(fingerprint);
  const snapshot = await get<ReconciliationPreview>(who, `${authorityPath(channel)}/reconciliation-preview`);
  await expect(confirmation).toContainText(snapshot.snapshot_sha256);
  await expect(confirmation.getByRole('button', { name: 'Confirm reconciliation', exact: true })).toBeDisabled();
  return { view, confirmation, snapshot };
}
async function verified(confirmation: Locator) {
  await confirmation.getByLabel('I verified this partner fingerprint and local snapshot', { exact: true }).check();
}
async function saved(who: Actor, channel: string) {
  return (await get<{ operations: Saved[] }>(who, `${authorityPath(channel)}/reconciliations`)).operations;
}
async function inspect(recordFile: string, record: SessionRecord) {
  const root = await realpath(join(dirname(recordFile), 'sessions')); const data = await realpath(record.data);
  const within = relative(root, data); expect(within && !within.startsWith('..') && !isAbsolute(within)).toBeTruthy();
  return new DatabaseSync(join(data, 'annex.db'), { readOnly: true });
}

test('populated history reconciliation preserves conflicts, resumes a lost acknowledgement, cancels staging, and adopts matching history', async ({ browser }) => {
  const files = [process.env.ANNEX_FEDERATION_UI_HOST_SESSION, process.env.ANNEX_FEDERATION_UI_PEER_SESSION];
  expect(files.every(Boolean), 'Use a fresh captured two-server pair for this lane').toBe(true);
  const records = await Promise.all(files.map(async file => JSON.parse((await readFile(file!, 'utf8')).replace(/^\uFEFF/, '')) as SessionRecord));
  const owner = await actor(browser, records[0].url), peer = await actor(browser, records[1].url);
  const companion = new Companion(files as string[]); const receipts: Record<string, unknown> = {};
  const cdp = await peer.context.newCDPSession(peer.page); let fetchEnabled = false;
  const dbs = await Promise.all(records.map((record, index) => inspect(files[index]!, record)));
  try {
    for (const who of [owner, peer]) await expect(who.page.locator('.admin-menu-btn')).toBeVisible();
    const setup = await companion.command('start', { headers: [await auth(owner), await auth(peer)] });
    const conflictChannel = setup.channelId!, successChannel = `reconcile-success-${Date.now()}`;
    const fingerprint = (setup.authorityIdentities as string[])[0];
    expect(fingerprint).toMatch(/^annex-server:[0-9a-f]{64}$/);
    // The companion has genuinely enrolled bridge identities. Refresh the
    // browsers through the ordinary session path before reusing their proof
    // headers in any independently observed administrative request.
    for (const who of [owner, peer]) {
      who.credentials.clear(); await who.page.reload();
      await expect(who.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible({ timeout: 150_000 });
      await auth(who);
    }
    await companion.command('create-legacy-channel', { channelId: successChannel, headers: [await auth(owner), await auth(peer)] });
    const shared = new Map<string, string>();
    for (const channel of [conflictChannel, successChannel]) {
      const receipt = await companion.command('publish', { source: 0, channelId: channel, text: `${channel}-retained`, delivery: true });
      shared.set(channel, receipt.messageId!);
      const [a, b] = await Promise.all([history(owner, channel), history(peer, channel)]);
      const original = a.find(value => value.message_id === receipt.messageId)!;
      const copy = b.find(value => value.message_id === receipt.messageId)!;
      for (const field of ['message_id', 'sender_pseudonym', 'content', 'created_at', 'reply_to_message_id'] as const) expect(copy[field]).toEqual(original[field]);
    }
    for (const who of [owner, peer]) { await who.page.reload(); await expect(who.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible({ timeout: 150_000 }); }
    await initialize(owner, conflictChannel, fingerprint);
    const orphan = await send(peer, conflictChannel, `${conflictChannel}-unaccepted-local-history`);
    await expect.poll(async () => (await get<OutboxPage>(peer, '/api/admin/federation/outbox?status=failed&limit=100')).entries
      .some(value => value.message_id === orphan.message_id), { timeout: 180_000 }).toBe(true);
    expect((await history(owner, conflictChannel)).some(value => value.message_id === orphan.message_id)).toBe(false);
    const retainedBefore = await history(peer, conflictChannel);
    let lost: { request: ReconciliationRequest; result: ReconciliationResult } | undefined;

    await test.step('Cancel review changes nothing; a genuine accepted conflict response is lost in transport after commit', async () => {
      const { view, confirmation, snapshot } = await review(peer, owner, conflictChannel, fingerprint);
      expect(snapshot.messages).toBe(2);
      await confirmation.getByRole('button', { name: 'Cancel reconciliation review', exact: true }).click();
      await expect(confirmation).toHaveCount(0); expect(await saved(peer, conflictChannel)).toEqual([]);
      await view.getByRole('button', { name: 'Review retained local history', exact: true }).click();
      await expect(confirmation).toContainText(snapshot.snapshot_sha256); await verified(confirmation);
      const captureErrors: string[] = [];
      cdp.on('Fetch.requestPaused', event => {
        void (async () => {
          if (event.request.method !== 'POST') { await cdp.send('Fetch.continueRequest', { requestId: event.requestId }); return; }
          try {
            expect(event.responseStatusCode).toBe(200);
            const body = await cdp.send('Fetch.getResponseBody', { requestId: event.requestId });
            const result = JSON.parse(Buffer.from(body.body, body.base64Encoded ? 'base64' : 'utf8').toString()) as ReconciliationResult;
            const request = JSON.parse(event.request.postData!) as ReconciliationRequest;
            expect(result.operation_id).toBe(request.operation_id); expect(result.status).toBe('conflict'); expect(result.remote_join_accepted).toBe(true);
            if (lost) expect(request).toEqual(lost.request); else lost = { request, result };
          } catch (error) { captureErrors.push(error instanceof Error ? error.message : String(error)); }
          finally { await cdp.send('Fetch.failRequest', { requestId: event.requestId, errorReason: 'ConnectionClosed' }); }
        })().catch(error => captureErrors.push(String(error)));
      });
      await cdp.send('Fetch.enable', { patterns: [{ urlPattern: `${peer.url}${authorityPath(conflictChannel)}/reconcile`, requestStage: 'Response' }] }); fetchEnabled = true;
      await confirmation.getByRole('button', { name: 'Confirm reconciliation', exact: true }).click();
      await expect(view.getByRole('alert')).toBeVisible({ timeout: 150_000 });
      expect(captureErrors).toEqual([]); expect(lost).toBeDefined();
      await cdp.send('Fetch.disable'); fetchEnabled = false;
      expect(lost!.request.expected_snapshot_sha256).toBe(snapshot.snapshot_sha256);
      expect(lost!.result.conflicts.some(value => value.id === orphan.message_id)).toBe(true);
      const committed = await saved(peer, conflictChannel); expect(committed).toHaveLength(1); expect(committed[0]).toEqual(lost);
      expect(await history(peer, conflictChannel)).toEqual(retainedBefore);
      expect((await get<ChannelAuthority>(peer, authorityPath(conflictChannel))).initialized).toBe(false);
      receipts.lostAcknowledgement = { operation: lost!.request.operation_id, snapshot: snapshot.snapshot_sha256, actualServerStatus: lost!.result.status };
    });

    await test.step('Reload reviews and resumes the identical saved operation, then explicit cancellation preserves history and the accepted remote Join', async () => {
      await peer.page.reload(); await expect(peer.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible({ timeout: 150_000 });
      const view = await dialog(peer, conflictChannel);
      await view.getByRole('button', { name: 'Review partner reconciliation', exact: true }).click();
      await view.getByRole('button', { name: 'Refresh saved reconciliations', exact: true }).click();
      await view.locator(`[data-reconciliation-operation="${lost!.request.operation_id}"]`).getByRole('button', { name: 'Review saved operation', exact: true }).click();
      const confirmation = view.getByRole('region', { name: 'Confirm history reconciliation', exact: true });
      await expect(confirmation).toContainText(fingerprint); await expect(confirmation).toContainText(lost!.request.expected_snapshot_sha256);
      await expect(confirmation.getByRole('region', { name: 'Reconciliation conflicts', exact: true })).toContainText(orphan.message_id);
      await verified(confirmation);
      const response = peer.page.waitForResponse(value => value.request().method() === 'POST' && new URL(value.url()).pathname === `${authorityPath(conflictChannel)}/reconcile`);
      await confirmation.getByRole('button', { name: 'Resume reconciliation', exact: true }).click();
      const accepted = await response; expect(accepted.status()).toBe(200); expect(accepted.request().postDataJSON()).toEqual(lost!.request);
      const resumed = await accepted.json() as ReconciliationResult; expect(resumed.status).toBe('conflict'); expect(resumed.operation_id).toBe(lost!.request.operation_id);
      await confirmation.getByRole('button', { name: 'Cancel remaining reconciliation', exact: true }).click();
      const cancel = view.getByRole('region', { name: 'Confirm reconciliation cancellation', exact: true });
      await cancel.getByRole('button', { name: 'Keep reconciliation', exact: true }).click(); await expect(cancel).toHaveCount(0);
      expect((await saved(peer, conflictChannel))[0].result.status).toBe('conflict');
      await confirmation.getByRole('button', { name: 'Cancel remaining reconciliation', exact: true }).click();
      await cancel.getByRole('button', { name: 'Confirm cancellation', exact: true }).click();
      await expect(confirmation.getByRole('status')).toContainText('Remaining local reconciliation staging was cancelled');
      const result = (await saved(peer, conflictChannel))[0]; expect(result.request).toEqual(lost!.request); expect(result.result.status).toBe('cancelled'); expect(result.result.remote_join_accepted).toBe(true);
      expect(await history(peer, conflictChannel)).toEqual(retainedBefore);
      expect(dbs[1].prepare('SELECT COUNT(*) AS n FROM channel_reconciliation_records WHERE channel_id=? AND operation_id=?').get(conflictChannel, lost!.request.operation_id)!.n).toBe(0);
      expect(dbs[0].prepare('SELECT COUNT(*) AS n FROM channel_members WHERE channel_id=? AND pseudonym_id=?').get(conflictChannel, orphan.sender_pseudonym)!.n).toBe(1);
      expect((await get<ChannelAuthority>(peer, authorityPath(conflictChannel))).initialized).toBe(false);
      await view.getByRole('button', { name: 'Done', exact: true }).click(); receipts.cancellation = { operation: result.result.operation_id, retainedMessages: retainedBefore.length, remoteJoinStillPresent: true };
    });

    await test.step('A separate matching populated channel adopts the pinned authority and a real new browser-authored message reaches both histories', async () => {
      await initialize(owner, successChannel, fingerprint);
      const original = (await history(peer, successChannel)).find(value => value.message_id === shared.get(successChannel))!;
      const { confirmation, snapshot } = await review(peer, owner, successChannel, fingerprint); expect(snapshot.messages).toBe(1);
      await verified(confirmation); await confirmation.getByRole('button', { name: 'Confirm reconciliation', exact: true }).click();
      await expect.poll(async () => (await get<ChannelAuthority>(peer, authorityPath(successChannel))).local_authority, { timeout: 150_000 }).toBe(false);
      const bound = await get<ChannelAuthority>(peer, authorityPath(successChannel)); expect(bound.initialized).toBe(true); expect(bound.authority_identity).toBe(fingerprint);
      const operation = (await saved(peer, successChannel))[0].result; expect(operation.status).toBe('complete'); expect(operation.retained_messages).toBe(1); expect(operation.removed_messages).toBe(0);
      const retained = (await history(peer, successChannel)).find(value => value.message_id === original.message_id)!;
      for (const field of ['message_id', 'sender_pseudonym', 'content', 'created_at', 'reply_to_message_id'] as const) expect(retained[field]).toEqual(original[field]);
      await peer.page.getByRole('dialog', { name: 'Channel authority', exact: true }).getByRole('button', { name: 'Done', exact: true }).click();
      const message = await send(peer, successChannel, `${successChannel}-after-adoption`);
      await expect.poll(async () => (await history(owner, successChannel)).filter(value => value.message_id === message.message_id).length).toBe(1);
      const source = (await history(owner, successChannel)).find(value => value.message_id === message.message_id)!; expect(source.content).toBe(message.content); expect(source.sender_pseudonym).toBe(message.sender_pseudonym);
      receipts.adoption = { operation: operation.operation_id, retainedMessage: original.message_id, newMessage: message.message_id, authority: bound.authority_identity };
    });
    expect(owner.errors).toEqual([]); expect(peer.errors).toEqual([]);
    await test.info().attach('actual-populated-reconciliation', { contentType: 'application/json', body: Buffer.from(JSON.stringify({ complete: true, receipts })) });
  } finally {
    if (fetchEnabled) await cdp.send('Fetch.disable'); await cdp.detach();
    for (const db of dbs) db.close(); await companion.close(); await owner.context.close(); await peer.context.close();
  }
});
