import { expect, type Page } from '@playwright/test';
import { SEED, type WarmRole } from './roles';

export type SeedAuthor = Extract<WarmRole, 'founder' | 'second-member'>;

const FOUNDER_CONTENTS = [
  SEED.messages.plain, SEED.messages.long, SEED.messages.replyParent,
  SEED.messages.editedAfter, SEED.messages.reply,
];

/** Wait for real seed-message edit windows, without changing time or controls. */
export async function waitForSeedReadiness(page: Page, author: SeedAuthor) {
  const messages = page.locator('.message-view .message');
  const own = page.locator('.message-view .message.self');
  const ownLive = page.locator('.message-view .message.self:not(.deleted)');
  const expectedOwn = author === 'founder' ? FOUNDER_CONTENTS : [SEED.messages.fromOther];
  const assertCompleteHistory = async () => {
    // An empty, loading, failed, or wrong-role history must not pass merely
    // because there are no action buttons. All seven acknowledged seeds must
    // still render in their intended roles and persisted edit/delete states.
    await expect(messages, 'Seed history must contain all seven acknowledged messages').toHaveCount(7);
    await expect(messages.locator('.message-content')).toHaveText([
      ...FOUNDER_CONTENTS.slice(0, 4), 'This message was deleted',
      SEED.messages.reply, SEED.messages.fromOther,
    ]);
    await expect(own, `Seed history must belong to ${author}`).toHaveCount(author === 'founder' ? 6 : 1);
    await expect(ownLive.locator('.message-content')).toHaveText(expectedOwn);
    await expect(page.locator('.message-view .message:not(.self)')).toHaveCount(author === 'founder' ? 1 : 6);
    await expect(page.locator('.message-view .message.pending, .message-view .message.failed')).toHaveCount(0);
    await expect(messages.locator('.message-deleted-text')).toHaveCount(1);
    await expect(messages.locator('.edited-badge')).toHaveCount(1);
    await expect(messages.locator('.reply-context')).toHaveCount(1);
  };

  await assertCompleteHistory();
  const started = Date.now();
  const actionGroupsBefore = await ownLive.locator('.message-actions').count();
  // MessageBubble removes these nodes when its real 60-second timer expires.
  // Hidden actions still occupy flex space, so visibility is not sufficient.
  // Slow hosts may have already expired; initial presence is not required.
  await expect(ownLive.locator('.message-actions'), `${author}'s seed edit windows must expire`)
    .toHaveCount(0, { timeout: 75_000 });
  await assertCompleteHistory();
  return {
    author, messages: 7, ownMessages: author === 'founder' ? 6 : 1,
    ownLiveMessages: expectedOwn.length, actionGroupsBefore, actionGroupsAfter: 0,
    waitedMilliseconds: Date.now() - started, observedAt: new Date().toISOString(),
  };
}
