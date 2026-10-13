import { describe, expect, it, vi } from 'vitest';
import { E2eChannelManagerV2 } from './e2e-manager-v2';
import { E2eKeyPendingError } from './e2e-channel';
import { encryptContent, publicKeyFromSecret, sealTo, toBase64, utf8 } from './e2e';
import { channelKeyId, deviceIdForPublicKey, openChannelKeyV2 } from './e2e-v2';
import type {
  ChannelKeyState, ChannelWrapCommit, DeviceKeyWrap, E2eApiV2, E2eKeyringV2,
  LegacyKeyWrap, LocalEncryptionDevice, MemberDeviceKey, RetainedChannelKey,
} from './e2e-v2-protocol';

const SERVER = `annex-server:${'a'.repeat(64)}`;
const AUTHORITY = SERVER;
const CHANNEL = 'channel';
const failure = (status: number) => Object.assign(new Error(`HTTP ${status}`), { status });
const tuple = (pid: string, device: string) => JSON.stringify([pid, device]);
const reference = (epoch: number, key: string) => `${epoch}:${key}`;

/** Contract simulator for client races; native server and browser tests remain independent gates. */
class Protocol {
  members = new Set(['alice', 'bob']);
  devices = new Map<string, MemberDeviceKey>();
  epochs = new Map<number, string>();
  wraps = new Map<string, Map<string, DeviceKeyWrap>>();
  legacyWraps = new Map<string, Map<string, DeviceKeyWrap>>();
  rawLegacy = new Map<string, LegacyKeyWrap[]>();
  state: ChannelKeyState = {
    version: 2, server_identity: SERVER, authority_identity: SERVER, authority_server: AUTHORITY, revision: 0,
    current_epoch: 0, required_epoch: 1, current_key_id: null, history_policy: 'all_members',
    e2e_enabled: true, has_key: false, rotation_required: true, legacy_history_available: false, max_epoch: 0,
  };
  pageSize = 200;
  commits: ChannelWrapCommit[] = [];
  private missing(epoch: number, key: string, legacy = false) {
    const stored = (legacy ? this.legacyWraps : this.wraps).get(reference(epoch, key));
    return this.eligible().filter(member => !stored?.has(tuple(member.pseudonym_id, member.device_id)));
  }
  eligible() {
    const stored = this.wraps.get(reference(this.state.current_epoch, this.state.current_key_id ?? ''));
    return [...this.devices.values()].filter(member => this.members.has(member.pseudonym_id))
      .map(member => ({ ...member, has_current_wrap: !!stored?.has(tuple(member.pseudonym_id, member.device_id)) }));
  }
  leave(pid: string) {
    this.members.delete(pid); this.state.revision++;
    this.state.required_epoch = Math.max(this.state.required_epoch, this.state.current_epoch + 1);
    this.state.rotation_required = true;
  }
  join(pid: string) { this.members.add(pid); this.state.revision++; }
  api(pid: string): E2eApiV2 {
    const member = () => { if (!this.members.has(pid)) throw failure(403); };
    const owned = (id: string) => { member(); if (!this.devices.has(tuple(pid, id))) throw failure(403); };
    return {
      assertCurrent() {},
      getState: async () => { member(); return { ...this.state }; },
      publishDevice: async publicKey => {
        const id = deviceIdForPublicKey(Uint8Array.from(publicKey.match(/../g)!, byte => Number.parseInt(byte, 16)));
        const entry = tuple(pid, id), created = !this.devices.has(entry);
        if (created) {
          this.devices.set(entry, { pseudonym_id: pid, device_id: id, x25519_pub_hex: publicKey,
            device_certificate: null, certificate_sha256: '0'.repeat(64), has_current_wrap: false });
          this.state.revision++;
        }
        return { device_id: id, created, affected_channels: [] };
      },
      getDevices: async target => ({ pseudonym_id: target, devices: [...this.devices.values()].filter(value => value.pseudonym_id === target) }),
      revokeDevice: async id => { owned(id); this.devices.delete(tuple(pid, id)); this.state.revision++;
        this.state.required_epoch = this.state.current_epoch + 1; this.state.rotation_required = true;
        return { device_id: id, revoked: true, affected_channels: [] }; },
      getDirectory: async (_channel, page = {}) => {
        member();
        const size = Math.min(this.pageSize, page.limit ?? 50);
        const history = [...this.epochs].filter(([epoch]) => epoch > (page.history_after_epoch ?? 0))
          .map(([epoch, key_id]) => ({ epoch, key_id, authority_namespace: AUTHORITY, missing_devices: this.missing(epoch, key_id) })).sort((a, b) => a.epoch - b.epoch);
        const legacy = [...this.legacyWraps.keys()].map(value => { const [epoch, key_id] = value.split(':');
          return { epoch: Number(epoch), key_id, authority_namespace: AUTHORITY, missing_devices: this.missing(Number(epoch), key_id, true) }; })
          .filter(value => value.epoch > (page.legacy_after_epoch ?? 0)
            || (value.epoch === page.legacy_after_epoch && value.key_id > (page.legacy_after_key_id ?? '')))
          .sort((a, b) => a.epoch - b.epoch || a.key_id.localeCompare(b.key_id));
        // This simulator tests key races below the independently tested signed
        // API boundary; it does not pretend these synthetic members are certified.
        return { ...this.state, manifest: { head_seq: 0, head_hash: '', genesis_hash: null, join_policy: 'eligible_self_join',
          history_policy: 'all_members', e2e_enabled: true, members: [], revoked_devices: [] },
          member_keys: this.eligible(), missing_current_devices: this.eligible().filter(value => !value.has_current_wrap),
          historical_epochs: history.slice(0, size), next_history_after_epoch: history.length > size ? history[size - 1].epoch : null,
          legacy_candidates: legacy.slice(0, size), next_legacy_cursor: legacy.length > size ? { epoch: legacy[size - 1].epoch, key_id: legacy[size - 1].key_id } : null };
      },
      getWraps: async (_channel, device, page = {}) => {
        owned(device);
        const size = Math.min(this.pageSize, page.limit ?? 50);
        const all = (table: Map<string, Map<string, DeviceKeyWrap>>) => [...table.values()]
          .map(value => value.get(tuple(pid, device))).filter((value): value is DeviceKeyWrap => !!value)
          .sort((a, b) => a.epoch - b.epoch || a.key_id.localeCompare(b.key_id));
        const wraps = all(this.wraps).filter(value => value.epoch > (page.after_epoch ?? 0));
        const legacy = all(this.legacyWraps).filter(value => value.epoch > (page.legacy_after_epoch ?? 0)
          || (value.epoch === page.legacy_after_epoch && value.key_id > (page.legacy_after_key_id ?? '')));
        const raw = (this.rawLegacy.get(pid) ?? []).filter(value => value.epoch > (page.legacy_wrap_after_epoch ?? 0));
        return { state: { ...this.state }, wraps: wraps.slice(0, size), next_after_epoch: wraps.length > size ? wraps[size - 1].epoch : null,
          legacy_device_wraps: legacy.slice(0, size), next_legacy_cursor: legacy.length > size ? { epoch: legacy[size - 1].epoch, key_id: legacy[size - 1].key_id } : null,
          legacy_wraps: raw.slice(0, size), next_legacy_wrap_after_epoch: raw.length > size ? raw[size - 1].epoch : null };
      },
      postWraps: async (_channel, request) => {
        owned(request.sender_device_id);
        if (request.expected_revision !== this.state.revision) throw failure(409);
        this.commits.push(request);
        const legacy = request.operation === 'admit_legacy_devices';
        if (request.operation === 'commit_epoch') {
          if (!this.state.e2e_enabled || request.epoch !== this.state.required_epoch || !this.state.rotation_required) throw failure(409);
          expect(new Set(request.wraps.map(wrap => tuple(wrap.recipient_pseudonym_id, wrap.recipient_device_id))))
            .toEqual(new Set(this.eligible().map(value => tuple(value.pseudonym_id, value.device_id))));
          this.epochs.set(request.epoch, request.key_id);
        } else if (!legacy && !this.wraps.get(reference(request.epoch, request.key_id))?.has(tuple(pid, request.sender_device_id))) {
          throw failure(403);
        }
        const table = legacy ? this.legacyWraps : this.wraps;
        const ref = reference(request.epoch, request.key_id);
        const stored = table.get(ref) ?? new Map<string, DeviceKeyWrap>();
        for (const wrap of request.wraps) {
          const existing = stored.get(tuple(wrap.recipient_pseudonym_id, wrap.recipient_device_id));
          if (existing && existing.wrapped_key_b64 !== wrap.wrapped_key_b64) throw failure(409);
        }
        let inserted = 0;
        for (const wrap of request.wraps) {
          const target = tuple(wrap.recipient_pseudonym_id, wrap.recipient_device_id);
          if (stored.has(target)) continue;
          stored.set(target, { epoch: request.epoch, key_id: request.key_id, authority_namespace: AUTHORITY, authorization: null,
            sender_pseudonym_id: pid, sender_device_id: request.sender_device_id,
            recipient_device_id: wrap.recipient_device_id, wrapped_key_b64: wrap.wrapped_key_b64 });
          inserted++;
        }
        table.set(ref, stored);
        if (request.operation === 'commit_epoch') Object.assign(this.state, {
          current_epoch: request.epoch, current_key_id: request.key_id, max_epoch: request.epoch, has_key: true, rotation_required: false,
        });
        if (inserted) this.state.revision++;
        return { status: 'ok', inserted, state: { ...this.state } };
      },
    };
  }
}

