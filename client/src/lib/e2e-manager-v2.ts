/** Device-aware key distribution. No uncommitted candidate is ever usable. */
import { E2eError, decryptContent, generateChannelKey, openFrom, toHex, utf8 } from './e2e';
import { E2eKeyPendingError } from './e2e-channel';
import {
  channelKeyId, decodeCanonicalBase64, decodeHex32, decodeUtf8Strict,
  decryptMessageV2, encryptMessageV2, openChannelKeyV2, parseMessageV2, sealChannelKeyV2,
} from './e2e-v2';
import type {
  ChannelKeyDirectory, ChannelKeyState, ChannelWrapCommit, DeviceKeyWrap, E2eApiV2,
  E2eKeyringV2, KeyPage, LegacyCursor, LocalEncryptionDevice, MemberDeviceKey,
  MissingEpochDevices, RetainedChannelKey,
} from './e2e-v2-protocol';

const LAST_EPOCH = Number.MAX_SAFE_INTEGER;
const LAST_HASH = 'f'.repeat(64);
const PAGE_LIMIT = 200;
const MAX_PAGES = 1000;
const MAX_ATTEMPTS = 4;

class SnapshotChanged extends E2eError {
  constructor() { super('Encryption membership changed. Refreshing the key snapshot.'); }
}
interface Operation {
  check(): void;
  wait<T>(pending: Promise<T>): Promise<T>;
}
interface ChannelContext {
  state: ChannelKeyState;
  keys: Map<string, RetainedChannelKey>;
  loaded: boolean;
  ownedCurrent: Set<string>;
  ownedLegacy: Set<string>;
  ownedRawLegacyEpochs: Set<number>;
  unscopedLegacy?: { epoch: number; cek: Uint8Array; verified: boolean };
}
export interface ResolvedChannelKey extends RetainedChannelKey {
  purpose: 'current';
  serverIdentity: string;
  channelId: string;
}
function keyReference(key: Pick<RetainedChannelKey, 'purpose' | 'epoch' | 'keyId'>): string {
  return `${key.purpose}:${key.epoch}:${key.keyId}`;
}
function isConflict(error: unknown): boolean {
  return error instanceof SnapshotChanged || (error instanceof Error && 'status' in error && error.status === 409);
}
function nextEpoch(previous: number, entries: { epoch: number }[], next: number | null): number {
  let last = previous;
  for (const entry of entries) {
    if (entry.epoch <= last) throw new E2eError('Key history is not strictly ordered');
    last = entry.epoch;
  }
  if (next !== null && (next <= previous || next !== last)) throw new E2eError('Invalid key-history cursor');
  return next ?? LAST_EPOCH;
}
function compareLegacy(a: LegacyCursor, b: LegacyCursor): number {
  return a.epoch - b.epoch || (a.key_id === b.key_id ? 0 : a.key_id < b.key_id ? -1 : 1);
}
function nextLegacy(previous: LegacyCursor, entries: LegacyCursor[], next: LegacyCursor | null): LegacyCursor {
  let last = previous;
  for (const entry of entries) {
    if (compareLegacy(entry, last) <= 0) throw new E2eError('Legacy key history is not strictly ordered');
    last = entry;
  }
  if (next !== null && (compareLegacy(next, previous) <= 0 || compareLegacy(next, last) !== 0)) {
    throw new E2eError('Invalid legacy key-history cursor');
  }
  return next ?? { epoch: LAST_EPOCH, key_id: LAST_HASH };
}

export class E2eChannelManagerV2 {
  private generation = 0;
  private serverIdentity: string | null = null;
  private device: LocalEncryptionDevice | null = null;
  private devicePending: Promise<LocalEncryptionDevice> | null = null;
  private published = false;
  private publicationPending: Promise<void> | null = null;
  private readonly channels = new Map<string, ChannelContext>();
  private readonly synchronization = new Map<string, Promise<ResolvedChannelKey | null>>();

