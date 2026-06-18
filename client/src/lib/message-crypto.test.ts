import { describe, it, expect, beforeEach, vi } from 'vitest';

// Mock the manager factory so we exercise message-crypto's decision logic
// without IndexedDB or the network.
const fakeManager = {
  ensureDevicePublished: vi.fn(async () => 'pubhex'),
  resolveChannelKey: vi.fn(async () => ({ epoch: 1, cek: new Uint8Array(32) })),
  encrypt: vi.fn(async (_channelId: string, plaintext: string) => `ENC(${plaintext})`),
  decrypt: vi.fn(async (_channelId: string, content: string) => {
    if (content === 'BAD') throw new Error('cannot decrypt');
    return content.replace(/^ENC\((.*)\)$/, '$1');
  }),
};

vi.mock('./e2e-store', () => ({
  getE2eChannelManager: () => fakeManager,
}));

import {
  decryptForDisplay,
  encryptForWire,
  ensureChannelReady,
  isChannelE2e,
  markChannelE2e,
  resetE2eChannels,
  setE2eIdentity,
  UNDECRYPTABLE_PLACEHOLDER,
} from './message-crypto';

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
    expect(wire).toBe('ENC(top secret)');
    expect(await decryptForDisplay('secret', wire)).toBe('top secret');
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
    expect(await decryptForDisplay('secret', 'BAD')).toBe(UNDECRYPTABLE_PLACEHOLDER);
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
