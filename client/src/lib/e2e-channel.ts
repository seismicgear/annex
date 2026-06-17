/**
 * E2E channel key manager — the client orchestration above `@/lib/e2e`.
 *
 * Responsibilities:
 *   1. Maintain this device's long-term X25519 key and publish its public half
 *      to the server directory so others can seal channel keys to us.
 *   2. Resolve a channel's content key (CEK): adopt the sealed key already
 *      addressed to us if one exists, otherwise provision a fresh CEK and seal
 *      it to every current member (humans AND agents alike).
 *   3. Encrypt outgoing / decrypt incoming message bodies for E2E channels.
 *
 * The server never sees the CEK or any plaintext: it stores only public keys
 * and opaque sealed blobs. This module is dependency-injected (api + store) so
 * it is fully unit-testable without a network or IndexedDB.
 */

import {
  decryptContent,
  encryptContent,
  fromBase64,
  fromHex,
  generateChannelKey,
  generateDeviceSecret,
  openFrom,
  publicKeyFromSecret,
  sealTo,
  toBase64,
  toHex,
  utf8,
} from './e2e';

/** Server endpoints this manager needs. Matches `@/api/e2e`. */
export interface E2eChannelApi {
  publishMyKey(x25519PubHex: string): Promise<void>;
  getChannelMemberKeys(channelId: string): Promise<{ pseudonym_id: string; x25519_pub_hex: string }[]>;
  postChannelKeyWraps(
    channelId: string,
    epoch: number,
    wraps: { recipient_pseudonym_id: string; wrapped_key_b64: string }[],
  ): Promise<number>;
  getChannelKeyWraps(
    channelId: string,
  ): Promise<{ epoch: number; sender_pseudonym_id: string; wrapped_key_b64: string }[]>;
}

/** Persistent storage for this device's secret and resolved channel keys. */
export interface E2eKeyStore {
  loadDeviceSecret(): Promise<Uint8Array | null>;
  saveDeviceSecret(secret: Uint8Array): Promise<void>;
  loadChannelKey(channelId: string): Promise<{ epoch: number; cek: Uint8Array } | null>;
  saveChannelKey(channelId: string, epoch: number, cek: Uint8Array): Promise<void>;
}

export class E2eChannelManager {
  private deviceSecret: Uint8Array | null = null;
  private readonly cache = new Map<string, { epoch: number; cek: Uint8Array }>();
  /** De-dupes concurrent resolves for the same channel. */
  private readonly inflight = new Map<string, Promise<{ epoch: number; cek: Uint8Array }>>();

  constructor(
    private readonly api: E2eChannelApi,
    private readonly store: E2eKeyStore,
  ) {}

  /** Load or generate this device's X25519 secret and publish its public half. */
  async ensureDevicePublished(): Promise<string> {
    const secret = await this.getDeviceSecret();
    const pubHex = toHex(publicKeyFromSecret(secret));
    await this.api.publishMyKey(pubHex);
    return pubHex;
  }

  private async getDeviceSecret(): Promise<Uint8Array> {
    if (this.deviceSecret) return this.deviceSecret;
    let secret = await this.store.loadDeviceSecret();
    if (!secret) {
      secret = generateDeviceSecret();
      await this.store.saveDeviceSecret(secret);
    }
    this.deviceSecret = secret;
    return secret;
  }

  /**
   * Resolve the channel content key. Converges all members onto a single key:
   * if a sealed key is already addressed to us we adopt it; otherwise we
   * provision one, seal it to every member, then re-read so we adopt whichever
   * wrap won the first-write-wins race at the server.
   */
  async resolveChannelKey(channelId: string): Promise<{ epoch: number; cek: Uint8Array }> {
    const cached = this.cache.get(channelId) ?? (await this.store.loadChannelKey(channelId));
    if (cached) {
      this.cache.set(channelId, cached);
      return cached;
    }
    const existing = this.inflight.get(channelId);
    if (existing) return existing;

    const job = this.doResolve(channelId).finally(() => this.inflight.delete(channelId));
    this.inflight.set(channelId, job);
    return job;
  }