  constructor(
    private readonly pseudonym: string,
    private readonly api: E2eApiV2,
    private readonly keyring: E2eKeyringV2,
  ) {}

  private operation(): Operation {
    const generation = this.generation;
    const check = () => {
      this.api.assertCurrent();
      if (generation !== this.generation) throw new E2eError('Encryption operation was cancelled');
    };
    check();
    return {
      check,
      async wait<T>(pending: Promise<T>): Promise<T> {
        try { const result = await pending; check(); return result; }
        catch (error) { check(); throw error; }
      },
    };
  }

  private observe(channelId: string, state: ChannelKeyState): ChannelContext {
    if (this.serverIdentity !== null && this.serverIdentity !== state.server_identity) {
      throw new E2eError('The encryption server identity changed. Reconnect before continuing.');
    }
    this.serverIdentity = state.server_identity;
    let context = this.channels.get(channelId);
    if (context) {
      if (context.state.authority_identity !== state.authority_identity) throw new E2eError('Channel encryption authority changed');
      if (state.revision < context.state.revision || state.current_epoch < context.state.current_epoch) throw new SnapshotChanged();
      if (state.current_epoch > 0 && state.current_epoch === context.state.current_epoch
        && state.current_key_id !== context.state.current_key_id) throw new E2eError('An established key epoch changed');
      context.state = state;
    } else {
      context = { state, keys: new Map(), loaded: false,
        ownedCurrent: new Set(), ownedLegacy: new Set(), ownedRawLegacyEpochs: new Set() };
      this.channels.set(channelId, context);
    }
    return context;
  }

  private async readState(channelId: string, op: Operation): Promise<ChannelContext> {
    return this.observe(channelId, await op.wait(this.api.getState(channelId)));
  }

  private async loadKeys(channelId: string, context: ChannelContext, op: Operation): Promise<void> {
    if (context.loaded) return;
    const keys = await op.wait(this.keyring.getKeys(context.state.authority_identity, this.pseudonym, channelId));
    for (const key of keys) this.cacheKey(context, key);
    // Keep the old unscoped record intact. It becomes a verified candidate only
    // after it authenticates a legacy message in this channel.
    const legacy = await op.wait(this.keyring.getUnscopedLegacyKey(this.pseudonym, channelId));
    if (legacy) context.unscopedLegacy = { ...legacy, verified: false };
    context.loaded = true;
  }

  private cacheKey(context: ChannelContext, key: RetainedChannelKey): void {
    if (!key.authorityServer || channelKeyId(key.cek) !== key.keyId) {
      throw new E2eError('Stored encryption key does not match this channel context');
    }
    if (key.purpose === 'current') {
      if (key.epoch === context.state.current_epoch && key.keyId !== context.state.current_key_id) {
        throw new E2eError('Stored channel key conflicts with the established epoch');
      }
      for (const previous of context.keys.values()) {
        if (previous.purpose === 'current' && previous.epoch === key.epoch && previous.keyId !== key.keyId) {
          throw new E2eError('An immutable channel key cannot be replaced');
        }
      }
    }
    const previous = context.keys.get(keyReference(key));
    if (previous && previous.authorityServer !== key.authorityServer) throw new E2eError('A stored epoch cannot change its authenticated namespace');
    context.keys.set(keyReference(key), { ...key, cek: key.cek.slice(),
      verifiedLegacy: key.verifiedLegacy || previous?.verifiedLegacy });
  }

  private async retain(channelId: string, context: ChannelContext, key: RetainedChannelKey, op: Operation): Promise<void> {
    op.check();
    // Validate against already retained epochs before writing durable state.
    this.cacheKey(context, key);
    await op.wait(this.keyring.putKey(context.state.authority_identity, this.pseudonym, channelId, key));
  }

  private async getDevice(context: ChannelContext, op: Operation): Promise<LocalEncryptionDevice> {
    if (this.device) return this.device;
    if (!this.devicePending) {
      this.devicePending = this.keyring.getOrCreateDevice(context.state.server_identity, this.pseudonym);
    }
    const pending = this.devicePending;
    try {
      const device = await op.wait(pending);
      this.device = device;
      return device;
    } finally {
      if (this.devicePending === pending) this.devicePending = null;
    }
  }

