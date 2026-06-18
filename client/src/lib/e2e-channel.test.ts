import { describe, it, expect, beforeEach } from 'vitest';
import {
  E2eChannelManager,
  E2eKeyPendingError,
  type E2eChannelApi,
  type E2eKeyStore,
} from './e2e-channel';

/**
 * In-memory fake of the server's content-blind key directory, mirroring the
 * real semantics from `api_e2e.rs`: membership is required to read/write a
 * channel's keys, and the first wrap per (recipient, epoch) wins. The fake
 * stores only what the real server stores — public keys and opaque base64
 * blobs — so it physically cannot read message content.
 */
class FakeServer {
  memberKeys = new Map<string, string>(); // pseudonym -> pubHex
  members = new Map<string, Set<string>>(); // channel -> pseudonyms
  wraps = new Map<string, { epoch: number; sender: string; b64: string }>(); // `chan|recip|epoch`

  addMember(channel: string, pseudonym: string) {
    if (!this.members.has(channel)) this.members.set(channel, new Set());
    this.members.get(channel)!.add(pseudonym);
  }
  private requireMember(channel: string, who: string) {
    if (!this.members.get(channel)?.has(who)) {
      throw new Error(`403: ${who} is not a member of ${channel}`);
    }
  }

  /** Returns a per-identity API view (binds the caller's pseudonym). */
  apiFor(me: string): E2eChannelApi {
    return {
      publishMyKey: async (pubHex) => {
        this.memberKeys.set(me, pubHex);
      },
      getChannelMemberKeys: async (channel) => {
        this.requireMember(channel, me);
        const out: { pseudonym_id: string; x25519_pub_hex: string }[] = [];
        for (const p of this.members.get(channel) ?? []) {
          const k = this.memberKeys.get(p);
          if (k) out.push({ pseudonym_id: p, x25519_pub_hex: k });
        }
        return out;
      },
      postChannelKeyWraps: async (channel, epoch, wraps) => {
        this.requireMember(channel, me);
        let inserted = 0;
        for (const w of wraps) {
          const key = `${channel}|${w.recipient_pseudonym_id}|${epoch}`;
          if (!this.wraps.has(key)) {
            this.wraps.set(key, { epoch, sender: me, b64: w.wrapped_key_b64 });
            inserted++;
          }
        }
        return inserted;
      },
      getChannelKeyWraps: async (channel) => {
        this.requireMember(channel, me);
        const out: { epoch: number; sender_pseudonym_id: string; wrapped_key_b64: string }[] = [];
        for (const [key, v] of this.wraps) {
          const [chan, recip] = key.split('|');
          if (chan === channel && recip === me) {
            out.push({ epoch: v.epoch, sender_pseudonym_id: v.sender, wrapped_key_b64: v.b64 });
          }
        }
        return out;
      },
      getChannelKeyStatus: async (channel) => {
        this.requireMember(channel, me);
        let count = 0;
        let maxEpoch = 0;
        for (const [key, v] of this.wraps) {
          if (key.split('|')[0] === channel) {
            count++;
            maxEpoch = Math.max(maxEpoch, v.epoch);
          }
        }
        return { has_key: count > 0, max_epoch: maxEpoch };
      },
    };
  }
}

function memStore(): E2eKeyStore {
  let deviceSecret: Uint8Array | null = null;
  const channelKeys = new Map<string, { epoch: number; cek: Uint8Array }>();
  return {
    loadDeviceSecret: async () => deviceSecret,
    saveDeviceSecret: async (s) => {
      deviceSecret = s;
    },
    loadChannelKey: async (c) => channelKeys.get(c) ?? null,
    saveChannelKey: async (c, epoch, cek) => {
      channelKeys.set(c, { epoch, cek });
    },
  };
}

