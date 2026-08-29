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
import { E2eKeyPendingError, isE2eBody } from './e2e-channel';

export { isE2eBody };

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

/**
 * Channels whose encryption state could not be determined.
 *
 * Distinct from "not encrypted" on purpose. `getChannelE2e` failing — a
 * dropped request, a 500, a token refresh mid-flight — used to be recorded
 * as `markChannelE2e(id, false)`, which is the same value a genuinely
 * plaintext channel has. `sendMessage` then took its plaintext branch and
 * put unencrypted content on the wire for the rest of the session, in a
 * channel the user had turned encryption on for.
 *
 * That defeats the rule stated at the send site — "NEVER send plaintext to
 * an E2E channel" — from upstream, by making the channel merely not *known*
 * to be E2E. For encryption, an unknown state has to fail closed: the one
 * outcome that cannot be undone is content already sent in the clear.
 */
const e2eUnknownChannels = new Set<string>();

/** Records that this channel's encryption state could not be established. */
export function markChannelE2eUnknown(channelId: string): void {
  e2eUnknownChannels.add(channelId);
  e2eChannels.delete(channelId);
}

/** True when the encryption state is unresolved and sending must not proceed. */
export function isChannelE2eUnknown(channelId: string): boolean {
  return e2eUnknownChannels.has(channelId);
}

/**
 * Whether this client can actually read an E2E channel.
 *
 * `resolveChannelKey` already distinguishes the two ways it can come back
 * without a key: `E2eKeyPendingError` means the channel HAS key material and
 * none of it is sealed to us yet — we are waiting to be admitted by a member
 * who holds it — while anything else is a genuine failure. `ensureChannelReady`
 * threw both away in one bare `catch`, so both looked identical from the UI:
 * a wall of "🔒 encrypted message (no key)" under a status bar reading
 * "End-to-end encrypted — the server can't read these messages." True, and
 * useless. Nothing said why the messages were unreadable, that the state
 * resolves on its own, or that anything had gone wrong.
 *
 * `pending` is not an error. Any member holding the key admits every current
 * member on channel open (`E2eChannelManager.reconcile`), and our device key
 * is published before we ever get here, so the wait ends without either party
 * doing anything deliberate.
 */
export type ChannelKeyState = 'ready' | 'pending' | 'failed';

const keyPendingChannels = new Set<string>();
const keyFailedChannels = new Map<string, string>();

export function getChannelKeyState(channelId: string): ChannelKeyState {
  if (keyPendingChannels.has(channelId)) return 'pending';
  if (keyFailedChannels.has(channelId)) return 'failed';
  return 'ready';
}

/** Why the channel key could not be resolved, for the `failed` state only. */
export function getChannelKeyError(channelId: string): string | null {
  return keyFailedChannels.get(channelId) ?? null;
}

/** Forget all E2E channel flags (e.g. on logout / identity switch). */
export function resetE2eChannels(): void {
  e2eChannels.clear();
  e2eUnknownChannels.clear();
  keyPendingChannels.clear();
  keyFailedChannels.clear();
}

/**
 * Warm the channel key before the user can send: publish our device key and
 * resolve/provision the CEK. Safe to call repeatedly (cached). No-op for
 * non-E2E channels or when no identity is active.
 */
export async function ensureChannelReady(channelId: string): Promise<void> {
  if (!isChannelE2e(channelId) || !activePseudonym) return;
  const mgr = getE2eChannelManager(activePseudonym);
  keyPendingChannels.delete(channelId);
  keyFailedChannels.delete(channelId);
  await mgr.ensureDevicePublished();
  try {
    await mgr.resolveChannelKey(channelId);
  } catch (err) {
    // Neither branch is fatal — the channel still opens, and sending still
    // refuses rather than leaking plaintext. What each needs is to be
    // visible, and they are not the same thing to say.
    if (err instanceof E2eKeyPendingError) {
      keyPendingChannels.add(channelId);
    } else {
      keyFailedChannels.set(
        channelId,
        err instanceof Error ? err.message : 'the channel key could not be resolved',
      );
    }
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
 * Turn an inbound body into display text. The decision to decrypt is
 * PER-MESSAGE — driven by the body's E2E marker, not the channel's current
 * `e2e_enabled` flag — so history renders correctly even after a moderator
 * toggles E2E on or off: marked bodies are always decrypted (the key wraps still
 * exist), unmarked/plaintext bodies always pass through. Returns a placeholder
 * (never throws) when an E2E body can't be opened, so one bad message can't
 * break the timeline.
 */
export async function decryptForDisplay(channelId: string, content: string): Promise<string> {
  if (!activePseudonym || !isE2eBody(content)) return content;
  try {
    return await getE2eChannelManager(activePseudonym).decrypt(channelId, content);
  } catch {
    return UNDECRYPTABLE_PLACEHOLDER;
  }
}