  private async enroll(context: ChannelContext, op: Operation): Promise<LocalEncryptionDevice> {
    const device = await this.getDevice(context, op);
    if (!this.published) {
      if (!this.publicationPending) {
        this.publicationPending = this.api.publishDevice(toHex(device.publicKey)).then(result => {
          op.check();
          if (result.device_id !== device.deviceId) throw new E2eError('The key directory returned another device');
          this.published = true;
        });
      }
      const pending = this.publicationPending;
      try { await op.wait(pending); }
      finally { if (this.publicationPending === pending) this.publicationPending = null; }
    }
    return device;
  }

  /** Establish the device without overwriting any prior device's secret. */
  async ensureDevicePublished(channelId: string): Promise<string> {
    const op = this.operation();
    const context = await this.readState(channelId, op);
    return (await this.enroll(context, op)).deviceId;
  }

  private async adoptWrap(
    channelId: string, context: ChannelContext, device: LocalEncryptionDevice,
    wrap: DeviceKeyWrap, purpose: 'current' | 'legacy', op: Operation,
  ): Promise<void> {
    if (wrap.recipient_device_id !== device.deviceId) throw new E2eError('A key wrap targets another device');
    const cek = openChannelKeyV2(wrap.wrapped_key_b64, device.secret, {
      authorityServer: wrap.authority_namespace, channelId, epoch: wrap.epoch,
      keyId: wrap.key_id, recipientPseudonym: this.pseudonym, recipientDeviceId: device.deviceId,
    }, purpose);
    await this.retain(channelId, context, { authorityServer: wrap.authority_namespace,
      epoch: wrap.epoch, keyId: wrap.key_id, cek, purpose, verifiedLegacy: false }, op);
  }

  private async readOwnWraps(channelId: string, context: ChannelContext, device: LocalEncryptionDevice, op: Operation): Promise<void> {
    const secrets = await op.wait(this.keyring.getLegacyDeviceSecrets(context.state.server_identity, this.pseudonym));
    let epoch = 0;
    let legacy: LegacyCursor = { epoch: 0, key_id: '' };
    let rawEpoch = 0;
    let revision: number | null = null;
    const ownedCurrent = new Set<string>();
    const ownedLegacy = new Set<string>();
    const ownedRawLegacyEpochs = new Set<number>();
    for (let pageNumber = 0; pageNumber < MAX_PAGES; pageNumber++) {
      const page = await op.wait(this.api.getWraps(channelId, device.deviceId, {
        limit: PAGE_LIMIT, after_epoch: epoch, legacy_after_epoch: legacy.epoch,
        ...(legacy.key_id ? { legacy_after_key_id: legacy.key_id } : {}), legacy_wrap_after_epoch: rawEpoch,
      }));
      this.observe(channelId, page.state);
      if (revision !== null && revision !== page.state.revision) throw new SnapshotChanged();
      revision = page.state.revision;
      epoch = nextEpoch(epoch, page.wraps, page.next_after_epoch);
      legacy = nextLegacy(legacy, page.legacy_device_wraps, page.next_legacy_cursor);
      rawEpoch = nextEpoch(rawEpoch, page.legacy_wraps, page.next_legacy_wrap_after_epoch);
      for (const wrap of page.wraps) {
        await this.adoptWrap(channelId, context, device, wrap, 'current', op);
        ownedCurrent.add(keyReference({ purpose: 'current', epoch: wrap.epoch, keyId: wrap.key_id }));
      }
      for (const wrap of page.legacy_device_wraps) {
        await this.adoptWrap(channelId, context, device, wrap, 'legacy', op);
        ownedLegacy.add(keyReference({ purpose: 'legacy', epoch: wrap.epoch, keyId: wrap.key_id }));
      }
      for (const wrap of page.legacy_wraps) {
        ownedRawLegacyEpochs.add(wrap.epoch);
        const blob = decodeCanonicalBase64(wrap.wrapped_key_b64, 3072);
        for (const secret of secrets) {
          let cek: Uint8Array;
          try { cek = openFrom(blob, secret); }
          catch { continue; } // An older linked device may own this raw wrap.
          if (cek.length !== 32) throw new E2eError('Legacy sealed key has an invalid length');
          const prior = context.unscopedLegacy;
          await this.retain(channelId, context, { authorityServer: wrap.authority_namespace,
            epoch: wrap.epoch, keyId: channelKeyId(cek), cek, purpose: 'legacy',
            verifiedLegacy: prior?.verified === true && channelKeyId(prior.cek) === channelKeyId(cek) }, op);
          break;
        }
      }
      if (page.next_after_epoch === null && page.next_legacy_cursor === null && page.next_legacy_wrap_after_epoch === null) {
        context.ownedCurrent = ownedCurrent;
        context.ownedLegacy = ownedLegacy;
        context.ownedRawLegacyEpochs = ownedRawLegacyEpochs;
        return;
      }
    }
    throw new E2eError('Key history exceeds the synchronization limit');
  }