describe('E2eChannelManager', () => {
  let server: FakeServer;
  let alice: E2eChannelManager;
  let bob: E2eChannelManager;

  beforeEach(async () => {
    server = new FakeServer();
    server.addMember('chan', 'alice');
    server.addMember('chan', 'bob');
    alice = new E2eChannelManager(server.apiFor('alice'), memStore());
    bob = new E2eChannelManager(server.apiFor('bob'), memStore());
    await alice.ensureDevicePublished();
    await bob.ensureDevicePublished();
  });

  it('two members converge on the same channel key and exchange ciphertext', async () => {
    // Alice provisions the key (implicitly) and encrypts a message.
    const cipher = await alice.encrypt('chan', 'meet at the docks 🚢');
    // The transported body is ciphertext, not the plaintext.
    expect(cipher).not.toContain('docks');

    // Bob independently resolves the channel key and decrypts.
    const plain = await bob.decrypt('chan', cipher);
    expect(plain).toBe('meet at the docks 🚢');
  });

  it('a late joiner can be admitted by an existing member and read history', async () => {
    const cipher = await alice.encrypt('chan', 'earlier secret');

    // Carol joins after the fact.
    server.addMember('chan', 'carol');
    const carol = new E2eChannelManager(server.apiFor('carol'), memStore());
    await carol.ensureDevicePublished();

    // An existing member seals the channel key to carol.
    const carolKey = server.memberKeys.get('carol')!;
    await alice.wrapKeyForNewMember('chan', { pseudonym_id: 'carol', x25519_pub_hex: carolKey });

    // Carol now decrypts the earlier message.
    expect(await carol.decrypt('chan', cipher)).toBe('earlier secret');
  });

  it('an outsider (non-member) cannot resolve or read the channel', async () => {
    const cipher = await alice.encrypt('chan', 'top secret');
    const eve = new E2eChannelManager(server.apiFor('eve'), memStore());
    await eve.ensureDevicePublished(); // eve has a key but is not a member
    await expect(eve.decrypt('chan', cipher)).rejects.toThrow();
  });

  it('the server stores only opaque blobs (no plaintext, no channel key)', async () => {
    const cipher = await alice.encrypt('chan', 'CANARY-PLAINTEXT-9');
    // Everything the server holds:
    const allServerData = [
      ...server.memberKeys.values(),
      ...[...server.wraps.values()].map((w) => w.b64),
      cipher,
    ].join('|');
    expect(allServerData).not.toContain('CANARY-PLAINTEXT-9');
  });

  it('resolving the key is idempotent and de-dupes concurrent calls', async () => {
    const [k1, k2] = await Promise.all([
      alice.resolveChannelKey('chan'),
      alice.resolveChannelKey('chan'),
    ]);
    expect(k1.cek).toEqual(k2.cek);
    // Bob converges to the very same key bytes.
    const kb = await bob.resolveChannelKey('chan');
    expect(kb.cek).toEqual(k1.cek);
  });

  it('a member who published a key late does not mint a rival key', async () => {
    // Alice provisions while Dave (a member) has not published a key yet, so
    // Alice's provisioning wraps only to herself + bob.
    server.addMember('chan', 'dave');
    const cipher = await alice.encrypt('chan', 'classified');

    const dave = new E2eChannelManager(server.apiFor('dave'), memStore());
    await dave.ensureDevicePublished(); // dave now in the directory, but no wrap yet

    // Dave must NOT provision a rival key — the channel already has one.
    await expect(dave.resolveChannelKey('chan')).rejects.toBeInstanceOf(E2eKeyPendingError);

    // An existing member opens the channel and reconciles, admitting dave.
    await alice.reconcile('chan');

    // Dave can now read the message under the SAME key (no divergence).
    expect(await dave.decrypt('chan', cipher)).toBe('classified');
  });

  it('forget() clears cached key material', async () => {
    await alice.resolveChannelKey('chan');
    alice.forget();
    // Still works afterwards (re-resolves from the server wrap).
    const cipher = await bob.encrypt('chan', 'after forget');
    expect(await alice.decrypt('chan', cipher)).toBe('after forget');
  });
});
