/**
 * Multi-user real-time delivery — the WebSocket broadcast path that the
 * single-user suite never exercises. Two independent browser contexts (two
 * separate ZK identities) join #General; a message sent by A must arrive in
 * B's view via the server's WS fan-out. B receiving it is hard proof the send
 * actually persisted and broadcast (the optimistic UI alone can't fake a
 * second client).
 */
import { test, expect } from '@playwright/test';
import { completeStartup, joinAndSelectGeneral } from './helpers';

test.describe('Multi-user real-time delivery', () => {
  test('A → B message delivery over WebSocket', async ({ browser }) => {
    test.setTimeout(240_000); // two identity setups, each generating a Groth16 proof

    const ctxA = await browser.newContext();
    const ctxB = await browser.newContext();
    const a = await ctxA.newPage();
    const b = await ctxB.newPage();

    try {
      await completeStartup(a);
      await completeStartup(b);
      await joinAndSelectGeneral(a);
      await joinAndSelectGeneral(b);

      const msg = `multiuser-${Date.now()}`;
      const inputA = a.getByPlaceholder('Type a message...');
      await expect(inputA).toBeVisible({ timeout: 10_000 });
      await inputA.fill(msg);
      await a.getByRole('button', { name: 'Send' }).click();

      // A sees its own message and it is NOT marked failed.
      await expect(a.locator('.message', { hasText: msg })).toBeVisible({ timeout: 15_000 });
      await expect(a.locator('.message.failed', { hasText: msg })).toHaveCount(0);

      // B receives it purely via the WS broadcast — the real cross-user path.
      await expect(b.locator('.message', { hasText: msg })).toBeVisible({ timeout: 20_000 });
      await b.screenshot({ path: 'e2e-results/multiuser-b-received.png', fullPage: true });

      // And the reverse direction, to prove the channel is fully bidirectional.
      const reply = `reply-${Date.now()}`;
      const inputB = b.getByPlaceholder('Type a message...');
      await inputB.fill(reply);
      await b.getByRole('button', { name: 'Send' }).click();
      await expect(a.locator('.message', { hasText: reply })).toBeVisible({ timeout: 20_000 });
    } finally {
      await ctxA.close();
      await ctxB.close();
    }
  });
});
