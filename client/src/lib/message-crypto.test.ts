import { describe, it, expect, beforeEach, vi } from 'vitest';

// Mock the manager factory so we exercise message-crypto's decision logic
// without IndexedDB or the network.
const fakeManager = {
  ensureDevicePublished: vi.fn(async () => 'pubhex'),
  resolveChannelKey: vi.fn(async () => ({ epoch: 1, cek: new Uint8Array(32) })),
  reconcile: vi.fn(async () => {}),
  // Mirror the real manager: ciphertext carries the 'e2e1:' marker, and decrypt
  // strips it before processing.
  encrypt: vi.fn(async (_channelId: string, plaintext: string) => `e2e1:ENC(${plaintext})`),
  decrypt: vi.fn(async (_channelId: string, body: string) => {
    const inner = body.startsWith('e2e1:') ? body.slice('e2e1:'.length) : body;
    if (inner === 'BAD') throw new Error('cannot decrypt');
    return inner.replace(/^ENC\((.*)\)$/, '$1');
  }),
};

vi.mock('./e2e-store', () => ({
  getE2eChannelManager: () => fakeManager,
}));

import {
  decryptForDisplay,
  encryptForWire,
  ensureChannelReady,
  getChannelKeyError,
  getChannelKeyState,
  isChannelE2e,
  markChannelE2e,
  resetE2eChannels,
  setE2eIdentity,
  UNDECRYPTABLE_PLACEHOLDER,
} from './message-crypto';
import { E2eKeyPendingError } from './e2e-channel';

describe('message-crypto', () => {
  beforeEach(() => {
    resetE2eChannels();
    setE2eIdentity(null);
    vi.clearAllMocks();
  });

  it('is a transparent pass-through for non-E2E channels', async () => {
    expect(isChannelE2e('plain')).toBe(false);
    expect(await encryptForWire('plain', 'hello')).toBe('hello');
    expect(await decryptForDisplay('plain', 'hello')).toBe('hello');
    expect(fakeManager.encrypt).not.toHaveBeenCalled();
    expect(fakeManager.decrypt).not.toHaveBeenCalled();
  });

  it('encrypts and decrypts for E2E channels when an identity is set', async () => {
    setE2eIdentity('alice');
    markChannelE2e('secret', true);
    expect(isChannelE2e('secret')).toBe(true);

    const wire = await encryptForWire('secret', 'top secret');
    expect(wire).toBe('e2e1:ENC(top secret)');
    expect(await decryptForDisplay('secret', wire)).toBe('top secret');
  });

  it('decrypts marked bodies regardless of the channel flag (toggle-independent)', async () => {
    setE2eIdentity('alice');
    // Channel flag is OFF (e.g. E2E was disabled after these messages were sent)…
    expect(isChannelE2e('chan')).toBe(false);
    // …but a marked body still decrypts (the key wraps still exist).
    expect(await decryptForDisplay('chan', 'e2e1:ENC(history)')).toBe('history');
    // And an unmarked plaintext body passes through even when the flag is ON.
    markChannelE2e('chan', true);
    expect(await decryptForDisplay('chan', 'just plaintext')).toBe('just plaintext');
  });

  it('refuses to emit plaintext to an E2E channel with no identity', async () => {
    markChannelE2e('secret', true);
    // No identity set.
    await expect(encryptForWire('secret', 'leak?')).rejects.toThrow();
    expect(fakeManager.encrypt).not.toHaveBeenCalled();
  });

  it('returns a placeholder (never throws) when a body cannot be decrypted', async () => {
    setE2eIdentity('alice');
    markChannelE2e('secret', true);
    expect(await decryptForDisplay('secret', 'e2e1:BAD')).toBe(UNDECRYPTABLE_PLACEHOLDER);
  });

  it('ensureChannelReady warms the key only for E2E channels with an identity', async () => {
    // Non-E2E: no-op.
    await ensureChannelReady('plain');
    expect(fakeManager.resolveChannelKey).not.toHaveBeenCalled();

    // E2E + identity: publishes device key and resolves the channel key.
    setE2eIdentity('alice');
    markChannelE2e('secret', true);
    await ensureChannelReady('secret');
    expect(fakeManager.ensureDevicePublished).toHaveBeenCalled();
    expect(fakeManager.resolveChannelKey).toHaveBeenCalledWith('secret');
  });

  it('markChannelE2e(false) and resetE2eChannels clear the flag', async () => {
    markChannelE2e('secret', true);
    expect(isChannelE2e('secret')).toBe(true);
    markChannelE2e('secret', false);
    expect(isChannelE2e('secret')).toBe(false);

    markChannelE2e('a', true);
    markChannelE2e('b', true);
    resetE2eChannels();
    expect(isChannelE2e('a')).toBe(false);
    expect(isChannelE2e('b')).toBe(false);
  });
});

/**
 * `ensureChannelReady` swallowed every `resolveChannelKey` failure in one bare
 * `catch`, which erased a distinction the layer below had already drawn.
 * `E2eKeyPendingError` means the channel is keyed and we are simply waiting to
 * be admitted — it resolves without anyone doing anything, because any
 * key-holder admits every member on channel open. Anything else is a real
 * failure that will not resolve. Collapsing them left the UI with nothing to
 * say in either case, so both rendered as a wall of "no key" placeholders
 * under a bar reporting that all was well.
 */
describe('message-crypto — waiting for the key is not the same as failing to get it', () => {
  beforeEach(() => {
    resetE2eChannels();
    setE2eIdentity('p-self');
    vi.clearAllMocks();
    fakeManager.resolveChannelKey.mockResolvedValue({ epoch: 1, cek: new Uint8Array(32) });
  });

  it('records a pending key when the channel is keyed but we are not admitted', async () => {
    markChannelE2e('ch-pending', true);
    fakeManager.resolveChannelKey.mockRejectedValueOnce(new E2eKeyPendingError('ch-pending'));

    await ensureChannelReady('ch-pending');

    expect(getChannelKeyState('ch-pending')).toBe('pending');
    expect(getChannelKeyError('ch-pending')).toBeNull();
  });

  it('records a failure, with its reason, for anything else', async () => {
    markChannelE2e('ch-broken', true);
    fakeManager.resolveChannelKey.mockRejectedValueOnce(new Error('network down'));

    await ensureChannelReady('ch-broken');

    expect(getChannelKeyState('ch-broken')).toBe('failed');
    expect(getChannelKeyError('ch-broken')).toBe('network down');
  });

  it('reports ready when the key resolves, and clears a prior pending state', async () => {
    markChannelE2e('ch-ok', true);
    fakeManager.resolveChannelKey.mockRejectedValueOnce(new E2eKeyPendingError('ch-ok'));
    await ensureChannelReady('ch-ok');
    expect(getChannelKeyState('ch-ok')).toBe('pending');

    // Admitted since — a second attempt must not stay stuck on the old answer.
    await ensureChannelReady('ch-ok');
    expect(getChannelKeyState('ch-ok')).toBe('ready');
  });

  it('forgets both states on identity switch', async () => {
    markChannelE2e('ch-pending', true);
    fakeManager.resolveChannelKey.mockRejectedValueOnce(new E2eKeyPendingError('ch-pending'));
    await ensureChannelReady('ch-pending');

    resetE2eChannels();
    expect(getChannelKeyState('ch-pending')).toBe('ready');
  });
});