function local(byte: number) {
  const secrets: Uint8Array[] = [new Uint8Array(32).fill(byte)];
  const keys = new Map<string, RetainedChannelKey>();
  const device = (): LocalEncryptionDevice => {
    const secret = secrets[secrets.length - 1].slice(), publicKey = publicKeyFromSecret(secret);
    return { secret, publicKey, deviceId: deviceIdForPublicKey(publicKey) };
  };
  const store: E2eKeyringV2 = {
    async getOrCreateDevice() { return device(); },
    async replaceDevice() { secrets.push(new Uint8Array(32).fill(byte + 1)); return device(); },
    async getLegacyDeviceSecrets() { return secrets.map(secret => secret.slice()); },
    async getKeys() { return [...keys.values()].map(key => ({ ...key, cek: key.cek.slice() })); },
    async putKey(_server, _pid, _channel, key) { keys.set(`${key.purpose}:${key.epoch}:${key.keyId}`, { ...key, cek: key.cek.slice() }); },
    async getUnscopedLegacyKey() { return null; },
  };
  return { store, keys, device };
}
function participant(server: Protocol, pid: string, byte: number) {
  const storage = local(byte), api = server.api(pid);
  return { ...storage, api, manager: new E2eChannelManagerV2(pid, api, storage.store) };
}

