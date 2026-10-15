import { expect, test } from '@playwright/test';
import { newActor, read, select, send } from './operator-helpers';
import type { Channel, Message, MessageEdit } from '../../src/types';

test('a real empty edit history belongs to an unedited message and the first accepted edit commits an audit visible to another member', async ({ browser }) => {
  const owner = await newActor(browser); const peer = await newActor(browser);
  try {
    await select(owner, 'General'); await select(peer, 'General');
    const channel = (await read<Channel[]>(owner, '/api/channels')).find(item => item.name === 'General')!;
    expect(channel).toBeDefined();
    const original = `audit-invariant-${Date.now()}`; const updated = `${original}-edited`;
    const message = await send(owner, 'General', original);
    const historyPath = `/api/channels/${channel.channel_id}/messages/${message.message_id}/edits`;
    const bubble = (actor: typeof owner, text: string) => actor.page.locator('.message').filter({
      has: actor.page.locator('.message-content', { hasText: text }),
    });
    for (const actor of [owner, peer]) {
      expect(await read<MessageEdit[]>(actor, historyPath)).toEqual([]);
      await expect(bubble(actor, original)).toHaveCount(1);
      await expect(bubble(actor, original).locator('.edited-badge')).toHaveCount(0);
      await expect(bubble(actor, original).locator('.edit-history-empty')).toHaveCount(0);
    }
    const source = bubble(owner, original);
    await source.locator('.edit-btn').click();
    // Editing replaces the content element used to locate the original bubble.
    const editor = owner.page.locator('.message-edit-form');
    await expect(editor).toHaveCount(1);
    await expect(editor.locator('.message-edit-input')).toHaveValue(original);
    await editor.locator('.message-edit-input').fill(updated);
    await editor.locator('.msg-edit-save').click();
    for (const actor of [owner, peer]) {
      const edited = bubble(actor, updated);
      await expect(edited.locator('.edited-badge')).toBeVisible();
      const audit = await read<MessageEdit[]>(actor, historyPath);
      expect(audit).toHaveLength(1); expect(audit[0].old_content).toBe(original);
      const messages = await read<Message[]>(actor, `/api/channels/${channel.channel_id}/messages?limit=100`);
      expect(messages.find(item => item.message_id === message.message_id)).toMatchObject({ content: updated, edited_at: expect.any(String) });
      await edited.locator('.edited-badge').click();
      await expect(edited.locator('.edit-history-content')).toHaveText([original, updated]);
      await expect(edited.locator('.edit-history-empty')).toHaveCount(0);
      await edited.locator('.edited-badge').click(); await expect(edited.locator('.edit-history')).toHaveCount(0);
    }
    expect(owner.errors).toEqual([]); expect(peer.errors).toEqual([]);
    await test.info().attach('actual-edit-audit-invariant', { contentType: 'application/json', body: Buffer.from(JSON.stringify({
      channelId: channel.channel_id, messageId: message.message_id, originalHistoryCount: 0,
      originalEditedBadgeCount: 0, editedHistoryCount: 1, independentMemberVerified: true,
      editedMessageEmptyHistoryBranchClaimed: false,
    })) });
  } finally { await owner.context.close(); await peer.context.close(); }
});