  private async doResolve(channelId: string): Promise<{ epoch: number; cek: Uint8Array }> {
    const secret = await this.getDeviceSecret();

    // 1. Adopt a key already sealed to us, if any (highest epoch wins).
    const mine = await this.adoptOwnWrap(channelId, secret);
    if (mine) return this.remember(channelId, mine);

    // 2. Provision a fresh CEK and seal it to every current member.
    const members = await this.api.getChannelMemberKeys(channelId);
    const cek = generateChannelKey();
    const epoch = 1;
    const wraps = members.map((m) => ({
      recipient_pseudonym_id: m.pseudonym_id,
      wrapped_key_b64: toBase64(sealTo(cek, fromHex(m.x25519_pub_hex))),
    }));
    if (wraps.length > 0) {
      await this.api.postChannelKeyWraps(channelId, epoch, wraps);
    }

    // 3. Re-read: another member may have provisioned first; adopt the winner.
    const won = await this.adoptOwnWrap(channelId, secret);
    if (won) return this.remember(channelId, won);

    // No directory entry for ourselves yet (e.g. we hadn't published a key):
    // fall back to the key we just generated.
    return this.remember(channelId, { epoch, cek });
  }

  /** Fetch the wraps addressed to us and open the highest-epoch one we can. */
  private async adoptOwnWrap(
    channelId: string,
    secret: Uint8Array,
  ): Promise<{ epoch: number; cek: Uint8Array } | null> {
    const wraps = await this.api.getChannelKeyWraps(channelId);
    const sorted = [...wraps].sort((a, b) => b.epoch - a.epoch);
    for (const w of sorted) {
      try {
        const cek = openFrom(fromBase64(w.wrapped_key_b64), secret);
        return { epoch: w.epoch, cek };
      } catch {
        // Not openable by us (garbage or wrong epoch); keep looking.
      }
    }
    return null;
  }

  private async remember(
    channelId: string,
    entry: { epoch: number; cek: Uint8Array },
  ): Promise<{ epoch: number; cek: Uint8Array }> {
    this.cache.set(channelId, entry);
    await this.store.saveChannelKey(channelId, entry.epoch, entry.cek);
    return entry;
  }

  /**
   * Seal the current channel key to a newly-joined member so they can read the
   * channel. Any existing member can call this. No-op if we cannot resolve the
   * key or the member has no published key.
   */
  async wrapKeyForNewMember(
    channelId: string,
    member: { pseudonym_id: string; x25519_pub_hex: string },
  ): Promise<void> {
    const { epoch, cek } = await this.resolveChannelKey(channelId);
    const wrapped = toBase64(sealTo(cek, fromHex(member.x25519_pub_hex)));
    await this.api.postChannelKeyWraps(channelId, epoch, [
      { recipient_pseudonym_id: member.pseudonym_id, wrapped_key_b64: wrapped },
    ]);
  }

  /** Encrypt a message body for an E2E channel; returns base64 ciphertext. */
  async encrypt(channelId: string, plaintext: string): Promise<string> {
    const { cek } = await this.resolveChannelKey(channelId);
    const aad = utf8.encode(channelId);
    return toBase64(encryptContent(cek, utf8.encode(plaintext), aad));
  }

  /** Decrypt a base64 ciphertext body from an E2E channel back to text. */
  async decrypt(channelId: string, contentB64: string): Promise<string> {
    const { cek } = await this.resolveChannelKey(channelId);
    const aad = utf8.encode(channelId);
    return utf8.decode(decryptContent(cek, fromBase64(contentB64), aad));
  }

  /** Drop cached key material (e.g. on logout). */
  forget(): void {
    this.deviceSecret = null;
    this.cache.clear();
    this.inflight.clear();
  }
}
