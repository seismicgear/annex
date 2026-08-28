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
  return bubble;
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
    mask: ['.search-result-time', '.search-result-sender'],
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
    setup: stub('**/api/voice/config-status', {
      configured: false,
      url: '',
      has_api_secret: false,
      token_ttl_seconds: 0,
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
    navigate: async (page) => {
      await selectChannel(page, SEED.channels.voice);
      const join = page.locator('.voice-join-btn');
      if (await join.isVisible().catch(() => false)) await join.click();
      await page.waitForTimeout(1500);
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
