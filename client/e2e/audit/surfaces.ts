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
    // Mask only the two regenerated values, not the whole panel — masking the
    // entire panel would leave the screenshot proving nothing about it.
    mask: ['.qr-container', '.pairing-code-value'],
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
    waive: { network: 'the 403 is injected deliberately to reach the refusal state' },
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
    waive: { network: 'the 403 is injected deliberately to reach the join-failure state' },
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
    intent: 'Channel delete surface; currently the only place using native confirm().',
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
    intent:
      'Delete affordances for all five channel types. Deletion here is the last remaining native ' +
      'confirm() in the app.',
    navigate: async (page) => {
      await openAdminSection(page, 'Channel Management');
      await expect(page.locator('.channel-manager-item').first()).toBeVisible({ timeout: 15_000 });
    },
    clip: '.channel-manager',
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
    waive: { network: 'the 500 is deliberately injected to capture the error state' },
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
    waive: { network: 'the 500 is injected deliberately to reach the error state' },
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
    waive: { network: 'the 429 is injected deliberately to reach the rate-limited state' },
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
    waive: { network: 'the 507 is injected deliberately to reach the storage-gate state' },
  },
];

/** Fast lookup used by the runner and by `manifest.spec.ts`. */
export const SURFACE_IDS = new Set(SURFACES.map((s) => s.id));