  private async directory(channelId: string, op: Operation): Promise<ChannelKeyDirectory> {
    let history = 0;
    let legacy: LegacyCursor = { epoch: 0, key_id: '' };
    let directory: ChannelKeyDirectory | null = null;
    const historical: MissingEpochDevices[] = [];
    const candidates: MissingEpochDevices[] = [];
    for (let pageNumber = 0; pageNumber < MAX_PAGES; pageNumber++) {
      const page: KeyPage = { limit: PAGE_LIMIT, history_after_epoch: history, legacy_after_epoch: legacy.epoch,
        ...(legacy.key_id ? { legacy_after_key_id: legacy.key_id } : {}) };
      const value = await op.wait(this.api.getDirectory(channelId, page));
      this.observe(channelId, value);
      if (directory && directory.revision !== value.revision) throw new SnapshotChanged();
      directory ??= value;
      history = nextEpoch(history, value.historical_epochs, value.next_history_after_epoch);
      legacy = nextLegacy(legacy, value.legacy_candidates, value.next_legacy_cursor);
      historical.push(...value.historical_epochs);
      candidates.push(...value.legacy_candidates);
      if (value.next_history_after_epoch === null && value.next_legacy_cursor === null) {
        return { ...directory, historical_epochs: historical, legacy_candidates: candidates };
      }
    }
    throw new E2eError('Device history exceeds the synchronization limit');
  }

  private sealRecipients(channelId: string, key: RetainedChannelKey, recipients: MemberDeviceKey[]) {
    return recipients.map(recipient => ({
      recipient_pseudonym_id: recipient.pseudonym_id, recipient_device_id: recipient.device_id,
      wrapped_key_b64: sealChannelKeyV2(key.cek, decodeHex32(recipient.x25519_pub_hex), {
        authorityServer: key.authorityServer, channelId, epoch: key.epoch, keyId: key.keyId,
        recipientPseudonym: recipient.pseudonym_id, recipientDeviceId: recipient.device_id,
      }, key.purpose),
    }));
  }

  private async distribute(
    channelId: string, context: ChannelContext, device: LocalEncryptionDevice,
    operation: ChannelWrapCommit['operation'], revision: number,
    key: RetainedChannelKey, recipients: MemberDeviceKey[], op: Operation,
  ): Promise<number> {
    if (!recipients.length) return revision;
    const response = await op.wait(this.api.postWraps(channelId, {
      version: 2, operation, expected_revision: revision, epoch: key.epoch, key_id: key.keyId,
      sender_device_id: device.deviceId, wraps: this.sealRecipients(channelId, key, recipients),
    }));
    this.observe(channelId, response.state);
    return context.state.revision;
  }

