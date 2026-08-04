/**
 * Warm authentication state for the audit harness.
 *
 * Reaching the main UI costs a real in-browser Groth16 membership proof —
 * 30-60s on a 4-core machine, and there is no client-side bypass (the server's
 * `enforce_zk_proofs` flag only relaxes the *server* side). Paying that once
 * per captured surface would make an exhaustive audit take hours.
 *
 * Instead `roles.setup.ts` drives the real startup flow once per role and
 * saves `storageState({ indexedDB: true })`. Annex keeps identity, keys and
 * the cached `x-annex-zk-proof` payload in IndexedDB (`client/src/lib/db.ts`),
 * so a restored context replays the existing proof instead of generating a new
 * one.
 *
 * Two constraints drive the design:
 *
 * - The cached proof binds to a **Merkle root**, and `scripts/e2e-server.sh`
 *   provisions a fresh SQLite DB per run. Saved state is therefore only valid
 *   against the server instance that produced it, so it is regenerated every
 *   run and never committed.
 * - `ensure_founder` promotes the **earliest registrant** to moderator, so the
 *   founder role has to be created before any other identity touches the
 *   server. `ROLE_ORDER` encodes that and the setup project runs serially.
 */

import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));

/** Roles that hold a registered identity (i.e. everything except `fresh`). */
export type AppRole = 'fresh' | 'member' | 'founder' | 'second-member';
export type WarmRole = Exclude<AppRole, 'fresh'>;

/**
 * Creation order. `founder` MUST be first — the server promotes the earliest
 * registrant to moderator, and that is the only way to reach the admin
 * surfaces.
 */
export const ROLE_ORDER: WarmRole[] = ['founder', 'member', 'second-member'];

/** Generated auth state lives here. Regenerated per run; gitignored. */
export const AUTH_DIR = path.join(HERE, '.auth');

export function storageStatePath(role: WarmRole): string {
  return path.join(AUTH_DIR, `${role}.json`);
}

/**
 * Names for the seeded fixture data, shared between the seeding step and the
 * surface manifest so a rename cannot silently desynchronise them.
 */
export const SEED = {
  /** Seeded by the server itself on first boot (`startup.rs`). */
  defaultChannel: 'General',
  channels: {
    text: 'audit-text',
    voice: 'audit-voice',
    hybrid: 'audit-hybrid',
    agent: 'audit-agent',
    broadcast: 'audit-broadcast',
  },
  /** A channel deliberately left with no messages, to capture the empty state. */
  emptyChannel: 'audit-empty',
  messages: {
    plain: 'Plain seeded message for the audit harness.',
    edited: 'This message was edited by the audit seeder.',
    deleted: 'This message will be deleted by the audit seeder.',
    replyParent: 'Parent message that the audit seeder replies to.',
    reply: 'Reply produced by the audit seeder.',
    long: 'Long message. '.repeat(60).trim(),
  },
} as const;
