/**
 * The surface manifest — every screen, dialog, overlay, popover and notice a
 * user can reach.
 *
 * This file is the coverage contract for the whole audit. `capture.spec.ts`
 * walks it; `manifest.spec.ts` asserts it stays in sync with the components
 * that actually exist, so adding a dialog without adding it here fails CI.
 *
 * Surfaces are grouped by journey stage in the order a real user meets them:
 * identity → server startup → registration → channels → messaging → voice →
 * settings → admin → federation → observability → agents → cross-cutting.
 * (Stage `01-install` is the desktop lane and lives in the Tauri harness — a
 * browser against the served SPA cannot reach it.)
 */

import { expect } from '@playwright/test';
import type { Surface } from './types';
import { SEED } from './roles';
import {
  openAdminDropdown,
  openAdminSection,
  openStatusBarDialog,
  openTab,
  selectChannel,
} from './nav';

/** Stub helper: make an endpoint return a fixed payload. */
function stub(pattern: string | RegExp, body: unknown, status = 200) {
  return async (page: import('@playwright/test').Page) => {
    await page.route(pattern, (route) =>
      route.fulfill({
        status,
        contentType: 'application/json',
        body: JSON.stringify(body),
      }),
    );
  };
}

/**
 * Put an extra server in the hub before the app boots.
 *
 * Two states in `KNOWN_UNCOVERED` needed a second registered server and so
 * had never been photographed: a placeholder whose registration failed, and
 * a switch that does not complete. The hub reads `annex-servers` from
 * IndexedDB, so seeding it directly is the whole trick — no server-side
 * state is touched and nothing leaks into the surfaces that follow.
 *
 * `identityId: ''` marks a placeholder the hub renders as failed once the
 * identity phase settles; a non-empty id that names no stored identity makes
 * `switchServer` throw on selection, which is the failure users hit when a
 * server's identity has been cleared out from under it.
 */
function seedSecondServer(identityId: string) {
  return async (page: import('@playwright/test').Page) => {
    await page.addInitScript((id) => {
      const req = indexedDB.open('annex-servers', 1);
      req.onupgradeneeded = () => {
        const db = req.result;
        if (!db.objectStoreNames.contains('servers')) {
          const store = db.createObjectStore('servers', { keyPath: 'id' });
          store.createIndex('identityId', 'identityId', { unique: false });
          store.createIndex('slug', 'slug', { unique: false });
        }
      };
      req.onsuccess = () => {
        const tx = req.result.transaction('servers', 'readwrite');
        tx.objectStore('servers').put({
          id: 'seeded-second-server',
          baseUrl: 'https://beta.example',
          slug: 'beta',
          label: 'Beta Station',
          identityId: id,
          personaId: null,
          accentColor: '#4c7fd4',
          vrpTopic: 'annex:server:beta:v1',
          lastConnectedAt: '2026-01-01T00:00:00.000Z',
          cachedSummary: null,
        });
      };
    }, identityId);
  };
}

/**
 * Post a message and return its bubble.
 *
 * Edit and delete are only offered within `EDIT_WINDOW_MS` (60s) of posting
 * (`MessageView.tsx`), so surfaces that need those controls cannot reuse the
 * fixtures written during setup — by capture time they are minutes old.
 */
async function postFreshMessage(page: import('@playwright/test').Page, text: string) {
  await page.getByPlaceholder('Type a message...').fill(text);
  await page.getByRole('button', { name: 'Send' }).click();
  const bubble = page.locator('.message', { hasText: text }).first();
  await expect(bubble).toBeVisible({ timeout: 20_000 });
  // Visible is not sent. The bubble is optimistic — it appears the moment the
  // frame leaves the device and stays there, marked `failed`, if the server
  // refuses. So this used to pass on a send that had not landed, and the
  // surface then photographed a failed message plus a composer error and
  // recorded it as the baseline. That is how an intermittent server-side
  // "database is locked" got into a committed screenshot instead of failing
  // the run: the harness could not tell a send from a non-send.
  await expect(bubble.locator('.failed-status')).toHaveCount(0, { timeout: 20_000 });
  await expect(page.locator('.composer-error')).toHaveCount(0);
  return bubble;
}

/**
 * A fixed 4×4 PNG, as bytes.
 *
 * Fixed rather than generated: the composer renders a staged attachment
 * through `FileReader` as a data URL, so anything varying between runs would
 * make the preview thumbnail a new image every time and the surface would
 * diff against itself forever.
 */
const TINY_PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAHElEQVQI12P4//8/w38GIAXDIBKE0DHxg' +
    'ljNBAAO9TXL0Y4OHwAAAABJRU5ErkJggg==',
  'base64',
);

/** Stage a file in the composer without sending it. */
async function stageAttachment(page: import('@playwright/test').Page, name: string) {
  await page.locator('.message-input input[type="file"]').setInputFiles({
    name,
    mimeType: 'image/png',
    buffer: TINY_PNG,
  });
  await expect(page.locator('.image-preview-bar')).toBeVisible({ timeout: 15_000 });
}