describe('device/epoch encryption orchestration', () => {
  it('concurrent provisioners converge on the single retained key and read one another’s ciphertext', async () => {
    const protocol = new Protocol();
    const alice = participant(protocol, 'alice', 1), bob = participant(protocol, 'bob', 2);
    await Promise.all([alice.manager.ensureDevicePublished(CHANNEL), bob.manager.ensureDevicePublished(CHANNEL)]);
    const [a, b] = await Promise.all([alice.manager.resolveChannelKey(CHANNEL), bob.manager.resolveChannelKey(CHANNEL)]);
    expect(a.cek).toEqual(b.cek);
    expect(protocol.epochs.size).toBe(1);
    const wire = await alice.manager.encrypt(CHANNEL, 'private text');
    expect(wire).toMatch(/^e2e2:1:/);
    expect(wire).not.toContain('private text');
    expect(await bob.manager.decrypt(CHANNEL, wire)).toBe('private text');
  });

  it('admits only missing device tuples and never reseals occupied tuples', async () => {
    const protocol = new Protocol();
    const alice = participant(protocol, 'alice', 1);
    await alice.manager.resolveChannelKey(CHANNEL);
    const bob = participant(protocol, 'bob', 2);
    await expect(bob.manager.resolveChannelKey(CHANNEL)).rejects.toBeInstanceOf(E2eKeyPendingError);
    await alice.manager.reconcile(CHANNEL);
    const admission = protocol.commits.find(value => value.operation === 'admit_devices')!;
    expect(admission.wraps.map(value => value.recipient_pseudonym_id)).toEqual(['bob']);
    expect((await bob.manager.resolveChannelKey(CHANNEL)).keyId).toBe(protocol.state.current_key_id);
    const count = protocol.commits.length;
    await alice.manager.reconcile(CHANNEL);
    expect(protocol.commits).toHaveLength(count);
  });

  it('does not use a generated candidate when the committed own wrap is missing', async () => {
    const protocol = new Protocol();
    const alice = participant(protocol, 'alice', 1);
    vi.spyOn(alice.api, 'postWraps').mockImplementation(async (_id, request) => {
      Object.assign(protocol.state, { revision: protocol.state.revision + 1, current_epoch: request.epoch,
        current_key_id: request.key_id, has_key: true, max_epoch: request.epoch, rotation_required: false });
      return { status: 'ok', inserted: request.wraps.length, state: { ...protocol.state } };
    });
    await expect(alice.manager.encrypt(CHANNEL, 'must not send')).rejects.toBeInstanceOf(E2eKeyPendingError);
    expect(alice.keys.size).toBe(0);
  });

  it('rotates after departure, preserves old history, and admits a rejoined identity to both epochs', async () => {
    const protocol = new Protocol();
    const alice = participant(protocol, 'alice', 1), bob = participant(protocol, 'bob', 2);
    await bob.manager.ensureDevicePublished(CHANNEL);
    const old = await alice.manager.encrypt(CHANNEL, 'old history');
    expect(await bob.manager.decrypt(CHANNEL, old)).toBe('old history');
    protocol.leave('bob');
    const next = await alice.manager.encrypt(CHANNEL, 'after departure');
    expect(next).toMatch(/^e2e2:2:/);
    expect(await bob.manager.decrypt(CHANNEL, old)).toBe('old history');
    await expect(bob.manager.decrypt(CHANNEL, next)).rejects.toMatchObject({ status: 403 });
    protocol.join('bob');
    await alice.manager.reconcile(CHANNEL);
    expect(await bob.manager.decrypt(CHANNEL, next)).toBe('after departure');
    expect(await bob.manager.decrypt(CHANNEL, old)).toBe('old history');
  });

  it('gives a linked device every authorized historical epoch across multiple pages', async () => {
    const protocol = new Protocol(); protocol.pageSize = 2;
    const alice = participant(protocol, 'alice', 1);
    const history: string[] = [];
    for (let i = 1; i <= 5; i++) {
      if (i > 1) { protocol.join('departing'); protocol.leave('departing'); }
      history.push(await alice.manager.encrypt(CHANNEL, `epoch ${i}`));
    }
    const linked = participant(protocol, 'alice', 3);
    await expect(linked.manager.resolveChannelKey(CHANNEL)).rejects.toBeInstanceOf(E2eKeyPendingError);
    await alice.manager.reconcile(CHANNEL);
    for (let i = 0; i < history.length; i++) expect(await linked.manager.decrypt(CHANNEL, history[i])).toBe(`epoch ${i + 1}`);
    expect(new Set([...linked.keys.values()].map(value => value.epoch))).toEqual(new Set([1, 2, 3, 4, 5]));
  });

  it('re-reads membership on a CAS conflict before distributing a rotation', async () => {
    const protocol = new Protocol();
    const alice = participant(protocol, 'alice', 1), bob = participant(protocol, 'bob', 2);
    await bob.manager.ensureDevicePublished(CHANNEL);
    const original = alice.api.postWraps;
    vi.spyOn(alice.api, 'postWraps').mockImplementation(original)
      .mockImplementationOnce(async () => { protocol.leave('bob'); throw failure(409); });
    const key = await alice.manager.resolveChannelKey(CHANNEL);
    expect(key.keyId).toBe(protocol.state.current_key_id);
    expect(protocol.commits[0].wraps.map(value => value.recipient_pseudonym_id)).toEqual(['alice']);
  });

  it('refuses a context-tampered own wrap without retaining its key', async () => {
    const protocol = new Protocol();
    const alice = participant(protocol, 'alice', 1);
    await alice.manager.resolveChannelKey(CHANNEL);
    const bob = participant(protocol, 'bob', 2);
    await bob.manager.ensureDevicePublished(CHANNEL);
    await alice.manager.reconcile(CHANNEL);
    const target = protocol.wraps.get(reference(1, protocol.state.current_key_id!))!;
    const wrap = target.get(tuple('bob', bob.device().deviceId))!;
    target.set(tuple('bob', bob.device().deviceId), { ...wrap, key_id: 'f'.repeat(64) });
    await expect(bob.manager.resolveChannelKey(CHANNEL)).rejects.toThrow('fingerprint');
    expect(bob.keys.size).toBe(0);
  });

  it('refuses stale encryption even when the old key remains cached', async () => {
    const protocol = new Protocol(); const alice = participant(protocol, 'alice', 1);
    await alice.manager.resolveChannelKey(CHANNEL);
    protocol.state.e2e_enabled = false; protocol.state.revision++;
    await expect(alice.manager.encrypt(CHANNEL, 'not silently encrypted')).rejects.toThrow('disabled');
    expect(protocol.epochs.size).toBe(1);
  });

  it('bridges legacy keys only after authenticating a legacy message and uses the separate legacy domain', async () => {
    const protocol = new Protocol(); const alice = participant(protocol, 'alice', 1);
    const cek = new Uint8Array(32).fill(17);
    protocol.rawLegacy.set('alice', [{ epoch: 7, key_epoch: 7, authority_namespace: AUTHORITY, sender_pseudonym_id: 'alice',
      wrapped_key_b64: toBase64(sealTo(cek, alice.device().publicKey)) }]);
    protocol.state.legacy_history_available = true;
    await alice.manager.resolveChannelKey(CHANNEL);
    expect(protocol.legacyWraps.size).toBe(0);
    const old = `e2e1:${toBase64(encryptContent(cek, utf8.encode('legacy history'), utf8.encode(CHANNEL)))}`;
    expect(await alice.manager.decrypt(CHANNEL, old)).toBe('legacy history');
    await alice.manager.reconcile(CHANNEL);
    const bridge = protocol.commits.find(value => value.operation === 'admit_legacy_devices')!;
    const wrap = bridge.wraps.find(value => value.recipient_pseudonym_id === 'alice')!;
    const context = { authorityServer: AUTHORITY, channelId: CHANNEL, epoch: 7, keyId: channelKeyId(cek),
      recipientPseudonym: 'alice', recipientDeviceId: alice.device().deviceId };
    expect(openChannelKeyV2(wrap.wrapped_key_b64, alice.device().secret, context, 'legacy')).toEqual(cek);
    expect(() => openChannelKeyV2(wrap.wrapped_key_b64, alice.device().secret, context)).toThrow('context');
    expect((await alice.manager.encrypt(CHANNEL, 'new message')).startsWith('e2e2:1:')).toBe(true);
  });

  it('retains keys across manager recreation without republishing merely to read local history', async () => {
    const protocol = new Protocol(); const alice = participant(protocol, 'alice', 1);
    const wire = await alice.manager.encrypt(CHANNEL, 'saved');
    alice.manager.forget();
    const restored = new E2eChannelManagerV2('alice', alice.api, alice.store);
    const publish = vi.spyOn(alice.api, 'publishDevice').mockRejectedValue(failure(403));
    expect(await restored.decrypt(CHANNEL, wire)).toBe('saved');
    expect(publish).not.toHaveBeenCalled();
  });

  it('cancels delayed resolution after logout instead of resurrecting the device or channel cache', async () => {
    const protocol = new Protocol(); const alice = participant(protocol, 'alice', 1);
    let release!: (state: ChannelKeyState) => void;
    vi.spyOn(alice.api, 'getState').mockImplementationOnce(() => new Promise(resolve => { release = resolve; }));
    const pending = alice.manager.resolveChannelKey(CHANNEL);
    alice.manager.forget();
    release({ ...protocol.state });
    await expect(pending).rejects.toThrow('cancelled');
    expect(protocol.devices.size).toBe(0);
    expect(alice.keys.size).toBe(0);
  });
});
