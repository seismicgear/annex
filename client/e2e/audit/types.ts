/**
 * Type contract for the UI audit harness.
 *
 * The harness is manifest-driven: `surfaces.ts` declares every screen, dialog,
 * overlay, popover and notice a user can reach, and the runner
 * (`capture.spec.ts`) walks that list capturing a screenshot and running a
 * fixed battery of automated audits against each one.
 *
 * The manifest IS the coverage contract. A surface that is not listed is a
 * surface nobody screenshots and nobody audits, so `manifest.spec.ts` asserts
 * the list stays in sync with the components that actually exist.
 */

import type { Page } from '@playwright/test';

/**
 * Stages of the real user journey, in the order a person encounters them.
 * The numeric prefix is load-bearing: it sorts stages correctly in the
 * filesystem, in the findings ledger, and in the generated contact sheet.
 */
export type JourneyStage =
  | '01-install'
  | '02-identity'
  | '03-server-startup'
  | '04-registration'
  | '05-channels'
  | '06-messaging'
  | '07-voice'
  | '08-user-settings'
  | '09-admin'
  | '10-federation'
  | '11-observability'
  | '12-agents'
  | '13-cross-cutting';

export const JOURNEY_STAGES: JourneyStage[] = [
  '01-install',
  '02-identity',
  '03-server-startup',
  '04-registration',
  '05-channels',
  '06-messaging',
  '07-voice',
  '08-user-settings',
  '09-admin',
  '10-federation',
  '11-observability',
  '12-agents',
  '13-cross-cutting',
];

/**
 * Which authenticated identity a surface needs.
 *
 * - `fresh`        — no stored state at all. The only way to see onboarding.
 * - `member`       — registered, no moderator capability.
 * - `founder`      — registered FIRST, so `ensure_founder` promoted it to
 *                    moderator. The only role that sees the admin gear.
 * - `second-member`— a distinct identity, for two-party surfaces (message
 *                    fan-out, multi-party calls, username grants).
 *
 * Storage state for the three registered roles is produced once per run by
 * `global-setup.ts` and reused, because a cold context pays a real in-browser
 * Groth16 proof (30-60s on a 4-core box).
 */
export type Role = 'fresh' | 'member' | 'founder' | 'second-member';

export const WARM_ROLES: Exclude<Role, 'fresh'>[] = ['founder', 'member', 'second-member'];

/** Capture viewports. `mobile` exists to *document* the responsive gap, not to assert it passes. */
export type ViewportId = 'desktop' | 'laptop' | 'narrow' | 'mobile';

export interface Viewport {
  id: ViewportId;
  width: number;
  height: number;
}

export const VIEWPORTS: Viewport[] = [
  { id: 'desktop', width: 1440, height: 900 },
  { id: 'laptop', width: 1280, height: 800 },
  { id: 'narrow', width: 1024, height: 768 },
  { id: 'mobile', width: 390, height: 844 },
];

/** The automated checks run against every captured surface. */
export type AuditId =
  /** axe-core scan (WCAG 2.1 A/AA + best practices). */
  | 'a11y'
  /** Uncaught page errors and `console.error` emitted while reaching the surface. */
  | 'console'
  /** Requests that failed or returned >= 400 while reaching the surface. */
  | 'network'
  /** Content wider than the viewport, or text clipped by its container. */
  | 'overflow'
  /** Dialogs: reachable by keyboard, trap focus, and close on Escape. */
  | 'keyboard';

export const AUDIT_IDS: AuditId[] = ['a11y', 'console', 'network', 'overflow', 'keyboard'];

export type Severity = 'p1' | 'p2' | 'p3';

export interface Finding {
  surfaceId: string;
  stage: JourneyStage;
  viewport: ViewportId;
  audit: AuditId;
  severity: Severity;
  /** Stable identifier for the failure, e.g. an axe rule id or `horizontal-overflow`. */
  rule: string;
  detail: string;
  /** CSS selector or element description, when the finding is element-scoped. */
  target?: string;
  /** Screenshot this finding was observed on, relative to the audit root. */
  screenshot?: string;
}

/**
 * A single capturable surface.
 *
 * `setup` runs before navigation (install route stubs / seed client state);
 * `navigate` drives the UI from the role's landing state to the surface.
 */
export interface Surface {
  /** Stable, unique, kebab-case. Becomes the screenshot filename. */
  id: string;
  stage: JourneyStage;
  /** Human title for the contact sheet. */
  title: string;
  role: Role;
  /**
   * One line on what this surface proves. Shown in the contact sheet so a
   * reviewer knows why the shot exists without reading the navigation code.
   */
  intent: string;

  /** Runs before `page.goto`. Use for `page.route` stubs and init scripts. */
  setup?: (page: Page) => Promise<void>;
  /** Drives the UI to the surface. Called with the app already loaded. */
  navigate: (page: Page) => Promise<void>;

  /** Screenshot only this element instead of the viewport. */
  clip?: string;
  /**
   * Extra selectors to mask. Identity-derived values (pseudonyms, leaf
   * indices, invite codes) change every run, so they are masked rather than
   * made reproducible — see `nav.ts::NONDETERMINISTIC_SELECTORS` for the
   * global set that applies everywhere.
   */
  mask?: string[];

  /**
   * Capture and audit, but do not pixel-diff. For surfaces that are
   * genuinely nondeterministic beyond what masking can fix (live WebRTC
   * video, animated media).
   */
  reportOnly?: boolean;

  /** Restrict to specific viewports. Defaults to all of `VIEWPORTS`. */
  viewports?: ViewportId[];

  /**
   * Waive an audit for this surface. The value is the REASON, never a bare
   * boolean — an unexplained waiver is indistinguishable from a bug someone
   * silenced, so the type makes the justification mandatory.
   */
  waive?: Partial<Record<AuditId, string>>;
}
