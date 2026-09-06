/**
 * Three people in one call.
 *
 * This is the test the SFU rework exists for, and it could not have passed
 * before it. `fan_out_rtp` used to write every sender's RTP into a single
 * outbound track per receiver, so with two remote senders their streams —
 * different SSRCs, independent encoder state — were interleaved onto one
 * track. A receiver cannot demultiplex that; VP8 carries inter-frame
 * references, so alternating packets from two encoders decodes to neither.
 * Calls were structurally limited to two participants and the client had no
 * per-sender track to attribute a tile to.
 *
 * Three real browser contexts join the same channel with fake media devices,
 * which is the only way to prove this: unit tests can assert the track map is
 * shaped correctly, but only a real call proves the renegotiation actually
 * reaches the peers already in it and that they answer.
 */

import { test, expect, type BrowserContext, type Page } from '@playwright/test';
import { landing, stabilize, selectChannel } from './nav';
import { SEED, storageStatePath, type WarmRole } from './roles';

/** Join `role` to the seeded voice channel and return its page. */
async function joinCall(
  browser: import('@playwright/test').Browser,
  role: WarmRole,
): Promise<{ context: BrowserContext; page: Page }> {
  const context = await browser.newContext({
    storageState: storageStatePath(role),
    permissions: ['camera', 'microphone'],
  });
  const page = await context.newPage();

  await landing(page, role);
  await selectChannel(page, SEED.channels.voice);
  await stabilize(page);

  await page.locator('.voice-join-btn').first().click();
  await expect(page.locator('.media-controls')).toBeVisible({ timeout: 45_000 });

  return { context, page };
}

/** How many remote tracks this page's session is actually receiving. */
async function remoteTrackSenders(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const tiles = document.querySelectorAll('.call-grid .call-tile .tile-name');
    return [...tiles].map((t) => t.textContent ?? '');
  });
}

test.describe('group calls', () => {
  test('three participants each see the other two, on their own tracks', async ({ browser }) => {
    test.setTimeout(240_000);

    const peers: { context: BrowserContext; page: Page }[] = [];
    try {
      // Serially, so each join exercises renegotiation against a room that
      // already has peers in it — which is the path that was missing.
      for (const role of ['founder', 'member', 'second-member'] as WarmRole[]) {
        peers.push(await joinCall(browser, role));
      }

      // Poll rather than sleep. Reaching the steady state takes an
      // offer/answer round trip per existing peer plus a roster poll on a 10s
      // interval, and a fixed wait either races that or pads every run by the
      // worst case.
      // One predicate, evaluated on a single snapshot each time: the whole
      // shape has to hold at once. Polling for the count and then re-reading
      // the names is two observations of a moving system, and they disagreed.
      for (const [i, { page }] of peers.entries()) {
        await expect
          .poll(
            async () => {
              const tiles = await remoteTrackSenders(page);
              const others = tiles.filter((t) => t !== 'You');
              return {
                tiles: tiles.length,
                self: tiles.filter((t) => t === 'You').length,
                // The whole point: the other two are distinguishable. While
                // every sender shared one track there was nothing to tell them
                // apart and both rendered the literal string "Participant".
                distinctOthers: new Set(others).size,
              };
            },
            {
              timeout: 60_000,
              message: `peer ${i} should see itself and two distinct others`,
            },
          )
          .toEqual({ tiles: 3, self: 1, distinctOthers: 2 });
      }
    } finally {
      for (const { context } of peers) {
        await context.close().catch(() => {});
      }
    }
  });

  test('a peer joining later appears for those already in the call', async ({ browser }) => {
    test.setTimeout(240_000);

    const peers: { context: BrowserContext; page: Page }[] = [];
    try {
      peers.push(await joinCall(browser, 'founder'));
      peers.push(await joinCall(browser, 'member'));

      await expect
        .poll(async () => (await remoteTrackSenders(peers[0].page)).length, {
          timeout: 60_000,
          message: 'the first peer should see the second',
        })
        .toBe(2);

      // The case renegotiation exists for: someone joins a call that is
      // already running. Without a server-initiated offer the peers already in
      // it never get a track for the newcomer, and the call stays frozen at
      // whoever it started with.
      peers.push(await joinCall(browser, 'second-member'));

      await expect
        .poll(async () => (await remoteTrackSenders(peers[0].page)).length, {
          timeout: 60_000,
          message: 'the first peer should gain a tile for the newcomer',
        })
        .toBe(3);
    } finally {
      for (const { context } of peers) {
        await context.close().catch(() => {});
      }
    }
  });
});
