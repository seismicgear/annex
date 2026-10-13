import { expect, test } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { actor, auth, Companion, get, type Actor, type RecordFile } from './federation-ui-helpers';
import type { Message, MessageEdit } from '../../src/types';
import type { AuthorityPeer, ChannelAuthority } from '../../src/api/channel-authority';

async function select(who: Actor, name: string) {
  await who.page.getByRole('button', { name: 'Chat', exact: true }).click();
  const row = who.page.locator('.channel-item').filter({ has: who.page.locator('.channel-name', { hasText: new RegExp(`^${name}$`) }) });
  await expect(row).toBeVisible();
  if (await row.locator('.join-btn').isVisible()) {
    const joined = who.page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith(`/${name}/join`));
    await row.locator('.join-btn').click(); expect((await joined).status()).toBe(200);
  }
  await row.locator('.channel-select').click(); return row;
}
function bubble(who: Actor, text: string) {
  return who.page.locator('.message').filter({ has: who.page.locator('.message-content', { hasText: text }) });
}
async function send(who: Actor, channel: string, text: string) {
  await who.page.getByPlaceholder('Type a message...').fill(text);
  await who.page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(bubble(who, text).locator('.reply-btn')).toBeVisible();
  let message: Message | undefined;
  await expect.poll(async () => { message = (await get<Message[]>(who, `/api/channels/${channel}/messages?limit=100`)).find(item => item.content === text); return Boolean(message); }).toBe(true);
  return message!;
}