export const SURFACES: Surface[] = [
  // ─────────────────────── 02 · identity ───────────────────────
  {
    id: 'identity-setup',
    stage: '02-identity',
    title: 'Identity creation (first launch)',
    role: 'fresh',
    intent: 'The very first thing a new user sees. Makes zero network requests by design.',
    navigate: async () => {
      /* `landing()` already leaves us here. */
    },
  },
  {
    id: 'identity-setup-device-link-choose',
    stage: '02-identity',
    title: 'Link from another device — mode chooser',
    role: 'fresh',
    intent: 'Alternative onboarding path for users who already have an identity elsewhere.',
    navigate: async (page) => {
      await page.locator('.device-link-setup-btn').click();
      await expect(page.locator('.device-link-choose')).toBeVisible();
    },
  },
  {
    id: 'identity-setup-device-link-receive',
    stage: '02-identity',
    title: 'Link from another device — receive',
    role: 'fresh',
    intent:
      'The only device-link mode reachable before you have an identity. "Link a New Device" is ' +
      'disabled here — you cannot share an identity you do not have yet — but nothing on screen ' +
      'says so, which is a finding in its own right.',
    navigate: async (page) => {
      await page.locator('.device-link-setup-btn').click();
      await page.locator('.device-link-option').nth(1).click();
      await expect(page.locator('.device-link-receive')).toBeVisible();
    },
  },
  {
    id: 'device-link-share',
    stage: '08-user-settings',
    title: 'Link another device — share (QR + pairing code)',
    role: 'founder',
    intent:
      'Sharing requires an existing identity, so this is only reachable from the status bar ' +
      'once signed in. The QR image and pairing code are regenerated per run, so both are masked.',
    navigate: async (page) => {
      await openStatusBarDialog(page, 'Link');
      await page.locator('.device-link-option').first().click();
      await expect(page.locator('.device-link-share')).toBeVisible();
    },
    clip: '.dialog',
    // Mask only the regenerated values, not the whole panel — masking the
    // entire panel would leave the screenshot proving nothing about it. All
    // three derive from a fresh keypair per run: the QR image, the pairing
    // code, and the transfer-code textarea holding the encrypted payload.
    mask: ['.qr-container', '.pairing-code-value', '.transfer-code-value'],
  },

  // ─────────────────────── 03 · server startup ───────────────────────
  {
    id: 'startup-mode-choose',
    stage: '03-server-startup',
    title: 'Server selection',
    role: 'fresh',
    intent: 'Second onboarding gate: use this server, or connect to a different one.',
    navigate: async (page) => {
      await page.getByRole('button', { name: 'Create New Identity' }).click();
      await expect(page.getByRole('button', { name: 'Continue' })).toBeVisible({
        timeout: 60_000,
      });
    },
  },

  {
    id: 'server-hub-registration-failed',
    stage: '03-server-startup',
    title: 'A server whose registration never completed',
    role: 'founder',
    intent:
      'A placeholder left behind when joining a server did not finish. It has to be visibly ' +
      'distinct from a working server and it has to offer a way out — a dead icon you can ' +
      'neither enter nor remove is worse than no icon.',
    setup: seedSecondServer(''),
    navigate: async (page) => {
      const failed = page.locator('.server-hub-icon.failed');
      await expect(failed).toBeVisible({ timeout: 20_000 });
      await failed.hover();
      await expect(page.locator('.server-hub-failed-actions')).toBeVisible({ timeout: 10_000 });
    },
    clip: '.server-hub',
  },

  {
    id: 'startup-remote-bad-address',
    stage: '03-server-startup',
    title: 'Server selection — the address is not one',
    role: 'fresh',
    intent:
      'The third failure on this screen and the commonest: a typo, or a pasted URL with a ' +
      'scheme the app does not speak. It never reaches the network, so it is the one branch ' +
      'that can only be judged on what it says — and it used to say "Invalid URL format." ' +
      'while its two siblings echoed the address and explained what to do.',
    navigate: async (page) => {
      await page.getByRole('button', { name: 'Create New Identity' }).click();
      await expect(page.getByRole('button', { name: 'Continue' })).toBeVisible({
        timeout: 60_000,
      });
      await page.getByPlaceholder('annex.example.com').fill('ftp://not-a-web-server');
      await page.getByRole('button', { name: 'Connect' }).click();
      await expect(page.locator('.form-error')).toBeVisible({ timeout: 15_000 });
    },
  },

  {
    id: 'startup-remote-unreachable',
    stage: '03-server-startup',
    title: 'Server selection — the address entered does not respond',
    role: 'fresh',
    intent:
      'The second onboarding screen had exactly one surface, its happy path. This is the ' +
      'failure a new user actually hits: they type a server address and it is wrong, or ' +
      'down. The message has to name the address and say what to do, and the form has to ' +
      'stay filled so it can be corrected rather than retyped.',
    setup: async (page) => {
      // Nothing answers at the address, which is what an unreachable server
      // looks like to `fetchWithTimeout` — a network failure, not a status.
      await page.route('**/api/public/server/summary', (route) => route.abort('connectionrefused'));
    },
    navigate: async (page) => {
      await page.getByRole('button', { name: 'Create New Identity' }).click();
      await expect(page.getByRole('button', { name: 'Continue' })).toBeVisible({
        timeout: 60_000,
      });
      await page.getByPlaceholder('annex.example.com').fill('does-not-exist.invalid');
      await page.getByRole('button', { name: 'Connect' }).click();
      await expect(page.locator('.form-error')).toBeVisible({ timeout: 20_000 });
    },
    waive: {
      network:
        'the unreachable address is the condition under test — the aborted request is what the ' +
        'surface exists to produce, not an incidental failure.',
      console: 'the browser logs the aborted fetch to the console as well as the network log',
    },
  },

  // ─────────────────────── 04 · registration ───────────────────────
  {
    id: 'registration-password-prompt',
    stage: '04-registration',
    title: 'Server password prompt',
    role: 'fresh',
    intent:
      'The join gate for `access_mode: password` servers. Its submit control did nothing, and the ' +
      'value reached the registration effect on every keystroke instead of on submit.',
    setup: stub('**/api/public/server/summary', {
      slug: 'audit',
      label: 'Annex Server',
      description: '',
      public_url: '',
      members_by_type: {},
      total_active_members: 0,
      channel_count: 1,
      federation_peer_count: 0,
      active_agent_count: 0,
      access_mode: 'password',
    }),
    navigate: async (page) => {
      await page.getByRole('button', { name: 'Create New Identity' }).click();
      await page.getByRole('button', { name: 'Continue' }).click();
      await expect(page.locator('.password-prompt')).toBeVisible({ timeout: 60_000 });
    },
  },
  {
    id: 'registration-progress',
    stage: '04-registration',
    title: 'Registering / proving progress',
    role: 'fresh',
    intent:
      'What a first-time user stares at for 30-60s while the Groth16 proof runs. The elapsed ' +
      'counter exists so the static label does not read as frozen.',
    navigate: async (page) => {
      await page.getByRole('button', { name: 'Create New Identity' }).click();
      await page.getByRole('button', { name: 'Continue' }).click();
      await expect(page.locator('.phase-status')).toBeVisible({ timeout: 30_000 });
    },
    // The phase label and elapsed counter both advance while we look at them.
    mask: ['.phase-label', '.phase-elapsed'],
    reportOnly: true,
  },
  {
    id: 'registration-error',
    stage: '04-registration',
    title: 'Registration refused by the server',
    role: 'fresh',
    intent:
      'A refusal must be shown once with its reason, not retried five times with backoff — that ' +
      'burned six of the ten-per-minute registration budget on a single wrong answer.',
    setup: stub('**/api/registry/register', { error: 'server is full' }, 403),
    navigate: async (page) => {
      await page.getByRole('button', { name: 'Create New Identity' }).click();
      await page.getByRole('button', { name: 'Continue' }).click();
      await expect(page.locator('.error-message')).toBeVisible({ timeout: 60_000 });
    },
    waive: {
      network: 'the 403 is injected deliberately to reach the refusal state',
      console: 'the browser logs the injected 403 to the console as well',
    },
  },

  // ─────────────────────── 05 · channels ───────────────────────
  {
    id: 'chat-main',
    stage: '05-channels',
    title: 'Main chat view (three columns)',
    role: 'founder',
    intent: 'The app’s home screen — channel list, message area, member list, status bar.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
    },
  },
  {
    id: 'channel-list-all-types',
    stage: '05-channels',
    title: 'Channel list with all five channel types',
    role: 'founder',
    intent: 'Every channel type icon (text, voice, hybrid, agent, broadcast) rendered together.',
    clip: '.sidebar-left',
    navigate: async (page) => {
      await expect(page.locator('.channel-item')).toHaveCount(7, { timeout: 15_000 });
    },
  },
  {
    id: 'channel-empty-state',
    stage: '05-channels',
    title: 'Channel with no messages',
    role: 'founder',
    intent: 'Empty state must read as “nothing here yet”, not as a failure.',
    navigate: async (page) => {
      await selectChannel(page, SEED.emptyChannel);
    },
  },
  {
    id: 'create-channel-dialog',
    stage: '05-channels',
    title: 'Create channel dialog',
    role: 'founder',
    intent: 'Moderator-only channel creation, including type picker and federation toggle.',
    navigate: async (page) => {
      await page.locator('.create-channel-btn').click();
      await expect(page.locator('.dialog')).toBeVisible();
    },
    clip: '.dialog',
  },
  {
    id: 'create-channel-rejected',
    stage: '05-channels',
    title: 'The server refused the channel',
    role: 'founder',
    intent:
      'A name that collides with an existing channel is the ordinary way this fails, and the ' +
      'dialog has to stay open with the typed values intact — closing on a refusal would ' +
      'throw away what the user wrote and leave them guessing whether it worked.',
    setup: async (page) => {
      // POST only. The dialog also GETs the channel list after a successful
      // create, and stubbing every method would break the list behind it.
      await page.route('**/api/channels', (route) =>
        route.request().method() === 'POST'
          ? route.fulfill({
              status: 409,
              contentType: 'application/json',
              body: JSON.stringify({ error: 'A channel with that name already exists.' }),
            })
          : route.fallback(),
      );
    },
    navigate: async (page) => {
      await page.locator('.create-channel-btn').click();
      await expect(page.locator('.dialog')).toBeVisible();
      await page.locator('#channel-name, .dialog input[type="text"]').first().fill('general');
      await page.getByRole('button', { name: /^create$/i }).click();
      await expect(page.locator('.dialog .error-message')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.dialog',
    waive: {
      network: 'the 409 is the stub this surface installs — it is the condition under test.',
      console: 'the browser logs the injected 409 to the console as well',
    },
  },

  // ─────────────────────── 06 · messaging ───────────────────────
  {
    id: 'message-list-populated',
    stage: '06-messaging',
    title: 'Message list with seeded conversation',
    role: 'founder',
    intent: 'Realistic message density — plain, long-wrapping, and reply-parent messages.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await expect(page.locator('.message').first()).toBeVisible({ timeout: 15_000 });
    },
    clip: '.chat-area',
  },
  {
    id: 'message-send-failed',
    stage: '06-messaging',
    title: 'A message the server refused',
    role: 'founder',
    intent:
      'The two halves of a rejected send, which are set together by one WebSocket error frame ' +
      'and had no surface between them: the bubble keeps its text and is marked failed with ' +
      'retry and dismiss beside it, and the composer says why. Getting this wrong is the ' +
      'defect this codebase produces most — a send that failed rendered as one that worked.',
    setup: async (page) => {
      // Intercept the socket rather than the HTTP layer: sending goes over
      // the WebSocket, which route stubbing cannot reach, and that is why
      // both of these states sat in `KNOWN_UNCOVERED`.
      //
      // Everything is proxied to the real server except the send itself,
      // which is answered with the error frame and NOT forwarded. That
      // matters beyond this surface: the channel is shared, so a message
      // that reached the server would be photographed by every messaging
      // surface captured after this one.
      await page.routeWebSocket(/\/ws/, (ws) => {
        const server = ws.connectToServer();
        ws.onMessage((raw) => {
          const text = typeof raw === 'string' ? raw : raw.toString();
          let frame: { type?: string; channelId?: string; clientRequestId?: string };
          try {
            frame = JSON.parse(text);
          } catch {
            server.send(raw);
            return;
          }
          if (frame.type === 'message' && frame.clientRequestId) {
            ws.send(
              JSON.stringify({
                type: 'error',
                channelId: frame.channelId,
                clientRequestId: frame.clientRequestId,
                message: 'Message rejected: you are not a member of this channel.',
              }),
            );
            return;
          }
          server.send(raw);
        });
        server.onMessage((raw) => ws.send(raw));
      });
    },
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await page.getByPlaceholder('Type a message...').fill('this one does not land');
      await page.getByRole('button', { name: 'Send' }).click();
      await expect(page.locator('.failed-status')).toBeVisible({ timeout: 20_000 });
      await expect(page.locator('.composer-error')).toBeVisible({ timeout: 20_000 });
    },
    clip: '.chat-area',
  },

  {
    id: 'message-search-open',
    stage: '06-messaging',
    title: 'Message search (expanded)',
    role: 'founder',
    intent: 'Ctrl/Cmd+F search bar — one of only two places Escape is handled today.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await page.locator('.search-toggle-btn').click();
      await expect(page.locator('.search-form')).toBeVisible();
    },
    // Clipped to the search panel, for the reason `message-search-results`
    // above already documents — it turned out to apply to its siblings too.
    //
    // The cause is now known: the encryption bar's button label starts with an
    // emoji, and the width that glyph resolves to depends on when the system
    // emoji font becomes available. A 32px swing in the button moved where the
    // bar's text wrapped, which changed the bar's height, which shifted every
    // message below it by 4px — 0.0075 of a mobile viewport, under the 0.005
    // tolerance on some runs and over it on others. The subject here is the
    // search panel; the audits still run against the whole page regardless.
    clip: '.message-search',
    // The message column behind the panel is masked as a whole.
    //
    // Clipping to `.message-search` was not enough: Playwright draws masks at
    // the masked elements' PAGE coordinates, even for elements outside the
    // clip, so the per-message `.sender` and avatar masks in the auto-scrolling
    // column behind the panel land at different places inside the crop when
    // that column settles a pixel or two differently. The panel has
    // transparent regions, so they show through.
    //
    // Masking `.message-view` itself replaces all of that with one rectangle
    // whose position follows a layout container rather than scroll state. The
    // search panel — the actual subject — is still diffed.
    mask: ['.message-view'],
  },

  {
    id: 'message-edit-mode',
    stage: '06-messaging',
    title: 'Editing your own message',
    role: 'founder',
    intent:
      'Inline edit with the live countdown showing how long the edit window has left. The ' +
      'countdown is masked because it ticks; the edit affordance around it is the point.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      // Edit and delete are only offered inside `EDIT_WINDOW_MS` (60s) of
      // posting, so the seeded messages — written minutes earlier during
      // setup — no longer show either control. Post a fresh one.
      const bubble = await postFreshMessage(page, 'Message posted for the edit-mode capture.');
      await bubble.hover();
      await bubble.locator('.edit-btn').click();
      await expect(page.locator('.message-edit-input')).toBeVisible();
    },
    clip: '.chat-area',
    mask: ['.edit-countdown'],
  },
  {
    id: 'message-edit-refused',
    stage: '06-messaging',
    title: 'An edit the server would not accept',
    role: 'founder',
    intent:
      'The commonest way an edit fails is the 60-second window closing, and until the frame ' +
      'carried a correlation id there was no way to tell which operation an error belonged ' +
      'to. The correction stayed on screen looking saved and came back undone on the next ' +
      'reload. What this photographs is the undo: the original text, and the reason.',
    setup: async (page) => {
      // Same shape as `message-send-failed`: proxy everything, answer the
      // edit with the refusal the server would send, and never forward it —
      // the channel is shared, so an edit that landed would be visible to
      // every messaging surface captured after this one.
      await page.routeWebSocket(/\/ws/, (ws) => {
        const server = ws.connectToServer();
        ws.onMessage((raw) => {
          const text = typeof raw === 'string' ? raw : raw.toString();
          let frame: { type?: string; clientRequestId?: string };
          try {
            frame = JSON.parse(text);
          } catch {
            server.send(raw);
            return;
          }
          if (frame.type === 'edit_message') {
            ws.send(
              JSON.stringify({
                type: 'error',
                clientRequestId: frame.clientRequestId,
                message: 'Edit window has expired',
              }),
            );
            return;
          }
          server.send(raw);
        });
        server.onMessage((raw) => ws.send(raw));
      });
    },
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      const bubble = await postFreshMessage(page, 'Message posted for the refused-edit capture.');
      await bubble.hover();
      await bubble.locator('.edit-btn').click();
      await page.locator('.message-edit-input').fill('this correction is refused');
      await page.locator('.msg-edit-save').click();
      await expect(page.locator('.message-action-error')).toBeVisible({ timeout: 20_000 });
    },
    clip: '.chat-area',
  },
  {
    id: 'message-delete-confirm',
    stage: '06-messaging',
    title: 'Delete confirmation (second click)',
    role: 'founder',
    intent:
      'Deletion is a two-click confirm rather than a dialog. Capturing the armed state proves it ' +
      'is actually distinguishable from the idle one.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      // Same 60s window as edit — see above.
      const bubble = await postFreshMessage(page, 'Message posted for the delete-confirm capture.');
      await bubble.hover();
      await bubble.locator('.delete-btn').click();
      await expect(bubble.locator('.delete-btn.confirming')).toBeVisible();
    },
    clip: '.chat-area',
    mask: ['.edit-countdown'],
  },
  {
    id: 'message-edited-badge',
    stage: '06-messaging',
    title: 'A message that has been edited',
    role: 'founder',
    intent:
      'The `(edited)` marker on a message whose text has changed. The seeder posted a message ' +
      'literally named "will be edited by the audit seeder" and then never edited it, so this ' +
      'badge — the only signal that message history is not what it appears — was never captured.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await expect(
        page.locator('.message', { hasText: SEED.messages.editedAfter.slice(0, 40) })
          .first()
          .locator('.edited-badge'),
      ).toBeVisible({ timeout: 15_000 });
    },
    clip: '.chat-area',
  },
  {
    id: 'message-edit-history',
    stage: '06-messaging',
    title: 'Edit history for an edited message',
    role: 'founder',
    intent:
      'Every prior version of a message, which is the transparency counterpart to allowing edits ' +
      'at all. It is also the surface behind the IDOR that served other channels\' drafts, and ' +
      'behind the redaction leak where deleting a message kept them — both fixed, neither visible ' +
      'here until now.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      const bubble = page
        .locator('.message', { hasText: SEED.messages.editedAfter.slice(0, 40) })
        .first();
      await bubble.locator('.edited-badge').click();
      await expect(page.locator('.edit-history')).toBeVisible({ timeout: 15_000 });
      await expect(page.locator('.edit-history-loading')).toHaveCount(0);
    },
    clip: '.chat-area',
    mask: ['.edit-history-time'],
  },
  {
    id: 'message-edit-history-error',
    stage: '06-messaging',
    title: 'Edit history the server could not return',
    role: 'founder',
    intent:
      'The failure branch of the panel above. It used to write the failed fetch into the history ' +
      'as an empty array, which renders as "No edit history found" — a claim that the message was ' +
      'never edited, made on a message carrying an "(edited)" badge. The audit trail is the one ' +
      'thing this panel exists to be trusted about, so the failure has to look like a failure.',
    setup: stub(
      '**/api/channels/*/messages/*/edits',
      { error: 'internal server error' },
      500,
    ),
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      const bubble = page
        .locator('.message', { hasText: SEED.messages.editedAfter.slice(0, 40) })
        .first();
      await bubble.locator('.edited-badge').click();
      await expect(page.locator('.edit-history-error')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.chat-area',
    waive: {
      network:
        'the 500 is the stub this surface installs on purpose — it is the condition under test, ' +
        'not an incidental failed request.',
      console: 'the browser logs the injected 500 to the console as well',
    },
  },
  {
    id: 'message-deleted-tombstone',
    stage: '06-messaging',
    title: 'A deleted message in the timeline',
    role: 'founder',
    intent:
      'What is left after a delete: a tombstone holding the position, not a gap. Proving the row ' +
      'still renders matters because the alternative — silently vanishing — makes a conversation ' +
      'read as though it never happened.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await expect(page.locator('.message-deleted-text').first()).toBeVisible({
        timeout: 15_000,
      });
    },
    clip: '.chat-area',
  },
  {
    id: 'message-reply-rendered',
    stage: '06-messaging',
    title: 'A message rendered as a reply',
    role: 'founder',
    intent:
      'The quoted parent above a reply. `message-reply-composer` captured the composer with a ' +
      'reply armed, but nothing ever sent one, so `.reply-context` — the half a reader actually ' +
      'sees — was uncaptured and SEED.messages.reply was dead text.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await expect(page.locator('.reply-context').first()).toBeVisible({ timeout: 15_000 });
    },
    clip: '.chat-area',
  },
  {
    id: 'message-reply-composer',
    stage: '06-messaging',
    title: 'Composer with a reply in progress',
    role: 'founder',
    intent: 'The reply bar above the composer, showing who is being replied to.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      const bubble = page.locator('.message', { hasText: SEED.messages.replyParent }).first();
      await bubble.hover();
      await bubble.locator('.reply-btn').click();
      await expect(page.locator('.reply-bar')).toBeVisible();
    },
    clip: '.chat-area',
  },
  {
    id: 'message-from-another-member',
    stage: '06-messaging',
    title: 'A message from somebody else',
    role: 'founder',
    intent:
      'The other half of every conversation, and it had never been captured. All fixture ' +
      'messages were the founder\u2019s own, so every bubble in every screenshot carried `.self` ' +
      'and the incoming path — left-aligned bubble, avatar, sender identity, username resolution ' +
      '— was rendered by nothing and audited by nothing. Posted by the `second-member` role, ' +
      'which the run already warms for the three-party call in `group-call.spec.ts` but which ' +
      'no capture surface had ever used.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      const incoming = page
        .locator('.message:not(.self)', { hasText: SEED.messages.fromOther.slice(0, 40) })
        .first();
      await expect(incoming).toBeVisible({ timeout: 15_000 });
      await incoming.scrollIntoViewIfNeeded();
    },
    clip: '.chat-area',
  },
  {
    id: 'message-reply-to-another-member',
    stage: '06-messaging',
    title: 'Replying to somebody else',
    role: 'founder',
    intent:
      'The reply bar naming a different person. It rendered `sender_pseudonym.slice(0, 12)` ' +
      'unconditionally — a hex string, even when that person\u2019s username was already ' +
      'resolved and shown on the very message being replied to. The fix had no surface that ' +
      'could see it, because there was never a message from anyone but yourself to reply to.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      const incoming = page
        .locator('.message:not(.self)', { hasText: SEED.messages.fromOther.slice(0, 40) })
        .first();
      await expect(incoming).toBeVisible({ timeout: 15_000 });
      await incoming.hover();
      await incoming.locator('.reply-btn').click();
      await expect(page.locator('.reply-bar')).toBeVisible();
    },
    clip: '.chat-area',
  },
  {
    id: 'composer-attachment-staged',
    stage: '06-messaging',
    title: 'An attachment staged in the composer',
    role: 'founder',
    intent:
      'What a user sees between choosing a file and sending it: thumbnail, name, size, category ' +
      'and a way to back out. The whole attach-and-send flow was uncaptured, so this step — the ' +
      'only chance to notice you picked the wrong file — had never been looked at.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await stageAttachment(page, 'audit-fixture.png');
    },
    clip: '.chat-area',
  },
  {
    id: 'composer-upload-failed',
    stage: '06-messaging',
    title: 'An upload the server refused',
    role: 'founder',
    intent:
      'The upload fails after the user has already committed to sending. The staged file must ' +
      'survive so the send can be retried, rather than the composer emptying and leaving them ' +
      'to guess whether anything was sent.',
    setup: stub('**/api/channels/*/upload', { error: 'storage unavailable' }, 500),
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await stageAttachment(page, 'rejected.png');
      await page.getByRole('button', { name: 'Send' }).click();
      await expect(page.locator('.upload-error-bar')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.chat-area',
    waive: {
      network:
        'the 500 is the stub this surface installs on purpose — it is the condition under test, ' +
        'not an incidental failed request.',
      console: 'the browser logs the injected 500 to the console as well',
    },
  },
  {
    id: 'message-image-lightbox',
    stage: '06-messaging',
    title: 'An uploaded image at full size',
    role: 'founder',
    intent:
      'The lightbox over the whole app — the one surface that covers everything else, and the ' +
      'only way out of it is a close button and a click on the backdrop. Worth a keyboard and ' +
      'contrast pass precisely because it takes over the screen.',
    // Deliberately unstubbed: a real upload against the real endpoint.
    //
    // The first version stubbed the upload to return a made-up URL. That put a
    // real message carrying a URL nothing serves into the shared channel, and
    // every later surface that opened it recorded a 404 — 58 findings from one
    // surface, none of them about the app. Uploading for real leaves behind a
    // message whose image actually loads, and exercises the upload path
    // end to end into the bargain.
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await stageAttachment(page, 'lightbox.png');
      await page.getByRole('button', { name: 'Send' }).click();
      const image = page.locator('.message-inline-image').last();
      await expect(image).toBeVisible({ timeout: 20_000 });
      await image.click();
      await expect(page.locator('.image-lightbox')).toBeVisible({ timeout: 15_000 });
    },
  },
  {
    id: 'message-search-results',
    stage: '06-messaging',
    title: 'Message search with results',
    role: 'founder',
    intent: 'The results listbox, including how a hit renders sender and timestamp.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await page.locator('.search-toggle-btn').click();
      await page.locator('.search-form input').fill('audit');
      await page.locator('.search-form input').press('Enter');
      await expect(page.locator('.search-results, .search-no-results')).toBeVisible({
        timeout: 15_000,
      });
    },
    // Clipped to the search panel rather than capturing the whole page.
    //
    // Full-page, this was the only surface that flipped between pass and
    // fail on identical commits — and only at the mobile viewport, which has
    // the least room. The masks sit on per-result elements, so their geometry
    // follows the results list; any variation in it perturbed the entire
    // image rather than the region under test. The surface is about how a hit
    // renders, so the page behind it is not evidence, it is noise.
    clip: '.message-search',
    // `.message-view` for the same reason its three siblings mask it: the
    // scrolling column behind the panel moves its masks inside the crop.
    mask: ['.search-result-time', '.search-result-sender', '.message-view'],
  },
  {
    id: 'message-search-no-results',
    stage: '06-messaging',
    title: 'Message search with no matches',
    role: 'founder',
    intent: 'Empty-result state — must read as "nothing matched", not as a failure.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await page.locator('.search-toggle-btn').click();
      await page.locator('.search-form input').fill('zzzznotpresentzzzz');
      await page.locator('.search-form input').press('Enter');
      await expect(page.locator('.search-no-results')).toBeVisible({ timeout: 15_000 });
    },
    // Clipped to the search panel, for the reason `message-search-results`
    // above already documents — it turned out to apply to its siblings too.
    //
    // The cause is now known: the encryption bar's button label starts with an
    // emoji, and the width that glyph resolves to depends on when the system
    // emoji font becomes available. A 32px swing in the button moved where the
    // bar's text wrapped, which changed the bar's height, which shifted every
    // message below it by 4px — 0.0075 of a mobile viewport, under the 0.005
    // tolerance on some runs and over it on others. The subject here is the
    // search panel; the audits still run against the whole page regardless.
    clip: '.message-search',
    // The message column behind the panel is masked as a whole.
    //
    // Clipping to `.message-search` was not enough: Playwright draws masks at
    // the masked elements' PAGE coordinates, even for elements outside the
    // clip, so the per-message `.sender` and avatar masks in the auto-scrolling
    // column behind the panel land at different places inside the crop when
    // that column settles a pixel or two differently. The panel has
    // transparent regions, so they show through.
    //
    // Masking `.message-view` itself replaces all of that with one rectangle
    // whose position follows a layout container rather than scroll state. The
    // search panel — the actual subject — is still diffed.
    mask: ['.message-view'],
  },
  {
    id: 'message-search-failed',
    stage: '06-messaging',
    title: 'Message search the server could not answer',
    role: 'founder',
    intent:
      'The counterpart to message-search-no-results, and the reason that one exists. Both ' +
      'end with an empty result list, and only one of them means "nothing matched". A search ' +
      'that 500s must not read as a fact about the archive.',
    setup: stub('**/api/messages/search*', { error: 'internal server error' }, 500),
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await page.locator('.search-toggle-btn').click();
      await page.locator('.search-form input').fill('anything');
      await page.locator('.search-form input').press('Enter');
      await expect(page.locator('.search-error')).toBeVisible({ timeout: 15_000 });
    },
    waive: {
      network: 'the 500 is the stub this surface installs — it is the condition under test',
      console: 'the browser logs the injected 500 to the console as well',
    },
    // Clipped like its three siblings. Written without this, it failed
    // verification on its very first run for the reason those were clipped
    // for: the encryption bar's emoji-width swing shifts the message column
    // behind the panel.
    clip: '.message-search',
    // The message column behind the panel is masked as a whole.
    //
    // Clipping to `.message-search` was not enough: Playwright draws masks at
    // the masked elements' PAGE coordinates, even for elements outside the
    // clip, so the per-message `.sender` and avatar masks in the auto-scrolling
    // column behind the panel land at different places inside the crop when
    // that column settles a pixel or two differently. The panel has
    // transparent regions, so they show through.
    //
    // Masking `.message-view` itself replaces all of that with one rectangle
    // whose position follows a layout container rather than scroll state. The
    // search panel — the actual subject — is still diffed.
    mask: ['.message-view'],
  },
  {
    id: 'message-search-partial-no-results',
    stage: '06-messaging',
    title: 'Message search that did not reach the whole archive',
    role: 'founder',
    intent:
      'The third empty-result state, and the one the panel used to get wrong. Bodies are ' +
      'encrypted at rest, so the server decrypts a bounded recent window per channel and ' +
      'filters in memory — a match older than the window is never seen. Rendered as "No ' +
      'messages found", that is the client stating something about the archive on behalf of ' +
      'a server that read only the top of it.',
    // Stubbed rather than seeded: reaching this state for real needs 1000
    // messages in a channel every later surface then shares.
    setup: stub('**/api/messages/search*', {
      results: [],
      complete: false,
      scanned_per_channel: 1000,
    }),
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await page.locator('.search-toggle-btn').click();
      await page.locator('.search-form input').fill('retrospective');
      await page.locator('.search-form input').press('Enter');
      await expect(page.locator('.search-no-results')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.message-search',
    mask: ['.message-view'],
  },
  {
    id: 'message-search-partial-with-results',
    stage: '06-messaging',
    title: 'Message search with results it cannot vouch for',
    role: 'founder',
    intent:
      'Same partial scan, but it found something. Someone reading a short list and concluding ' +
      '"that is all of it" needs the same caveat the empty case gets — the note sits under the ' +
      'results, qualifying them, rather than above where it would read as a failure.',
    setup: stub('**/api/messages/search*', {
      results: [
        {
          id: 4242,
          server_id: 1,
          channel_id: SEED.defaultChannel,
          message_id: 'audit-partial-search-hit',
          sender_pseudonym: 'psn-audit-partial-search-0001',
          content: 'quarterly retrospective notes are in the shared drive',
          reply_to_message_id: null,
          created_at: '2026-02-01 09:00:00',
          expires_at: null,
          edited_at: null,
          deleted_at: null,
        },
      ],
      complete: false,
      scanned_per_channel: 1000,
    }),
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await page.locator('.search-toggle-btn').click();
      await page.locator('.search-form input').fill('retrospective');
      await page.locator('.search-form input').press('Enter');
      await expect(page.locator('.search-coverage-note')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.message-search',
    // `.search-result-time` renders through `toLocaleString`, and the sender
    // is a pseudonym in a proportional font — both are masked on
    // `message-search-results` for the same reasons.
    mask: ['.search-result-time', '.search-result-sender', '.message-view'],
  },
  {
    id: 'message-edit-history-empty',
    stage: '06-messaging',
    title: 'Edit history the server says is genuinely empty',
    role: 'founder',
    intent:
      'The third state of the same panel. "No edit history found" is the correct thing to ' +
      'say when the server really returns none — the defect was ever saying it for a ' +
      'failure. Capturing the honest empty case beside the error case is what keeps them ' +
      'visibly different.',
    setup: stub('**/api/channels/*/messages/*/edits', []),
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      const bubble = page
        .locator('.message', { hasText: SEED.messages.editedAfter.slice(0, 40) })
        .first();
      await bubble.locator('.edited-badge').click();
      await expect(page.locator('.edit-history-empty')).toBeVisible({ timeout: 15_000 });
    },
    // `.edit-history`, not the `.chat-area` its three siblings use.
    //
    // `.chat-area` contains the encryption bar AND the message column, so a
    // change in the bar's height still moves everything inside the clip — it
    // narrows the picture without removing the exposure. Those three have been
    // stable so far and are left alone rather than re-recorded for a fault
    // they have not shown; when one of them next needs re-recording it should
    // move here too.
    clip: '.edit-history',
  },
  {
    id: 'channel-encryption-failed-to-enable',
    stage: '06-messaging',
    title: 'Turning on encryption, refused by the server',
    role: 'founder',
    intent:
      'The bar offers one irreversible action and had no captured failure state. Turning on ' +
      'E2E either happened or it did not, and a moderator who is told nothing will press the ' +
      'button again — on a channel that may or may not already be encrypted.',
    setup: stub('**/api/channels/*/e2e', { error: 'could not enable encryption' }, 500),
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await page.locator('.channel-encryption-bar button').click();
      await expect(page.locator('.channel-encryption-error')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.channel-encryption-bar',
    waive: {
      network: 'the 500 is the stub this surface installs — it is the condition under test',
      console: 'the browser logs the injected 500 to the console as well',
    },
  },
  {
    id: 'channel-encryption-bar-cta',
    stage: '06-messaging',
    title: 'End-to-end encryption call to action',
    role: 'founder',
    intent:
      'Moderator-only prompt to turn on E2E for a channel. Non-moderators never see this bar.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await expect(page.locator('.channel-encryption-bar')).toBeVisible();
    },
    clip: '.channel-encryption-bar',
  },
  {
    id: 'channel-encryption-enabled',
    stage: '06-messaging',
    title: 'An encrypted channel, key in hand',
    role: 'founder',
    intent:
      'The headline feature actually switched on, which nothing captured before: only the ' +
      '"turn it on" prompt and the non-moderator hidden case were reachable. This is the state ' +
      'the other two encryption surfaces are measured against — the one where the user can read ' +
      'the channel.',
    setup: async (page) => {
      // Hermetic on purpose. Left to the real server this surface passed at
      // the first viewport and failed at the other three: provisioning a key
      // writes wraps to the shared DB, and the next context has a fresh
      // device key that can open none of them, so the channel came back as
      // `has_key: true` with nothing for us — the pending state, captured
      // under the name of the ready one. Stubbing both key routes makes each
      // viewport mint its own key and reach the same state, and stops this
      // surface leaving key material behind for the ones after it.
      await page.route('**/api/channels/*/e2e', (route) =>
        route.fulfill({ status: 200, contentType: 'application/json', body: '{"e2e_enabled":true}' }),
      );
      await page.route('**/api/channels/*/key-status', (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: '{"has_key":false,"max_epoch":0}',
        }),
      );
      await page.route('**/api/channels/*/key-wraps', (route) =>
        route.request().method() === 'POST'
          ? route.fulfill({
              status: 200,
              contentType: 'application/json',
              body: '{"status":"ok","inserted":1}',
            })
          : route.fulfill({ status: 200, contentType: 'application/json', body: '{"wraps":[]}' }),
      );
    },
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await expect(page.locator('.channel-encryption-bar.encrypted')).toBeVisible({
        timeout: 15_000,
      });
    },
    clip: '.channel-encryption-bar',
  },
  {
    id: 'channel-encryption-key-pending',
    stage: '06-messaging',
    title: 'Encrypted, and not admitted yet',
    role: 'founder',
    intent:
      'You are in an encrypted channel whose key nobody has sealed to you yet, so every message ' +
      'reads "🔒 encrypted message (no key)". This used to render under the reassuring green bar ' +
      'above — true, and useless. The wait ends on its own when any key-holder next opens the ' +
      'channel, and the bar now says so rather than leaving it looking like a dead end.',
    setup: async (page) => {
      // The channel is encrypted and HAS key material, but none of it is
      // sealed to us — exactly the shape that makes `resolveChannelKey`
      // raise `E2eKeyPendingError`.
      await page.route('**/api/channels/*/e2e', (route) =>
        route.fulfill({ status: 200, contentType: 'application/json', body: '{"e2e_enabled":true}' }),
      );
      await page.route('**/api/channels/*/key-wraps', (route) =>
        route.fulfill({ status: 200, contentType: 'application/json', body: '{"wraps":[]}' }),
      );
      await page.route('**/api/channels/*/key-status', (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: '{"has_key":true,"max_epoch":1}',
        }),
      );
    },
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await expect(page.locator('.channel-encryption-bar.key-pending')).toBeVisible({
        timeout: 15_000,
      });
    },
    // Diffed again, after two runs of identical code disagreed here.
    //
    // The difference was one constant-width rectangle spanning columns
    // 368-674 that had moved horizontally — no text moved, and the crop was
    // 708x123 in both. The bar contains no element in the mask set, so that
    // box could only be a mask overlay from something else on the page,
    // painted at its own page coordinates into a crop it has no business
    // being in. The surface was made `reportOnly` on that evidence, which
    // was the right read of a harness defect and the wrong remedy: it gave
    // up the pixel diff instead of fixing the masks.
    //
    // `maskLocators` now scopes masks to the clipped element, so a clipped
    // capture is painted only with masks that belong inside the picture.
    // The overlay that moved is no longer drawn at all.
    clip: '.channel-encryption-bar',
  },
  {
    id: 'channel-encryption-key-failed',
    stage: '06-messaging',
    title: 'Encrypted, and the key would not load',
    role: 'founder',
    intent:
      'The other half. A key that cannot be fetched at all is not a wait — it will not resolve, ' +
      'and sending stays blocked. It used to be indistinguishable from the state above because ' +
      'one bare catch handled both.',
    setup: async (page) => {
      await page.route('**/api/channels/*/e2e', (route) =>
        route.fulfill({ status: 200, contentType: 'application/json', body: '{"e2e_enabled":true}' }),
      );
      await page.route('**/api/channels/*/key-wraps', (route) =>
        route.fulfill({
          status: 500,
          contentType: 'application/json',
          body: '{"error":"internal server error"}',
        }),
      );
    },
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await expect(page.locator('.channel-encryption-bar.key-failed')).toBeVisible({
        timeout: 15_000,
      });
    },
    // Diffed again, after two runs of identical code disagreed here.
    //
    // The difference was one constant-width rectangle spanning columns
    // 368-674 that had moved horizontally — no text moved, and the crop was
    // 708x123 in both. The bar contains no element in the mask set, so that
    // box could only be a mask overlay from something else on the page,
    // painted at its own page coordinates into a crop it has no business
    // being in. The surface was made `reportOnly` on that evidence, which
    // was the right read of a harness defect and the wrong remedy: it gave
    // up the pixel diff instead of fixing the masks.
    //
    // `maskLocators` now scopes masks to the clipped element, so a clipped
    // capture is painted only with masks that belong inside the picture.
    // The overlay that moved is no longer drawn at all.
    clip: '.channel-encryption-bar',
    waive: {
      network:
        'the 500 is the stub this surface installs on purpose — it is the condition under test, ' +
        'not an incidental failed request.',
      console: 'the browser logs the injected 500 to the console as well',
    },
  },

  // ─────────────────────── 07 · voice ───────────────────────
  {
    id: 'voice-panel-disconnected',
    stage: '07-voice',
    title: 'Voice panel on a voice channel (not yet joined)',
    role: 'founder',
    intent: 'Pre-call state: the join affordance and any capability/permission notices.',
    navigate: async (page) => {
      await selectChannel(page, SEED.channels.voice);
      await expect(page.locator('.voice-panel, .voice-permissions-notice')).toBeVisible({
        timeout: 15_000,
      });
    },
  },

  {
    id: 'voice-panel-hybrid-channel',
    stage: '07-voice',
    title: 'Voice controls on a hybrid (text + voice) channel',
    role: 'founder',
    intent: 'Hybrid channels show the call affordance alongside the message list, unlike pure text.',
    navigate: async (page) => {
      await selectChannel(page, SEED.channels.hybrid);
      await expect(page.locator('.voice-panel, .voice-permissions-notice')).toBeVisible({
        timeout: 15_000,
      });
    },
  },
  // ── In-call ──
  //
  // Everything above is a pre-call state. These need a real WebRTC session:
  // `getUserMedia` has to resolve, the SFU has to answer, and the peer
  // connection has to reach `connected` before the grid, the controls or the
  // diagnostics render at all. The audit runs Chromium with fake media devices
  // and the e2e server has the in-process SFU configured, so the whole in-call
  // journey is reachable — see `playwright.config.ts` and `e2e-server.sh`.
  {
    id: 'call-in-progress',
    stage: '07-voice',
    title: 'In a call — participant grid and controls',
    role: 'founder',
    intent:
      'What a user actually looks at for the length of a call: their own tile, the mic/camera/' +
      'screen-share controls, and the connection state. Alone in the channel there must be no ' +
      'remote tile — counting them from inbound track count showed a phantom "Participant", ' +
      'because the SFU attaches three outbound tracks to every connection at join.',
    navigate: async (page) => {
      await selectChannel(page, SEED.channels.voice);
      await page.locator('.voice-join-btn').first().click();
      await expect(page.locator('.media-controls')).toBeVisible({ timeout: 30_000 });
      await expect(page.locator('.call-grid')).toBeVisible({ timeout: 30_000 });
    },
    clip: '.voice-panel',
  },
  {
    id: 'call-camera-on',
    stage: '07-voice',
    title: 'In a call with the camera on',
    role: 'founder',
    intent:
      'The video path, end to end: a real captured track rendered into a tile. The tile content ' +
      'is live video, so it is masked — the point of the capture is the surrounding chrome and ' +
      'the control states, which are what change when video is enabled.',
    navigate: async (page) => {
      await selectChannel(page, SEED.channels.voice);
      await page.locator('.voice-join-btn').first().click();
      await expect(page.locator('.media-controls')).toBeVisible({ timeout: 30_000 });
      await page.getByTitle('Turn on camera').click();
      await expect(page.locator('.tile-video')).toBeVisible({ timeout: 30_000 });
    },
    clip: '.voice-panel',
    mask: ['.tile-video'],
  },
  {
    id: 'call-camera-device-missing',
    stage: '07-voice',
    title: 'In a call whose saved camera is gone',
    role: 'founder',
    intent:
      'A camera chosen in settings and later unplugged. `useLocalMedia` asks for it by device ' +
      'id, the browser answers OverconstrainedError, and the recovery prompt is the only thing ' +
      'standing between the user and a camera button that silently does nothing.',
    setup: async (page) => {
      // The saved device id has to be present before the app boots, and has to
      // name a device Chromium's fake-device set does not contain.
      await page.addInitScript(() => {
        localStorage.setItem(
          'annex:audioSettings',
          JSON.stringify({ cameraDeviceId: 'camera-that-was-unplugged' }),
        );
      });
    },
    navigate: async (page) => {
      await selectChannel(page, SEED.channels.voice);
      await page.locator('.voice-join-btn').first().click();
      await expect(page.locator('.media-controls')).toBeVisible({ timeout: 30_000 });
      await page.getByTitle('Turn on camera').click();
      await expect(page.locator('.stale-camera-recovery')).toBeVisible({ timeout: 30_000 });
      // The recovery block is only useful if it offers the way out, so name
      // the actions rather than leaving them as an unnamed gap.
      await expect(page.locator('.stale-camera-actions')).toBeVisible();
    },
    clip: '.voice-panel',
  },
  {
    id: 'call-mic-device-missing',
    stage: '07-voice',
    title: 'In a call whose saved microphone is gone',
    role: 'founder',
    intent:
      'The microphone half of the unplugged-device problem. The camera offers a button to ' +
      'choose the fallback; the microphone cannot — leaving someone muted is worse than ' +
      'using a different input — so it falls back on its own and has to say so, or the user ' +
      'is talking into a device they never picked with nothing on screen admitting it.',
    setup: async (page) => {
      // Same shape as the camera surface: the saved id has to be in place
      // before boot, and has to name a device Chromium's fake set lacks.
      await page.addInitScript(() => {
        localStorage.setItem(
          'annex:audioSettings',
          JSON.stringify({ inputDeviceId: 'microphone-that-was-unplugged' }),
        );
      });
    },
    navigate: async (page) => {
      await selectChannel(page, SEED.channels.voice);
      await page.locator('.voice-join-btn').first().click();
      await expect(page.locator('.media-controls')).toBeVisible({ timeout: 30_000 });
      // The fallback lives in the enable branch of `toggleMic`, so the mic has
      // to be off before the toggle that exercises it. Scoped to the in-call
      // controls — the status bar carries a button with the same title.
      const controls = page.locator('.media-controls');
      await controls.getByTitle('Mute microphone').click();
      await expect(controls.getByTitle('Unmute microphone')).toBeVisible({ timeout: 15_000 });
      await controls.getByTitle('Unmute microphone').click();
      await expect(page.locator('.media-error')).toBeVisible({ timeout: 30_000 });
    },
    clip: '.voice-panel',
  },
  {
    id: 'call-mic-muted',
    stage: '07-voice',
    title: 'In a call with the microphone muted',
    role: 'founder',
    intent:
      'Mute is the control users reach for most and the one whose state has to be unambiguous ' +
      'at a glance — a call where you cannot tell whether you are muted is the classic failure.',
    navigate: async (page) => {
      await selectChannel(page, SEED.channels.voice);
      await page.locator('.voice-join-btn').first().click();
      await expect(page.locator('.media-controls')).toBeVisible({ timeout: 30_000 });
      // Scoped to the in-call controls: the status bar carries its own mute
      // button with the same title, so an unscoped locator matches both.
      const controls = page.locator('.media-controls');
      await controls.getByTitle('Mute microphone').click();
      await expect(controls.getByTitle('Unmute microphone')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.voice-panel',
  },
  {
    id: 'call-diagnostics',
    stage: '07-voice',
    title: 'In-call media diagnostics',
    role: 'founder',
    intent:
      'The status pills that tell a user whether their own microphone and camera are actually ' +
      'producing media — the first thing anyone looks at when others say they cannot be heard.',
    navigate: async (page) => {
      await selectChannel(page, SEED.channels.voice);
      await page.locator('.voice-join-btn').first().click();
      await expect(page.locator('.local-media-status')).toBeVisible({ timeout: 30_000 });
    },
    clip: '.local-media-status',
  },
  {
    id: 'voice-not-configured',
    stage: '07-voice',
    title: 'Voice channel on a server with voice unavailable',
    role: 'founder',
    intent:
      'Operators who have not provisioned WebRTC need a clear explanation here, not a dead button.',
    // The real shape of `GET /api/voice/config-status` (routes/mod.rs). The
    // stub used to return `{configured, url, has_api_secret, token_ttl_seconds}`
    // — fields the server has never sent — which was harmless only for as long
    // as the client ignored this endpoint until a join had already failed.
    setup: stub('**/api/voice/config-status', {
      voice_enabled: false,
      policy_enabled: true,
      infrastructure_ready: false,
      has_public_url: false,
      has_local_url: false,
      stt_ready: false,
      setup_hint:
        'Voice is enabled by policy but WebRTC is not configured. Set webrtc.url, ' +
        'webrtc.api_key, and webrtc.api_secret in config.toml or use ANNEX_WEBRTC_* ' +
        'environment variables.',
    }),
    navigate: async (page) => {
      await selectChannel(page, SEED.channels.voice);
      await expect(page.locator('.voice-panel, .voice-permissions-notice')).toBeVisible({
        timeout: 15_000,
      });
    },
  },
  {
    id: 'voice-join-failure',
    stage: '07-voice',
    title: 'Voice join rejected by the server',
    role: 'founder',
    intent:
      'The voice-join handler returns a JSON-shaped body with a text/plain content type — one of ' +
      'the error dialects the client had to be taught to read. This proves the message reaches the user.',
    setup: stub(
      '**/api/channels/*/voice/join',
      { error: 'voice_disabled', message: 'Voice is disabled on this server.' },
      403,
    ),
    // This used to click the join button only `if (await join.isVisible())`
    // and then sleep 1500ms with no assertion at all — the one surface in the
    // manifest that asserted nothing. Both halves are the harness making the
    // mistake it exists to catch: if the button ever stopped rendering, the
    // click was skipped silently, and the sleep would then photograph an
    // ordinary disconnected panel under the name "Voice join rejected by the
    // server". `--update-baselines` would write that down as correct.
    //
    // The message text is asserted, not just the presence of `.voice-error`,
    // because the panel renders that same element for four other reasons —
    // a join error, a dropped call, an unavailable server, a denied identity
    // — and any of them would satisfy a bare visibility check.
    navigate: async (page) => {
      await selectChannel(page, SEED.channels.voice);
      await page.locator('.voice-join-btn').click();
      await expect(page.locator('.voice-error')).toContainText(
        'Voice is disabled on this server.',
        { timeout: 15_000 },
      );
    },
    waive: {
      network: 'the 403 is injected deliberately to reach the join-failure state',
      console: 'the browser logs the injected 403 to the console as well',
    },
  },

  // ─────────────────────── 08 · user settings ───────────────────────
  {
    id: 'audio-settings-dialog',
    stage: '08-user-settings',
    title: 'Audio & video settings',
    role: 'founder',
    intent: 'Input/output device pickers, volume sliders, camera selection.',
    navigate: async (page) => {
      await openStatusBarDialog(page, 'audio');
    },
    clip: '.dialog',
  },
  {
    id: 'identity-settings-dialog',
    stage: '08-user-settings',
    title: 'Identity settings (persona, username, visibility)',
    role: 'founder',
    intent: 'The user’s own profile surface: persona CRUD, server username, visibility grants.',
    navigate: async (page) => {
      await openStatusBarDialog(page, 'Identity');
    },
    clip: '.dialog',
  },
  {
    id: 'identity-settings-grants-error',
    stage: '08-user-settings',
    title: 'Username visibility when the grant list fails',
    role: 'founder',
    intent:
      'A failed grant list used to leave every member row reading "Hidden" with a "Grant" button ' +
      '— a privacy assurance that nobody can see your username, produced by a dropped request. ' +
      'Because no row was ever marked granted, "Revoke" never appeared either, so a user who ' +
      'opened this dialog to take someone\u2019s access away was told there was nothing to take. ' +
      'The roster is now withheld rather than shown wrong.',
    setup: stub('**/api/profile/username/grants', { error: 'internal server error' }, 500),
    navigate: async (page) => {
      await openStatusBarDialog(page, 'Identity');
      await expect(page.locator('.member-list-error')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.dialog',
    waive: {
      network:
        'the 500 is the stub this surface installs on purpose — it is the condition under test, ' +
        'not an incidental failed request.',
      console: 'the browser logs the injected 500 to the console as well',
    },
  },
  {
    id: 'identity-settings-members-error',
    stage: '08-user-settings',
    title: 'Username visibility when the member list fails',
    role: 'founder',
    intent:
      'The other half. A failed member list used to render as "No other members on this server ' +
      'yet." — a statement about the server, made because a request did not arrive. The two ' +
      'lists fail independently and now report independently.',
    setup: stub('**/api/admin/members', { error: 'internal server error' }, 500),
    navigate: async (page) => {
      await openStatusBarDialog(page, 'Identity');
      await expect(page.locator('.member-list-error')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.dialog',
    waive: {
      network:
        'the 500 is the stub this surface installs on purpose — it is the condition under test, ' +
        'not an incidental failed request.',
      console: 'the browser logs the injected 500 to the console as well',
    },
  },
  {
    id: 'device-link-dialog',
    stage: '08-user-settings',
    title: 'Device link dialog (from status bar)',
    role: 'founder',
    intent: 'Same dialog as onboarding, reached mid-session.',
    navigate: async (page) => {
      await openStatusBarDialog(page, 'Link');
    },
    clip: '.dialog',
  },
  {
    id: 'social-recovery-dialog',
    stage: '08-user-settings',
    title: 'Social recovery (mode chooser)',
    role: 'founder',
    intent: 'Shamir key-splitting entry point — set up shards or recover from them.',
    navigate: async (page) => {
      await openStatusBarDialog(page, 'Recovery');
    },
    clip: '.dialog',
  },

  {
    id: 'social-recovery-shards-generated',
    stage: '08-user-settings',
    title: 'Recovery shards, generated',
    role: 'founder',
    intent:
      'The success state of the only flow that can get a lost identity back, and the one place ' +
      'the shards themselves are ever shown. `success-message` is used by five dialogs and had ' +
      'never been photographed in any of them — the confirmations were the half of the app the ' +
      'audit had no picture of.',
    navigate: async (page) => {
      await openStatusBarDialog(page, 'Recovery');
      await page.getByRole('button', { name: /Set Up Recovery/ }).click();
      const guardians = page.locator('.guardian-entry input:not(.guardian-pseudo)');
      const count = await guardians.count();
      for (let i = 0; i < count; i += 1) {
        await guardians.nth(i).fill(`Guardian ${i + 1}`);
      }
      await page.getByRole('button', { name: 'Generate Shards' }).click();
      await expect(page.locator('.recovery-complete .success-message')).toBeVisible({
        timeout: 20_000,
      });
    },
    clip: '.dialog',
    // The shards are a Shamir split of this run's secret key, so their bytes
    // differ every time. The label and the copy control beside them are the
    // part worth diffing.
    mask: ['.shard-data'],
  },

  {
    id: 'identity-settings-persona-form',
    stage: '08-user-settings',
    title: 'Persona editor (name, bio, colour picker)',
    role: 'founder',
    intent: 'The 12-swatch accent picker that decides how this identity is coloured everywhere.',
    navigate: async (page) => {
      await openStatusBarDialog(page, 'Identity');
      await page.getByRole('button', { name: 'New Persona' }).click();
      await expect(page.locator('.color-picker')).toBeVisible();
    },
    clip: '.dialog',
  },
  {
    id: 'social-recovery-setup',
    stage: '08-user-settings',
    title: 'Social recovery — shard setup',
    role: 'founder',
    intent: 'Choosing trustees and threshold for Shamir key splitting.',
    navigate: async (page) => {
      await openStatusBarDialog(page, 'Recovery');
      await page.locator('.device-link-option').first().click();
      await expect(page.locator('.dialog')).toBeVisible();
    },
    clip: '.dialog',
    mask: ['.shard-list', '.shard-value'],
  },
  {
    id: 'social-recovery-recover',
    stage: '08-user-settings',
    title: 'Social recovery — reconstruct from shards',
    role: 'founder',
    intent: 'The paste-shards path a user takes when they have lost their device.',
    navigate: async (page) => {
      await openStatusBarDialog(page, 'Recovery');
      await page.locator('.device-link-option').nth(1).click();
      await expect(page.locator('.dialog')).toBeVisible();
    },
    clip: '.dialog',
  },
  {
    id: 'device-link-receive',
    stage: '08-user-settings',
    title: 'Link another device — receive',
    role: 'founder',
    intent: 'The paste-payload half of device linking, reached mid-session.',
    navigate: async (page) => {
      await openStatusBarDialog(page, 'Link');
      await page.locator('.device-link-option').nth(1).click();
      await expect(page.locator('.device-link-receive')).toBeVisible();
    },
    clip: '.dialog',
  },
  {
    id: 'add-server-dialog',
    stage: '08-user-settings',
    title: 'Add another server',
    role: 'founder',
    intent: 'Multi-server entry point from the server hub rail.',
    navigate: async (page) => {
      await page.locator('.add-server-btn').click();
      await expect(page.locator('.add-server-dialog')).toBeVisible();
    },
    clip: '.dialog',
  },

  // ─────────────────────── 09 · admin ───────────────────────
  {
    id: 'admin-dropdown',
    stage: '09-admin',
    title: 'Admin menu (gear dropdown)',
    role: 'founder',
    intent: 'The only route to every admin surface; renders only when can_moderate.',
    navigate: async (page) => {
      await openAdminDropdown(page);
    },
  },
  {
    id: 'admin-server-settings',
    stage: '09-admin',
    title: 'Admin — server settings',
    role: 'founder',
    intent: 'Server image, name, slug, public URL, and invite-link generation.',
    navigate: async (page) => {
      await openAdminSection(page, 'Server Settings');
    },
  },
  {
    id: 'admin-storage-gate-degraded',
    stage: '09-admin',
    title: 'Admin — storage gate holding writes',
    role: 'founder',
    intent:
      'The other half of `storage-gate-507`. That surface photographs what a user sees when ' +
      'the gate is closed; this is the screen the operator opens next. Both the read and the ' +
      'clear existed on the server with no caller in the client, so the panel used to say ' +
      'nothing about a server that had stopped accepting writes — and the gate has no ' +
      'automatic recovery, so the only way out was a process restart.',
    setup: stub('**/api/admin/storage', {
      state: 'degraded',
      reason: 'free space 41 MB is below the configured block threshold of 128 MB',
      writes_blocked: true,
    }),
    navigate: async (page) => {
      await openAdminSection(page, 'Server Settings');
      await expect(page.locator('.storage-gate-state')).toContainText('degraded', {
        timeout: 15_000,
      });
    },
    clip: '.admin-panel',
  },
  {
    id: 'admin-storage-gate-unreadable',
    stage: '09-admin',
    title: 'Admin — storage state the server would not report',
    role: 'founder',
    intent:
      'A dropped read must not render as a healthy server. This panel exists to say when the ' +
      'light is red, so a green one produced by a failed request is worse than no panel: the ' +
      'operator stops looking.',
    setup: stub('**/api/admin/storage', { error: 'internal server error' }, 500),
    navigate: async (page) => {
      await openAdminSection(page, 'Server Settings');
      await expect(page.locator('.warning-hint[role="alert"]')).toContainText(
        'Could not read storage health',
        { timeout: 15_000 },
      );
    },
    waive: {
      network: 'the 500 is the stub this surface installs — it is the condition under test',
      console: 'the browser logs the injected 500 to the console as well',
    },
    clip: '.admin-panel',
  },
  {
    id: 'admin-server-policy',
    stage: '09-admin',
    title: 'Admin — server policy',
    role: 'founder',
    intent: 'The densest form in the app: access mode, limits, uploads, rate limits, VRP axioms.',
    navigate: async (page) => {
      await openAdminSection(page, 'Server Policy');
    },
  },
  {
    id: 'admin-member-management',
    stage: '09-admin',
    title: 'Admin — member management',
    role: 'founder',
    intent: 'Per-member capability checkboxes — the only UI for granting moderator.',
    navigate: async (page) => {
      await openAdminSection(page, 'Member Management');
    },
  },
  {
    id: 'admin-channel-management',
    stage: '09-admin',
    title: 'Admin — channel management',
    role: 'founder',
    intent: 'Channel list with per-channel delete affordances, as a moderator sees it.',
    navigate: async (page) => {
      await openAdminSection(page, 'Channel Management');
    },
  },

  {
    id: 'admin-policy-password-mode',
    stage: '09-admin',
    title: 'Server policy with password access mode selected',
    role: 'founder',
    intent:
      'Choosing "password" reveals a password field. This is the mode behind the join flow whose ' +
      'submit button did nothing and which re-fired registration on every keystroke.',
    navigate: async (page) => {
      await openAdminSection(page, 'Server Policy');
      await page.locator('select').first().selectOption('password');
      await expect(page.locator('.view-content')).toBeVisible();
    },
  },
  {
    id: 'admin-channel-management-list',
    stage: '09-admin',
    title: 'Channel management with every channel type',
    role: 'founder',
    intent: 'Delete affordances for all five channel types.',
    navigate: async (page) => {
      await openAdminSection(page, 'Channel Management');
      await expect(page.locator('.channel-manager-item').first()).toBeVisible({ timeout: 15_000 });
    },
    clip: '.channel-manager',
  },
  {
    id: 'admin-channel-delete-confirm',
    stage: '09-admin',
    title: 'Confirming a channel deletion',
    role: 'founder',
    intent:
      'Deleting a channel is irreversible and takes its messages with it, so the confirmation ' +
      'has to name the channel. This was the browser\'s own confirm() until now — modal at the ' +
      'OS level, unstyled, and invisible to this audit because a native dialog is outside the page.',
    navigate: async (page) => {
      await openAdminSection(page, 'Channel Management');
      const row = page.locator('.channel-manager-item').first();
      await expect(row).toBeVisible({ timeout: 15_000 });
      await row.getByRole('button', { name: 'Delete' }).click();
      await expect(page.getByRole('dialog')).toBeVisible();
    },
    clip: '.dialog',
  },
  {
    id: 'non-admin-chat',
    stage: '09-admin',
    title: 'Chat as a non-moderator',
    role: 'member',
    intent:
      'The negative case for every admin affordance: no gear, no create-channel button, no ' +
      'encryption CTA. Proves capability gating actually hides things rather than only disabling them.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await expect(page.locator('.admin-menu-btn')).toHaveCount(0);
    },
  },

  // ─────────────────────── 10 · federation ───────────────────────
  {
    id: 'federation-panel-empty',
    stage: '10-federation',
    title: 'Federation panel with no peers',
    role: 'founder',
    intent:
      'The default state on a standalone server. Must be distinguishable from a failed fetch — today it is not.',
    navigate: async (page) => {
      await openTab(page, 'Federation');
    },
  },
  {
    id: 'federation-panel-peers',
    stage: '10-federation',
    title: 'Federation panel with peers',
    role: 'founder',
    intent: 'Peer rows with alignment and transfer-scope badges. Stubbed — no second server in this lane.',
    setup: stub('**/api/public/federation/peers*', {
      peers: [
        {
          instance_id: 'peer-alpha',
          label: 'Alpha Station',
          url: 'https://alpha.example',
          alignment_status: 'Aligned',
          transfer_scope: 'FullKnowledgeBundle',
          joined: false,
        },
        {
          instance_id: 'peer-beta',
          label: 'Beta Relay',
          url: 'https://beta.example',
          alignment_status: 'Partial',
          transfer_scope: 'ReflectionSummariesOnly',
          joined: true,
        },
      ],
    }),
    navigate: async (page) => {
      await openTab(page, 'Federation');
      await expect(page.locator('.federation-panel')).toBeVisible();
    },
  },
  {
    id: 'federation-panel-error',
    stage: '10-federation',
    title: 'Federation panel when the peer fetch fails',
    role: 'founder',
    intent:
      'Proves the empty/error conflation: a 500 here renders exactly like “no peers”. This is a defect the capture documents.',
    setup: stub('**/api/public/federation/peers*', { error: 'internal server error' }, 500),
    navigate: async (page) => {
      await openTab(page, 'Federation');
      await expect(page.locator('.federation-panel')).toBeVisible();
    },
    waive: {
      network: 'the 500 is injected deliberately to reach the error state',
      console: 'the browser logs the injected 500 to the console as well',
    },
  },

  // ─────────────────────── 11 · observability ───────────────────────
  {
    id: 'event-log',
    stage: '11-observability',
    title: 'Event log',
    role: 'founder',
    intent: 'Signed, hash-chained audit trail — identity/presence/federation/agent/moderation events.',
    navigate: async (page) => {
      await openTab(page, 'Events');
      await expect(page.locator('.event-log')).toBeVisible({ timeout: 15_000 });
    },
    mask: ['.event-col-time', '.event-col-entity'],
  },
  {
    id: 'event-log-empty',
    stage: '11-observability',
    title: 'Event log with no matching events',
    role: 'founder',
    intent: 'Empty state for a domain filter that matches nothing.',
    setup: stub('**/api/public/events*', []),
    navigate: async (page) => {
      await openTab(page, 'Events');
      await expect(page.locator('.event-log')).toBeVisible({ timeout: 15_000 });
    },
  },

  // ─────────────────────── 12 · agents ───────────────────────
  {
    id: 'member-list-agents',
    stage: '12-agents',
    title: 'Member list with active agents',
    role: 'founder',
    intent: 'Agent presence with VRP alignment badges. Stubbed — no live agent in this lane.',
    setup: stub('**/api/public/agents*', {
      agents: [
        {
          pseudonym_id: 'agent-aurora',
          display_name: 'Aurora',
          alignment_status: 'Aligned',
          transfer_scope: 'FullKnowledgeBundle',
          reputation_score: 0.92,
          capabilities: ['summarise', 'translate'],
          active: true,
        },
      ],
    }),
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await expect(page.locator('.member-list')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.sidebar-right',
    // Hidden below 1100px by design — see agent-detail-overlay.
    viewports: ['desktop', 'laptop'],
  },
  {
    id: 'federation-peer-detail',
    stage: '10-federation',
    title: 'Peer detail (explore a federated server)',
    role: 'founder',
    intent:
      'The "Explore" dialog. It fetches the remote server cross-origin via `requestRemote`, which ' +
      'is the path the CSP `connect-src` and the remote CORS default both constrain — so this ' +
      'surface is where a browser-only federation break would show up.',
    setup: async (page) => {
      await page.route('**/api/public/federation/peers*', (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            peers: [
              {
                instance_id: 'peer-alpha',
                label: 'Alpha Station',
                base_url: 'https://alpha.example',
                alignment_status: 'Aligned',
                transfer_scope: 'FullKnowledgeBundle',
              },
            ],
          }),
        }),
      );
      await page.route('https://alpha.example/**', (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            slug: 'alpha',
            label: 'Alpha Station',
            description: '',
            public_url: 'https://alpha.example',
            members_by_type: { HUMAN: 42 },
            total_active_members: 42,
            channel_count: 9,
            federation_peer_count: 3,
            active_agent_count: 2,
            access_mode: 'public',
          }),
        }),
      );
    },
    navigate: async (page) => {
      await openTab(page, 'Federation');
      await page.getByRole('button', { name: /Explore|View Upstream/ }).first().click();
      await expect(page.locator('.peer-detail-dialog')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.dialog',
  },
  {
    id: 'info-tip-near-edge',
    stage: '09-admin',
    title: 'An InfoTip opened next to the edge it has to avoid',
    role: 'founder',
    intent:
      'The open state is captured elsewhere, in the middle of a wide panel where its ' +
      'viewport-overflow correction never runs. This one opens the rightmost tip in a narrow ' +
      'dialog, which is the case that correction exists for — and the only way to see whether ' +
      'it lands somewhere readable.',
    setup: async (page) => {
      await page.route('**/api/public/federation/peers*', (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            peers: [
              {
                instance_id: 'peer-alpha',
                label: 'Alpha Station',
                base_url: 'https://alpha.example',
                alignment_status: 'Aligned',
                transfer_scope: 'FullKnowledgeBundle',
              },
            ],
          }),
        }),
      );
      await page.route('https://alpha.example/**', (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            slug: 'alpha',
            label: 'Alpha Station',
            description: '',
            public_url: 'https://alpha.example',
            members_by_type: { HUMAN: 42 },
            total_active_members: 42,
            channel_count: 9,
            federation_peer_count: 3,
            active_agent_count: 2,
            access_mode: 'public',
          }),
        }),
      );
    },
    navigate: async (page) => {
      await openTab(page, 'Federation');
      await page.getByRole('button', { name: /Explore|View Upstream/ }).first().click();
      await expect(page.locator('.peer-detail-dialog')).toBeVisible({ timeout: 15_000 });
      await page.locator('.peer-detail-dialog .info-tip').last().focus();
      await expect(page.locator('.info-tip-popup')).toBeVisible({ timeout: 10_000 });
    },
    // Deliberately NOT clipped to the dialog: the whole question is whether
    // the popup stays inside the viewport, and clipping to the dialog would
    // crop away the evidence either way.
    viewports: ['mobile', 'narrow'],
  },
  {
    id: 'federation-join-failed',
    stage: '10-federation',
    title: 'Joining a peer that stops answering',
    role: 'founder',
    intent:
      'The dialog loaded the peer fine and then the join failed — a peer that went down between ' +
      'looking and joining, which is the ordinary way this fails. The message used to be the ' +
      'four words "Could not reach server." for two unrelated causes; it now carries whichever ' +
      'one happened. The Retry label matters too: the dialog must stay usable rather than ' +
      'becoming a dead end.',
    setup: async (page) => {
      await page.route('**/api/public/federation/peers*', (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            peers: [
              {
                instance_id: 'peer-alpha',
                label: 'Alpha Station',
                base_url: 'https://alpha.example',
                alignment_status: 'Aligned',
                transfer_scope: 'FullKnowledgeBundle',
              },
            ],
          }),
        }),
      );
      // The dialog's own summary fetch and the join's both call
      // `getRemoteServerSummary` on the same URL, so they cannot be told
      // apart by pattern. Answer the first (the dialog opens normally) and
      // refuse the rest (the join fails) — which is also exactly the
      // sequence a peer going down between the two looks like.
      let answered = 0;
      await page.route('https://alpha.example/**', (route) => {
        answered += 1;
        if (answered > 1) return route.abort('connectionrefused');
        return route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            slug: 'alpha',
            label: 'Alpha Station',
            description: '',
            public_url: 'https://alpha.example',
            members_by_type: { HUMAN: 42 },
            total_active_members: 42,
            channel_count: 9,
            federation_peer_count: 3,
            active_agent_count: 2,
            access_mode: 'public',
          }),
        });
      });
    },
    navigate: async (page) => {
      await openTab(page, 'Federation');
      await page.getByRole('button', { name: /Explore|View Upstream/ }).first().click();
      await expect(page.locator('.peer-detail-dialog')).toBeVisible({ timeout: 15_000 });
      await page.getByRole('button', { name: 'Join this Server' }).click();
      await expect(page.locator('.peer-detail-dialog .error-text')).toBeVisible({
        timeout: 20_000,
      });
      // A click leaves the pointer where it clicked, and the error text
      // reflows the dialog underneath it — on the first recording that put
      // the cursor over an InfoTip and photographed a tooltip nobody asked
      // for. Park it somewhere harmless so the capture shows the state under
      // test and not an artifact of where the mouse happened to land.
      await page.mouse.move(0, 0);
      await expect(page.locator('.info-tip-popup')).toHaveCount(0);
    },
    clip: '.dialog',
    waive: {
      network: 'the refused join request is the condition under test, installed by this surface.',
      console: 'the browser logs the aborted request to the console as well',
    },
  },
  {
    id: 'federation-peer-unreachable',
    stage: '10-federation',
    title: 'A peer that will not answer',
    role: 'founder',
    intent:
      'The failure branch of the dialog above. It rendered "Could not reach server at …" and ' +
      'stopped there — no reason, and no retry — and because the Join button is gated on the ' +
      'summary having loaded, a single dropped cross-origin request turned the whole dialog into ' +
      'a dead end that only closing and reopening could clear. The enclosing panel had already ' +
      'been given a reason and a retry; the dialog nested inside it had not.',
    setup: async (page) => {
      await page.route('**/api/public/federation/peers*', (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            peers: [
              {
                instance_id: 'peer-alpha',
                label: 'Alpha Station',
                base_url: 'https://alpha.example',
                alignment_status: 'Aligned',
                transfer_scope: 'FullKnowledgeBundle',
              },
            ],
          }),
        }),
      );
      // The peer itself refuses, which is what a dead host or a CORS
      // rejection looks like from the browser.
      await page.route('https://alpha.example/**', (route) => route.abort('failed'));
    },
    navigate: async (page) => {
      await openTab(page, 'Federation');
      await page.getByRole('button', { name: /Explore|View Upstream/ }).first().click();
      await expect(page.locator('.peer-detail-dialog')).toBeVisible({ timeout: 15_000 });
      await expect(page.locator('.peer-detail-error')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.dialog',
    waive: {
      network:
        'the aborted request is the stub this surface installs on purpose — it is the condition ' +
        'under test, not an incidental failure.',
      console: 'the browser logs the aborted cross-origin request to the console as well',
    },
  },
  {
    id: 'event-log-filtered',
    stage: '11-observability',
    title: 'Event log filtered to one domain',
    role: 'founder',
    intent: 'Domain filter applied — proves the select actually narrows the table.',
    navigate: async (page) => {
      await openTab(page, 'Events');
      await page.locator('.domain-filter').selectOption('IDENTITY');
      await expect(page.locator('.event-log')).toBeVisible({ timeout: 15_000 });
    },
    mask: ['.event-col-time', '.event-col-entity'],
  },
  {
    id: 'event-log-error',
    stage: '11-observability',
    title: 'Event log when the events endpoint fails',
    role: 'founder',
    intent:
      'This used to render as "No events found" on an auto-refreshing audit log — a dead backend ' +
      'was indistinguishable from a quiet server. It must now read as a failure with a retry.',
    setup: stub('**/api/public/events*', { error: 'internal server error' }, 500),
    navigate: async (page) => {
      await openTab(page, 'Events');
      await expect(page.locator('.event-log-error')).toBeVisible({ timeout: 15_000 });
    },
    waive: {
      network: 'the 500 is injected deliberately to reach the error state',
      console: 'the browser logs the injected 500 to the console as well',
    },
  },
  {
    id: 'agent-detail-overlay',
    stage: '12-agents',
    title: 'Agent detail (alignment, transfer scope, reputation)',
    role: 'founder',
    intent: 'The only surface exposing an agent’s VRP standing to a human.',
    setup: stub('**/api/public/agents*', {
      agents: [
        {
          pseudonym_id: 'agent-aurora',
          display_name: 'Aurora',
          alignment_status: 'Aligned',
          transfer_scope: 'FullKnowledgeBundle',
          reputation_score: 0.92,
          capabilities: ['summarise', 'translate'],
          active: true,
        },
      ],
    }),
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await page.locator('.agent-item').first().click();
      await expect(page.locator('.agent-detail')).toBeVisible({ timeout: 15_000 });
    },
    // The member rail is deliberately hidden below 1100px — a phone has no
    // room for it and the channel list matters more. There is nothing to
    // click there, so capturing it would assert a layout we chose against.
    viewports: ['desktop', 'laptop'],
  },
  // ── Link previews ──
  //
  // A pasted URL renders a card built from server-proxied metadata (the proxy
  // exists so the client never fetches a third-party origin directly). Three
  // render branches, all of which a user sees.
  {
    id: 'link-preview-card',
    stage: '06-messaging',
    title: 'Message with a link preview card',
    role: 'founder',
    intent:
      'The rich card a pasted link becomes. Stubbed because the real endpoint reaches a third ' +
      'party, which no audit should depend on — and the image is proxied through the server, so ' +
      'the card renders entirely from this payload.',
    setup: stub('**/api/link-preview?*', {
      url: 'https://example.com/an-article',
      title: 'An article about distributed systems',
      description:
        'Why consensus is hard, why everybody gets clock skew wrong, and what to do about both.',
      siteName: 'example.com',
      imageUrl: null,
    }),
    navigate: async (page) => {
      // A dedicated channel, not the shared default: these messages persist for
      // the rest of the run, and a later capture of the same channel would
      // re-fetch their previews un-stubbed and record the failures as its own.
      await selectChannel(page, SEED.channels.text);
      await postFreshMessage(page, 'Worth a read: https://example.com/an-article');
      // `.first()`: surfaces share the seeded channel and each posts a message,
      // so by the time a later one runs there are several previews on screen.
      await expect(page.locator('.link-preview-card').first()).toBeVisible({ timeout: 20_000 });
    },
    clip: '.chat-area',
  },
  {
    id: 'link-preview-unavailable',
    stage: '06-messaging',
    title: 'Link preview when the proxy cannot fetch metadata',
    role: 'founder',
    intent:
      'A link whose metadata could not be fetched must still be a usable link — the fallback is ' +
      'the domain and the URL, not a broken card and not a silently missing one.',
    setup: stub('**/api/link-preview?*', { error: 'upstream unreachable' }, 502),
    navigate: async (page) => {
      await selectChannel(page, SEED.channels.text);
      await postFreshMessage(page, 'Also worth a read: https://example.org/another-article');
      await expect(page.locator('.link-preview-minimal').first()).toBeVisible({ timeout: 20_000 });
    },
    clip: '.chat-area',
    waive: {
      network: 'the 502 is injected deliberately to reach the fallback state',
      console: 'the browser logs the injected 502 to the console as well',
    },
  },

  // ── Inline help ──
  {
    id: 'info-tip-open',
    stage: '09-admin',
    title: 'An InfoTip popup, open',
    role: 'founder',
    intent:
      'InfoTip is the only inline help in the app and appears beside a dozen admin controls, but ' +
      'its OPEN state had never been captured — only the icon. It opens on focus as well as ' +
      'hover, which is what makes it reachable without a pointer.',
    navigate: async (page) => {
      await openAdminSection(page, 'Server Policy');
      // Focus rather than hover: it is the keyboard path, and a hover that the
      // screenshot does not preserve would capture nothing.
      await page.locator('.info-tip').first().focus();
      await expect(page.locator('.info-tip-popup')).toBeVisible({ timeout: 10_000 });
    },
    clip: '.view-content',
  },

  // ── Render crash containment ──
  {
    id: 'error-boundary',
    stage: '13-cross-cutting',
    title: 'A view that crashed during render',
    role: 'founder',
    intent:
      'What a user sees when a component throws. The boundaries exist so one bad record takes ' +
      'out one column rather than the whole app, and this is the only surface that proves the ' +
      'containment actually works. Triggered with an agent record missing `capabilities`, which ' +
      'MemberList maps over unguarded.',
    setup: stub('**/api/public/agents*', {
      agents: [
        {
          pseudonym_id: 'agent-malformed',
          display_name: 'Malformed',
          alignment_status: 'Aligned',
          transfer_scope: 'FullKnowledgeBundle',
          reputation_score: 0.5,
          // `capabilities` deliberately absent — MemberList reads `.length` on it.
          active: true,
        },
      ],
    }),
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      // `capabilities` is read in the agent DETAIL overlay, not the list, so
      // the list renders fine and the throw happens on open.
      await page.locator('.agent-item').first().click();
      await expect(page.locator('.error-boundary')).toBeVisible({ timeout: 20_000 });
      // Name the three parts, not just the container. The coverage contract
      // matches on class names, and these were sitting in KNOWN_UNCOVERED as
      // "photographed, just not named" — which is a gap in the bookkeeping,
      // not in the coverage. Asserting them also pins that a contained crash
      // still tells the user what happened, where, and how to see the detail.
      await expect(page.locator('.error-boundary-title')).toBeVisible();
      await expect(page.locator('.error-boundary-hint')).toBeVisible();
      await expect(page.locator('.error-details')).toBeVisible();
    },
    // The member rail — and so the agent list — is hidden below 1100px.
    viewports: ['desktop', 'laptop'],
    waive: {
      console: 'React logs the caught render error, which is the point of the surface',
      a11y: 'the crashed subtree is what is being captured; auditing it audits the crash',
    },
  },

  {
    id: 'reconnection-banner-disconnected',
    stage: '13-cross-cutting',
    title: 'Connection-lost banner',
    role: 'founder',
    intent:
      'A genuine drop, as opposed to the phantom one every page load used to show. Driven by ' +
      'taking the browser offline rather than by poking state, so it exercises the real path.',
    navigate: async (page) => {
      await selectChannel(page, SEED.defaultChannel);
      await page.context().setOffline(true);
      // How fast an already-established WebSocket notices the network going
      // away is up to the browser, so this is given real time and captured
      // whatever state it reaches rather than failing the sweep on timing.
      await page
        .locator('.reconnection-banner')
        .waitFor({ state: 'visible', timeout: 25_000 })
        .catch(() => {});
    },
    reportOnly: true,
    waive: {
      network: 'the context is deliberately taken offline, so every in-flight request fails',
      console: 'every dropped request is also logged to the console',
      a11y: 'axe cannot fetch its own resources while the context is offline',
    },
  },
  {
    id: 'rate-limited',
    stage: '13-cross-cutting',
    title: 'Rate limited (HTTP 429)',
    role: 'founder',
    intent:
      'Every protected route can return 429 with a Retry-After. The client turns it into a ' +
      'sentence with the wait time; this proves the user is told how long to wait.',
    setup: async (page) => {
      await page.route('**/api/channels', (route) =>
        route.fulfill({
          status: 429,
          headers: { 'Retry-After': '60' },
          contentType: 'application/json',
          body: JSON.stringify({ error: 'rate limit exceeded' }),
        }),
      );
    },
    navigate: async (page) => {
      await expect(page.locator('.channel-action-error, .channel-list')).toBeVisible({
        timeout: 20_000,
      });
    },
    waive: {
      network: 'the 429 is injected deliberately to reach the rate-limited state',
      console: 'the browser logs the injected 429 to the console as well',
    },
  },
  {
    id: 'storage-gate-507',
    stage: '13-cross-cutting',
    title: 'Server out of storage (HTTP 507)',
    role: 'founder',
    intent:
      'The storage gate blocks every mutating request once free space runs out. Users must be ' +
      'told the server is full rather than seeing writes fail for no stated reason.',
    setup: async (page) => {
      await page.route('**/api/channels', (route) =>
        route.request().method() === 'POST'
          ? route.fulfill({
              status: 507,
              contentType: 'application/json',
              body: JSON.stringify({ error: 'server is out of storage' }),
            })
          : route.continue(),
      );
    },
    navigate: async (page) => {
      await page.locator('.create-channel-btn').click();
      const dialog = page.locator('.dialog');
      await dialog.getByPlaceholder('general').fill('storage-gate-probe');
      await dialog.getByRole('button', { name: 'Create' }).click();
      await expect(dialog.locator('.error-message')).toBeVisible({ timeout: 15_000 });
    },
    clip: '.dialog',
    waive: {
      network: 'the 507 is injected deliberately to reach the storage-gate state',
      console: 'the browser logs the injected 507 to the console as well',
    },
  },
];

/** Fast lookup used by the runner and by `manifest.spec.ts`. */
export const SURFACE_IDS = new Set(SURFACES.map((s) => s.id));