  private async synchronize(channelId: string): Promise<ResolvedChannelKey | null> {
    const existing = this.synchronization.get(channelId);
    if (existing) return existing;
    const op = this.operation();
    const pending = this.synchronizeNow(channelId, op).finally(() => {
      if (this.synchronization.get(channelId) === pending) this.synchronization.delete(channelId);
    });
    this.synchronization.set(channelId, pending);
    return pending;
  }

  private async synchronizeNow(channelId: string, op: Operation): Promise<ResolvedChannelKey | null> {
    for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
      try {
        const context = await this.readState(channelId, op);
        await this.loadKeys(channelId, context, op);
        const device = await this.enroll(context, op);
        await this.readOwnWraps(channelId, context, device, op);
        let directory = await this.directory(channelId, op);
        if (!directory.member_keys.some(member => member.pseudonym_id === this.pseudonym && member.device_id === device.deviceId)) {
          throw new E2eError('This device is absent from the authorized channel membership');
        }
        if (directory.e2e_enabled && directory.rotation_required) {
          const cek = generateChannelKey();
          try {
            await this.distribute(channelId, context, device, 'commit_epoch', directory.revision, {
              authorityServer: directory.authority_identity, epoch: directory.required_epoch,
              keyId: channelKeyId(cek), cek, purpose: 'current',
            }, directory.member_keys, op);
          } finally { cek.fill(0); }
          // The commit response is insufficient: use only the authenticated
          // wrap that the server actually retained for this device.
          await this.readOwnWraps(channelId, context, device, op);
          directory = await this.directory(channelId, op);
        }
        let revision = directory.revision;
        const currentReference = directory.current_key_id === null ? null : keyReference({
          purpose: 'current', epoch: directory.current_epoch, keyId: directory.current_key_id,
        });
        const current = currentReference && context.ownedCurrent.has(currentReference)
          ? context.keys.get(currentReference) : undefined;
        if (current && !directory.rotation_required) {
          revision = await this.distribute(channelId, context, device, 'admit_devices', revision,
            current, directory.missing_current_devices, op);
        }
        for (const history of directory.historical_epochs) {
          if (history.epoch === directory.current_epoch && !directory.rotation_required) continue;
          const key = context.keys.get(keyReference({ purpose: 'current', epoch: history.epoch, keyId: history.key_id }));
          if (key && key.authorityServer !== history.authority_namespace) throw new E2eError('Historical key namespace changed');
          if (key && context.ownedCurrent.has(keyReference(key))) revision = await this.distribute(channelId, context, device, 'admit_history_devices', revision,
            key, history.missing_devices, op);
        }
        for (const key of context.keys.values()) {
          if (key.purpose !== 'legacy' || !key.verifiedLegacy) continue;
          if (!context.ownedLegacy.has(keyReference(key)) && !context.ownedRawLegacyEpochs.has(key.epoch)) continue;
          const known = directory.legacy_candidates.find(candidate => candidate.epoch === key.epoch && candidate.key_id === key.keyId);
          if (known && known.authority_namespace !== key.authorityServer) throw new E2eError('Legacy key namespace changed');
          revision = await this.distribute(channelId, context, device, 'admit_legacy_devices', revision,
            key, known ? known.missing_devices : directory.member_keys, op);
        }
        const final = await this.readState(channelId, op);
        if (final.state.current_epoch !== directory.current_epoch || final.state.required_epoch !== directory.required_epoch
          || final.state.current_key_id !== directory.current_key_id || final.state.e2e_enabled !== directory.e2e_enabled) {
          throw new SnapshotChanged();
        }
        if (!current || final.state.rotation_required) return null;
        return { ...current, purpose: 'current', serverIdentity: final.state.authority_identity, channelId };
      } catch (error) {
        op.check();
        if (!isConflict(error) || attempt === MAX_ATTEMPTS - 1) throw error;
      }
    }
    throw new E2eError('Channel key synchronization did not converge');
  }

  async resolveChannelKey(channelId: string): Promise<ResolvedChannelKey> {
    const key = await this.synchronize(channelId);
    if (!this.channels.get(channelId)?.state.e2e_enabled) throw new E2eError('Channel encryption is disabled');
    if (!key) throw new E2eKeyPendingError(channelId);
    return key;
  }

  /** Refresh membership and seal only missing tuples in the captured snapshot. */
  async reconcile(channelId: string): Promise<void> { await this.synchronize(channelId); }

  async encrypt(channelId: string, plaintext: string): Promise<string> {
    const op = this.operation();
    const key = await op.wait(this.resolveChannelKey(channelId));
    return encryptMessageV2(plaintext, key.cek, key);
  }

  /** Retained file keys must never become plaintext when a channel is disabled. */
  async encryptEdit(channelId: string, plaintext: string, previousWire: string): Promise<string> {
    const op = this.operation();
    const context = await this.readState(channelId, op);
    if (context.state.e2e_enabled) return this.encrypt(channelId, plaintext);
    await op.wait(this.decrypt(channelId, previousWire));
    const previous = parseMessageV2(previousWire);
    const key = context.keys.get(keyReference({ purpose: 'current', epoch: previous.epoch, keyId: previous.keyId }));
    if (!key) throw new E2eKeyPendingError(channelId);
    return encryptMessageV2(plaintext, key.cek, { ...key, channelId });
  }

  async decrypt(channelId: string, body: string): Promise<string> {
    const op = this.operation();
    const context = this.channels.get(channelId) ?? await this.readState(channelId, op);
    await this.loadKeys(channelId, context, op);
    if (body.startsWith('e2e2:')) {
      const parsed = parseMessageV2(body);
      const reference = keyReference({ purpose: 'current', epoch: parsed.epoch, keyId: parsed.keyId });
      if (!context.keys.has(reference)) await op.wait(this.synchronize(channelId));
      const key = context.keys.get(reference);
      if (!key) throw new E2eKeyPendingError(channelId);
      return decryptMessageV2(body, key.cek, { ...key, channelId });
    }
    if (!body.startsWith('e2e1:')) throw new E2eError('Not an encrypted message');
    const ciphertext = decodeCanonicalBase64(body.slice(5));
    const tryLegacy = async (): Promise<string | null> => {
      for (const candidate of context.keys.values()) {
        if (candidate.purpose !== 'legacy') continue;
        let plaintext: Uint8Array;
        try { plaintext = decryptContent(candidate.cek, ciphertext, utf8.encode(channelId)); }
        catch { continue; }
        const text = decodeUtf8Strict(plaintext);
        if (!candidate.verifiedLegacy) {
          await this.retain(channelId, context, { ...candidate, verifiedLegacy: true }, op);
        }
        return text;
      }
      if (context.unscopedLegacy) {
        let plaintext: Uint8Array;
        try { plaintext = decryptContent(context.unscopedLegacy.cek, ciphertext, utf8.encode(channelId)); }
        catch { return null; }
        const text = decodeUtf8Strict(plaintext);
        // A legacy local key can authenticate history, but its absent origin
        // namespace must never be guessed from today's routing URL or sealed
        // to another device until an original server wrap establishes it.
        context.unscopedLegacy.verified = true;
        return text;
      }
      return null;
    };
    const cached = await tryLegacy();
    if (cached !== null) return cached;
    await op.wait(this.synchronize(channelId));
    const fetched = await tryLegacy();
    if (fetched === null) throw new E2eKeyPendingError(channelId);
    return fetched;
  }

  /** Durable historical material is retained; in-flight work cannot resurrect it in memory. */
  forget(): void {
    this.generation++;
    if (this.device) this.device.secret.fill(0);
    for (const context of this.channels.values()) {
      for (const key of context.keys.values()) key.cek.fill(0);
      context.unscopedLegacy?.cek.fill(0);
    }
    this.device = null;
    this.devicePending = null;
    this.publicationPending = null;
    this.published = false;
    this.serverIdentity = null;
    this.channels.clear();
    this.synchronization.clear();
  }
}