test('actual partner catalog, fixed authority confirmation and retained history work through ordinary controls', async ({ browser }) => {
  const files = [process.env.ANNEX_FEDERATION_UI_HOST_SESSION, process.env.ANNEX_FEDERATION_UI_PEER_SESSION];
  expect(files.every(Boolean), 'Provide a separate fresh pair, not the already-severed federation UI pair').toBe(true);
  const records = await Promise.all(files.map(async file => JSON.parse((await readFile(file!, 'utf8')).replace(/^\uFEFF/, '')) as RecordFile));
  const owner = await actor(browser, records[0].url); const peer = await actor(browser, records[1].url);
  const companion = new Companion(files as string[]);
  const cdp = await owner.context.newCDPSession(owner.page); const peerCdp = await peer.context.newCDPSession(peer.page);
  await cdp.send('Network.enable'); await peerCdp.send('Network.enable');
  const nonce = `authority-ui-${Date.now()}`; const channel = `${nonce}-50`;
  const receipts: Record<string, unknown> = {};
  try {
    for (const who of [owner, peer]) await expect(who.page.locator('.admin-menu-btn')).toBeVisible();
    const setup = await companion.command('start', { headers: [await auth(owner), await auth(peer)] });
    const authorityIdentity = (setup.authorityIdentities as string[])[0]; expect(authorityIdentity).toMatch(/^annex-server:[a-f0-9]{64}$/);
    // Fifty-one genuinely created source channels establish the ordinary
    // paginated catalog. Only the last needs initialized retained history.
    for (let index = 0; index < 51; index++) {
      const id = `${nonce}-${String(index).padStart(2, '0')}`;
      const created = await owner.context.request.post(`${owner.url}/api/channels`, { headers: await auth(owner),
        data: { channel_id: id, name: id, channel_type: 'Text', federation_scope: 'Federated' } });
      expect(created.status()).toBe(200);
    }
    await owner.page.reload(); await expect(owner.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible({ timeout: 150_000 });
    const sourceRow = await select(owner, channel);
    const original = `${nonce}-retained-original`; const revised = `${nonce}-retained-revised`;
    const message = await send(owner, channel, original);
    await bubble(owner, original).locator('.edit-btn').click();
    const editor = owner.page.locator('.message-edit-form');
    await expect(editor).toHaveCount(1);
    await expect(editor.locator('.message-edit-input')).toHaveValue(original);
    await editor.locator('.message-edit-input').fill(revised);
    await editor.locator('.msg-edit-save').click(); await expect(bubble(owner, revised)).toBeVisible();
    const priorHistory = await get<MessageEdit[]>(owner, `/api/channels/${channel}/messages/${message.message_id}/edits`);
    expect(priorHistory.map(item => item.old_content)).toEqual([original]);

    await test.step('Initialization Cancel and actual failure leave authority uninitialized; confirmed retry preserves existing message and audit', async () => {
      await sourceRow.getByTitle('Channel authority', { exact: true }).click();
      const dialog = owner.page.getByRole('dialog', { name: 'Channel authority', exact: true });
      await expect(dialog).toContainText(authorityIdentity);
      const before = await get<ChannelAuthority>(owner, `/api/channels/${channel}/authority`); expect(before.initialized).toBe(false);
      await dialog.getByRole('button', { name: 'Initialize channel authority', exact: true }).click();
      const confirmation = dialog.getByRole('region', { name: 'Confirm channel initialization', exact: true });
      await confirmation.getByRole('button', { name: 'Cancel initialization', exact: true }).click();
      await expect(confirmation).toHaveCount(0);
      expect((await get<ChannelAuthority>(owner, `/api/channels/${channel}/authority`)).initialized).toBe(false);
      await dialog.getByRole('button', { name: 'Initialize channel authority', exact: true }).click();
      const path = `/api/channels/${channel}/authority/initialize`;
      await cdp.send('Network.setBlockedURLs', { urls: [`${owner.url}${path}`] });
      await confirmation.getByRole('button', { name: 'Confirm initialization', exact: true }).click();
      await expect(dialog.getByRole('alert')).toBeVisible();
      expect((await get<ChannelAuthority>(owner, `/api/channels/${channel}/authority`)).initialized).toBe(false);
      await cdp.send('Network.setBlockedURLs', { urls: [] });
      const accepted = owner.page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === path);
      await confirmation.getByRole('button', { name: 'Confirm initialization', exact: true }).click(); expect((await accepted).status()).toBe(200);
      await expect(dialog.getByRole('status')).toContainText('This server accepts changes');
      const after = await get<ChannelAuthority>(owner, `/api/channels/${channel}/authority`);
      expect(after.initialized).toBe(true); expect(after.authority_identity).toBe(authorityIdentity); expect(after.head_seq).toBeGreaterThan(1);
      const retained = (await get<Message[]>(owner, `/api/channels/${channel}/messages?limit=100`)).find(item => item.message_id === message.message_id)!;
      expect(retained.content).toBe(revised); expect(retained.sender_pseudonym).toBe(message.sender_pseudonym);
      expect(await get<MessageEdit[]>(owner, `/api/channels/${channel}/messages/${message.message_id}/edits`)).toEqual(priorHistory);
      await dialog.getByRole('button', { name: 'Refresh authority', exact: true }).click();
      await expect(dialog).toContainText(`${after.head_seq} events`);
      await dialog.getByRole('button', { name: 'Done', exact: true }).click();
      receipts.initialization = { authority: authorityIdentity, retainedMessageId: message.message_id, head: after.head_seq, retainedEditCount: priorHistory.length };
    });

    await test.step('Partner list recovery and actual catalog pagination require explicit matching fingerprint before admission', async () => {
      await peer.page.getByRole('button', { name: 'Chat', exact: true }).click();
      await peerCdp.send('Network.setBlockedURLs', { urls: [`${peer.url}/api/federation/authority-peers`] });
      await peer.page.getByRole('button', { name: 'Federated channels', exact: true }).click();
      const dialog = peer.page.getByRole('dialog', { name: 'Federated channels', exact: true });
      await expect(dialog.getByRole('alert')).toBeVisible();
      await expect(dialog.getByText('No active federation partners are available.', { exact: true })).toHaveCount(0);
      await peerCdp.send('Network.setBlockedURLs', { urls: [] });
      await dialog.getByRole('button', { name: 'Refresh partners', exact: true }).click();
      const partners = await get<{ peers: AuthorityPeer[] }>(peer, '/api/federation/authority-peers');
      const partner = partners.peers.find(item => item.base_url === owner.url)!; expect(partner.authority_identity).toBe(authorityIdentity);
      await dialog.getByRole('combobox', { name: 'Partner server', exact: true }).selectOption(String(partner.peer_id));
      await expect(dialog).toContainText(authorityIdentity);
      const catalogPath = `/api/federation/authority-peers/${partner.peer_id}/channels`;
      await peerCdp.send('Network.setBlockedURLs', { urls: [`${peer.url}${catalogPath}*`] });
      await dialog.getByRole('button', { name: 'Load partner channels', exact: true }).click();
      await expect(dialog.getByRole('alert')).toBeVisible();
      await peerCdp.send('Network.setBlockedURLs', { urls: [] });
      await dialog.getByRole('button', { name: 'Load partner channels', exact: true }).click();
      const unavailable = dialog.locator(`[data-federated-channel="${nonce}-00"]`);
      await expect(unavailable).toContainText('The partner must initialize this channel’s authority');
      await expect(unavailable.getByRole('button', { name: 'Join channel', exact: true })).toHaveCount(0);
      await expect(dialog.locator(`[data-federated-channel="${channel}"]`)).toHaveCount(0);
      await dialog.getByRole('button', { name: 'Load more channels', exact: true }).click();
      const target = dialog.locator(`[data-federated-channel="${channel}"]`); await expect(target).toBeVisible();
      await target.getByRole('button', { name: 'Join channel', exact: true }).click();
      const confirmation = dialog.getByRole('region', { name: 'Confirm federated channel', exact: true });
      await expect(confirmation).toContainText(authorityIdentity);
      await expect(confirmation.getByRole('button', { name: 'Confirm joining', exact: true })).toBeDisabled();
      await confirmation.getByRole('button', { name: 'Cancel joining', exact: true }).click();
      await expect(confirmation).toHaveCount(0);
      await target.getByRole('button', { name: 'Join channel', exact: true }).click();
      await confirmation.getByLabel('I verified this partner fingerprint', { exact: true }).check();
      const attachPath = `/api/federation/channels/${channel}/attach`;
      await peerCdp.send('Network.setBlockedURLs', { urls: [`${peer.url}${attachPath}`] });
      await confirmation.getByRole('button', { name: 'Confirm joining', exact: true }).click(); await expect(dialog.getByRole('alert')).toBeVisible();
      await peerCdp.send('Network.setBlockedURLs', { urls: [] });
      const accepted = peer.page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === attachPath);
      await confirmation.getByRole('button', { name: 'Confirm joining', exact: true }).click(); expect((await accepted).status()).toBe(200);
      await expect(dialog).toHaveCount(0); await expect(bubble(peer, revised)).toBeVisible();
      const bound = await get<ChannelAuthority>(peer, `/api/channels/${channel}/authority`);
      expect(bound.authority_identity).toBe(authorityIdentity); expect(bound.local_authority).toBe(false); expect(bound.initialized).toBe(true);
      const retained = (await get<Message[]>(peer, `/api/channels/${channel}/messages?limit=100`)).find(item => item.message_id === message.message_id)!;
      expect(retained.content).toBe(revised); expect(retained.sender_pseudonym).toBe(message.sender_pseudonym);
      expect((await get<MessageEdit[]>(peer, `/api/channels/${channel}/messages/${message.message_id}/edits`)).map(item => item.old_content)).toEqual([original]);
      receipts.admission = { partnerId: partner.peer_id, authority: bound.authority_identity, head: bound.head_seq, retainedMessageId: retained.message_id };
    });
    await test.step('Ordinary local identity authors through the pinned partner and both sides retain the new accepted message', async () => {
      const text = `${nonce}-from-replica`; const reply = await send(peer, channel, text);
      await expect(bubble(owner, text)).toBeVisible();
      const origin = (await get<Message[]>(owner, `/api/channels/${channel}/messages?limit=100`)).filter(item => item.message_id === reply.message_id);
      expect(origin).toHaveLength(1); expect(origin[0].content).toBe(text); expect(origin[0].sender_pseudonym).toBe(reply.sender_pseudonym);
      const sourceAuthority = await get<ChannelAuthority>(owner, `/api/channels/${channel}/authority`);
      const replicaAuthority = await get<ChannelAuthority>(peer, `/api/channels/${channel}/authority`);
      expect(replicaAuthority.head_hash).toBe(sourceAuthority.head_hash); expect(replicaAuthority.head_seq).toBe(sourceAuthority.head_seq);
      receipts.ordinaryMessage = { messageId: reply.message_id, sender: reply.sender_pseudonym, authoritySequence: sourceAuthority.head_seq };
    });
    expect(owner.errors).toEqual([]); expect(peer.errors).toEqual([]);
    await test.info().attach('actual-authority-controls', { contentType: 'application/json', body: Buffer.from(JSON.stringify({ complete: true, receipts })) });
  } finally {
    await cdp.send('Network.setBlockedURLs', { urls: [] }); await peerCdp.send('Network.setBlockedURLs', { urls: [] });
    await cdp.detach(); await peerCdp.detach(); await companion.close(); await owner.context.close(); await peer.context.close();
  }
});
