/**
 * Message-level E2E glue between the channels store and the channel-key manager.
 *
 * This is the single seam the store touches. For non-E2E channels every
 * function is a transparent pass-through, so default behaviour is byte-for-byte
 * unchanged. For E2E channels it encrypts outgoing bodies and decrypts incoming
 * ones, and — critically — NEVER falls back to sending plaintext to an E2E
 * channel (a failure throws instead, so the optimistic message is marked failed
 * rather than leaking cleartext).
 */

import { getE2eChannelManager } from './e2e-store';

/** Placeholder shown when an inbound E2E body cannot be decrypted. */
export const UNDECRYPTABLE_PLACEHOLDER = '🔒 encrypted message (no key)';

const e2eChannels = new Set<string>();
let activePseudonym: string | null = null;

/** Set (or clear) the identity whose device key unlocks E2E channels. */
export function setE2eIdentity(pseudonymId: string | null): void {
  activePseudonym = pseudonymId;
}

/** Record whether a channel is end-to-end encrypted. */
export function markChannelE2e(channelId: string, enabled: boolean): void {
  if (enabled) e2eChannels.add(channelId);
  else e2eChannels.delete(channelId);
}

export function isChannelE2e(channelId: string): boolean {
  return e2eChannels.has(channelId);
}

/** Forget all E2E channel flags (e.g. on logout / identity switch). */
export function resetE2eChannels(): void {
  e2eChannels.clear();
}

/**
 * Warm the channel key before the user can send: publish our device key and
 * resolve/provision the CEK. Safe to call repeatedly (cached). No-op for
 * non-E2E channels or when no identity is active.
 */
export async function ensureChannelReady(channelId: string): Promise<void> {
  if (!isChannelE2e(channelId) || !activePseudonym) return;
  const mgr = getE2eChannelManager(activePseudonym);
  await mgr.ensureDevicePublished();
  try {
    await mgr.resolveChannelKey(channelId);
  } catch {
    // Key not yet available (we're awaiting admission by an existing member).
    // Non-fatal: opening the channel still works, bodies just stay sealed
    // until we're admitted.
  }
  // If we hold the key, admit any members who joined / published a key after
  // it was provisioned (idempotent, best-effort).
  await mgr.reconcile(channelId);
}

/**
 * Produce the body to put on the wire. Pass-through for non-E2E channels;
 * ciphertext for E2E channels. Throws for an E2E channel with no usable
 * identity/key so the caller can fail the send instead of leaking plaintext.
 */
export async function encryptForWire(channelId: string, plaintext: string): Promise<string> {
  if (!isChannelE2e(channelId)) return plaintext;
  if (!activePseudonym) {
    throw new Error('Cannot encrypt for E2E channel: no active identity.');
  }
  return getE2eChannelManager(activePseudonym).encrypt(channelId, plaintext);
}

/**
 * Turn an inbound body into display text. Pass-through for non-E2E channels;
 * decrypts for E2E channels, returning a placeholder (never throwing) when the
 * key is unavailable so one bad message can't break the timeline.
 */
export async function decryptForDisplay(channelId: string, content: string): Promise<string> {
  if (!isChannelE2e(channelId) || !activePseudonym) return content;
  try {
    return await getE2eChannelManager(activePseudonym).decrypt(channelId, content);
  } catch {
    return UNDECRYPTABLE_PLACEHOLDER;
  }
}
