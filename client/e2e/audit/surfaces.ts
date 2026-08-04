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
    id: 'identity-setup-device-link-share',
    stage: '02-identity',
    title: 'Link from another device — share (QR + pairing code)',
    role: 'fresh',
    intent: 'Proves the QR/pairing-code surface renders; the code is masked as nondeterministic.',
    navigate: async (page) => {
      await page.locator('.device-link-setup-btn').click();
      await page.locator('.device-link-option').first().click();
      await expect(page.locator('.device-link-share, .device-link-receive')).toBeVisible();
    },
    mask: ['.device-link-share', '.device-link-receive'],
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
          reputation: 0.92,
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
  },
];

/** Fast lookup used by the runner and by `manifest.spec.ts`. */
export const SURFACE_IDS = new Set(SURFACES.map((s) => s.id));
