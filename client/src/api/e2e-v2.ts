import { authHeaders, getCredentialContext, isCredentialContextCurrent, request, StaleCredentialContextError } from './core';
import { E2eError, toHex } from '@/lib/e2e';
import { decodeCanonicalBase64, decodeHex32, deviceIdForPublicKey, requireSafeInteger } from '@/lib/e2e-v2';
import { createDeviceCertificate, parseCertificateContext, parseDeviceCertificate, verifyDeviceCertificate,
  type CertificateContext, type DeviceCertificate } from '@/lib/device-certificate';
import { getDeviceSigningMaterial, getStoredDeviceCertificate, indexedDbKeyringV2,
  pinEncryptionServer, retainDeviceCertificate, getChannelManifestPin, retainChannelManifestPin, assertPinnedEncryptionServer } from '@/lib/e2e-keyring-v2';
import { checkManifestRecipient, manifestProjection, parseManifestEvent, parseManifestPage, parseManifestProjection,
  parseWrapAuthorization, signWrapAuthorization, verifyWrapAuthorization, type JoinPolicy } from '@/lib/e2e-manifest';
import { ChannelManifestSession, type ManifestHistory } from '@/lib/e2e-manifest-session';
import { observeVerifiedChannel } from '@/lib/verified-channel';
import type {
  AffectedChannelState, ChannelKeyDirectory, ChannelKeyState, DeviceKeyWrap, DevicePublicKey,
  E2eApiV2, KeyPage, LegacyCursor, LegacyKeyWrap, MemberDeviceKey, MissingEpochDevices, OwnChannelWraps,
} from '@/lib/e2e-v2-protocol';

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new E2eError('Invalid key-service response');
  return value as Record<string, unknown>;
}
function string(value: unknown, maximum = 4096): string {
  if (typeof value !== 'string' || !value.length || value.length > maximum) throw new E2eError('Invalid key-service text');
  return value;
}
function flag(value: unknown): boolean {
  if (typeof value !== 'boolean') throw new E2eError('Invalid key-service flag');
  return value;
}
function integer(value: unknown, minimum = 0): number {
  if (typeof value !== 'number') throw new E2eError('Invalid key-service counter');
  return requireSafeInteger(value, minimum);
}
function hash(value: unknown): string {
  const hex = string(value, 64);
  decodeHex32(hex);
  return hex;
}
function stableIdentity(value: unknown): string {
  const result = string(value, 77);
  if (!/^annex-server:[0-9a-f]{64}$/.test(result)) throw new E2eError('Invalid stable server identity');
  return result;
}
function array<T>(value: unknown, parse: (item: unknown) => T, maximum: number): T[] {
  if (!Array.isArray(value) || value.length > maximum) throw new E2eError('Invalid key-service collection');
  return value.map(parse);
}
function cursor(value: unknown): number | null { return value === null ? null : integer(value, 1); }
function legacyCursor(value: unknown): LegacyCursor | null {
  if (value === null) return null;
  const item = record(value);
  return { epoch: integer(item.epoch, 1), key_id: hash(item.key_id) };
}

export function parseChannelKeyState(value: unknown): ChannelKeyState {
  const item = record(value);
  if (item.version !== 2 || item.history_policy !== 'all_members') throw new E2eError('Unsupported channel key protocol or history policy');
  const identity = stableIdentity(item.server_identity);
  const state: ChannelKeyState = {
    version: 2, server_identity: identity, authority_identity: stableIdentity(item.authority_identity), authority_server: string(item.authority_server, 2048),
    revision: integer(item.revision), current_epoch: integer(item.current_epoch),
    required_epoch: integer(item.required_epoch, 1), current_key_id: item.current_key_id === null ? null : hash(item.current_key_id),
    history_policy: 'all_members', e2e_enabled: flag(item.e2e_enabled), has_key: flag(item.has_key),
    rotation_required: flag(item.rotation_required), legacy_history_available: flag(item.legacy_history_available),
    max_epoch: integer(item.max_epoch),
  };
  if (state.required_epoch < state.current_epoch || state.has_key !== (state.current_epoch > 0)
    || state.rotation_required !== (state.required_epoch > state.current_epoch)
    || (state.current_key_id === null) !== (state.current_epoch === 0) || state.max_epoch !== state.current_epoch) {
    throw new E2eError('Inconsistent channel key state');
  }
  return state;
}

function device(value: unknown): DevicePublicKey {
  const item = record(value);
  const pub = hash(item.x25519_pub_hex);
  const id = hash(item.device_id);
  if (deviceIdForPublicKey(decodeHex32(pub)) !== id) {
    throw new E2eError('Unrecognized or inconsistent encryption device');
  }
  return { device_id: id, x25519_pub_hex: pub,
    device_certificate: item.device_certificate === null ? null : parseDeviceCertificate(item.device_certificate) };
}
function memberDevice(value: unknown): MemberDeviceKey {
  const item = record(value);
  return { ...device(item), pseudonym_id: string(item.pseudonym_id, 512), certificate_sha256: hash(item.certificate_sha256), has_current_wrap: flag(item.has_current_wrap) };
}
function missingEpoch(value: unknown): MissingEpochDevices {
  const item = record(value);
  return { epoch: integer(item.epoch, 1), key_id: hash(item.key_id), authority_namespace: string(item.authority_namespace, 2048), missing_devices: array(item.missing_devices, memberDevice, 10000) };
}
function sealed(value: unknown): string {
  const text = string(value, 4096);
  if (decodeCanonicalBase64(text, 3072).length < 60) throw new E2eError('Truncated sealed key');
  return text;
}
function wrap(value: unknown): DeviceKeyWrap {
  const item = record(value);
  return { epoch: integer(item.epoch, 1), key_id: hash(item.key_id), recipient_device_id: hash(item.recipient_device_id),
    authority_namespace: string(item.authority_namespace, 2048), authorization: item.authorization === null ? null : parseWrapAuthorization(item.authorization),
    sender_pseudonym_id: string(item.sender_pseudonym_id, 512), sender_device_id: hash(item.sender_device_id), wrapped_key_b64: sealed(item.wrapped_key_b64) };
}
function legacyWrap(value: unknown): LegacyKeyWrap {
  const item = record(value);
  const epoch = integer(item.epoch, 1);
  if (item.key_epoch !== epoch) throw new E2eError('Inconsistent legacy key epoch');
  return { epoch, key_epoch: epoch, authority_namespace: string(item.authority_namespace, 2048), sender_pseudonym_id: string(item.sender_pseudonym_id, 512), wrapped_key_b64: sealed(item.wrapped_key_b64) };
}
function affected(value: unknown): AffectedChannelState {
  const item = record(value);
  return { ...parseChannelKeyState(item), channel_id: string(item.channel_id, 256) };
}

export function parseChannelKeyDirectory(value: unknown): ChannelKeyDirectory {
  const item = record(value);
  const directory: ChannelKeyDirectory = {
    ...parseChannelKeyState(item), manifest: parseManifestProjection(item.manifest), member_keys: array(item.member_keys, memberDevice, 10000),
    missing_current_devices: array(item.missing_current_devices, memberDevice, 10000),
    historical_epochs: array(item.historical_epochs, missingEpoch, 200),
    next_history_after_epoch: cursor(item.next_history_after_epoch),
    legacy_candidates: array(item.legacy_candidates, missingEpoch, 200), next_legacy_cursor: legacyCursor(item.next_legacy_cursor),
  };
  const members = new Map<string, MemberDeviceKey>();
  for (const member of directory.member_keys) {
    const id = JSON.stringify([member.pseudonym_id, member.device_id]);
    if (members.has(id)) throw new E2eError('Duplicate member device');
    members.set(id, member);
  }
  const missing = new Set<string>();
  for (const member of directory.missing_current_devices) {
    const id = JSON.stringify([member.pseudonym_id, member.device_id]);
    const original = members.get(id);
    if (!original || original.has_current_wrap || member.has_current_wrap
      || original.x25519_pub_hex !== member.x25519_pub_hex || missing.has(id)) throw new E2eError('Invalid missing-device snapshot');
    missing.add(id);
  }
  if ([...members.entries()].some(([id, member]) => !member.has_current_wrap && !missing.has(id))) {
    throw new E2eError('Incomplete missing-device snapshot');
  }
  return directory;
}

export function parseOwnChannelWraps(value: unknown): OwnChannelWraps {
  const item = record(value);
  return { state: parseChannelKeyState(item.state), wraps: array(item.wraps, wrap, 200), next_after_epoch: cursor(item.next_after_epoch),
    legacy_device_wraps: array(item.legacy_device_wraps, wrap, 200), next_legacy_cursor: legacyCursor(item.next_legacy_cursor),
    legacy_wraps: array(item.legacy_wraps, legacyWrap, 200), next_legacy_wrap_after_epoch: cursor(item.next_legacy_wrap_after_epoch) };
}

function query(page: KeyPage = {}, deviceId?: string): string {
  const params = new URLSearchParams();
  if (deviceId) params.set('device_id', hash(deviceId));
  for (const [key, value] of Object.entries(page)) {
    if (value === undefined) continue;
    if (key === 'legacy_after_key_id') params.set(key, hash(value));
    else {
      const number = integer(value, key === 'limit' ? 1 : 0);
      if (key === 'limit' && number > 200) throw new E2eError('Invalid key page limit');
      params.set(key, String(number));
    }
  }
  return params.size ? `?${params}` : '';
}

export interface E2eControls {
  ensureCertifiedDevice(): Promise<{ device_id: string; server_identity: string }>;
  inspectChannel(channelId: string): Promise<{ state: ChannelKeyState; lineage: ManifestHistory }>;
  trustChannel(channelId: string, authorityIdentity: string, genesisHash: string): Promise<void>;
  setEncryption(channelId: string, enabled: boolean, joinPolicy?: JoinPolicy): Promise<void>;
  recoverChannel(channelId: string): Promise<void>;
  leaveManifest(channelId: string): Promise<void>;
  channelDevices(channelId: string): Promise<{ currentDeviceId: string; devices: (DevicePublicKey & { admitted: boolean; revoked: boolean })[] }>;
  approveChannelDevice(channelId: string, certificate: DeviceCertificate): Promise<void>;
  revokeChannelDevice(channelId: string, deviceId: string): Promise<void>;
  admissionDevices(channelId: string, pseudonymId: string): Promise<DeviceCertificate[]>;
  admitChannelMember(channelId: string, certificate: DeviceCertificate): Promise<void>;
  setAdmissionPolicy(channelId: string, policy: JoinPolicy): Promise<void>;
  setChannelRole(channelId: string, pseudonymId: string, role: 'member' | 'moderator'): Promise<void>;
  removeChannelMember(channelId: string, pseudonymId: string): Promise<void>;
}
export function boundE2eApiV2(pseudonym: string): E2eApiV2 & E2eControls {
  const context = getCredentialContext();
  const assertCurrent = () => {
    if (!isCredentialContextCurrent(context) || context.pseudonymId !== pseudonym) {
      throw new StaleCredentialContextError('Encryption operation');
    }
  };
  async function call<T>(path: string, parse: (value: unknown) => T | Promise<T>, options?: RequestInit): Promise<T> {
    assertCurrent();
    try {
      const response = await request<unknown>(path, { ...options, headers: authHeaders(pseudonym) });
      assertCurrent();
      const parsed = await parse(response);
      assertCurrent();
      return parsed;
    } catch (error) { assertCurrent(); throw error; }
  }
  let certificateContext: Promise<CertificateContext> | null = null;
  const getCertificateContext = async () => {
    assertCurrent();
    certificateContext ??= call('/api/keys/certificate-context', parseCertificateContext).then(async value => {
      await pinEncryptionServer(context.baseUrl, value.server_identity);
      assertCurrent();
      return value;
    }).catch(error => { certificateContext = null; throw error; });
    return certificateContext;
  };
  const certificates = new Map<string, Promise<DeviceCertificate>>();
  const ownCertificate = async (publicKeyHex: string): Promise<DeviceCertificate> => {
    assertCurrent();
    const pending = certificates.get(publicKeyHex);
    if (pending) return pending;
    const work = (async () => {
      const certificateContext = await getCertificateContext();
      const serverIdentity = certificateContext.server_identity;
      const local = await indexedDbKeyringV2().getOrCreateDevice(serverIdentity, pseudonym);
      assertCurrent();
      if (local.deviceId !== deviceIdForPublicKey(decodeHex32(publicKeyHex))) throw new E2eError('Encryption device changed during enrollment');
      let cert = await getStoredDeviceCertificate(serverIdentity, pseudonym, local.deviceId);
      assertCurrent();
      if (!cert) {
        const { listIdentities } = await import('@/lib/db');
        const identities = await listIdentities();
        assertCurrent();
        const identity = identities.find(value => value.pseudonymId === pseudonym && value.commitmentHex === certificateContext.commitment_hex);
        if (!identity) throw new E2eError('This device cannot find the identity secret needed to certify its encryption key');
        const path = await call(`/api/registry/path/${encodeURIComponent(identity.commitmentHex)}`, value => {
          const item = record(value);
          const pathElements = array(item.pathElements, hash, 32);
          const pathIndexBits = array(item.pathIndexBits, bit => {
            if (bit !== 0 && bit !== 1) throw new E2eError('Invalid Merkle path bit');
            return bit;
          }, 32);
          if (pathElements.length !== pathIndexBits.length) throw new E2eError('Invalid Merkle path dimensions');
          return { leafIndex: integer(item.leafIndex), rootHex: hash(item.rootHex), pathElements, pathIndexBits };
        });
        const signing = await getDeviceSigningMaterial(serverIdentity, pseudonym, local.deviceId);
        assertCurrent();
        try {
          cert = await createDeviceCertificate({ context: certificateContext, pseudonym, publicKeyHex,
            signingSecret: signing.secret, rootHex: path.rootHex,
            membership: { sk: BigInt(`0x${identity.sk}`), roleCode: identity.roleCode, nodeId: identity.nodeId,
              leafIndex: path.leafIndex, pathElements: path.pathElements, pathIndexBits: path.pathIndexBits },
            ...(signing.previousCertificate ? { previous: signing.previousCertificate } : {}) });
          assertCurrent();
          cert = await retainDeviceCertificate(cert);
          assertCurrent();
        } finally { signing.secret.fill(0); }
      }
      return verifyDeviceCertificate(cert, { serverIdentity, pseudonym, deviceId: local.deviceId, publicKeyHex });
    })();
    certificates.set(publicKeyHex, work);
    try { const result = await work; assertCurrent(); return result; }
    catch (error) { certificates.delete(publicKeyHex); throw error; }
  };
  const channel = (id: string) => `/api/channels/${encodeURIComponent(id)}`;
  const readState = (id: string) => call(`${channel(id)}/key-status`, parseChannelKeyState);
  const publish = async (publicKeyHex: string) => {
    const certificate = await ownCertificate(publicKeyHex);
    assertCurrent();
    return call('/api/keys/me', value => {
      const item = record(value);
      const id = hash(item.device_id);
      if (id !== deviceIdForPublicKey(decodeHex32(publicKeyHex))) throw new E2eError('Server returned another device ID');
      return { device_id: id, created: flag(item.created), affected_channels: array(item.affected_channels, affected, 10000) };
    }, { method: 'PUT', body: JSON.stringify({ version: 2, x25519_pub_hex: hash(publicKeyHex), device_certificate: certificate }) });
  };
  const signer = async () => {
    const certContext = await getCertificateContext();
    const local = await indexedDbKeyringV2().getOrCreateDevice(certContext.server_identity, pseudonym);
    assertCurrent();
    const certificate = await ownCertificate(toHex(local.publicKey));
    await publish(toHex(local.publicKey));
    const material = await getDeviceSigningMaterial(certContext.server_identity, pseudonym, local.deviceId);
    assertCurrent();
    return { certificate, secret: material.secret };
  };
  const manifests = new ChannelManifestSession(pseudonym, {
    assertCurrent,
    read: (id, after) => call(`${channel(id)}/key-manifest?after_seq=${after}&limit=200`, parseManifestPage),
    write: (id, event) => call(`${channel(id)}/key-manifest`, value => {
      const item = record(value);
      if (item.status !== 'ok' || typeof item.applied !== 'boolean') throw new E2eError('Signed membership change was not confirmed');
      return { event: parseManifestEvent(item.event), manifest: parseManifestProjection(item.manifest) };
    }, { method: 'POST', body: JSON.stringify(event) }),
    signer,
    pin: (authorityIdentity, id) => getChannelManifestPin(authorityIdentity, pseudonym, id),
    retain: async (value, firstUse) => {
      // A channel that was explicitly trusted must not silently change authority
      // even when the replacement serves an otherwise valid independent chain.
      await pinEncryptionServer(JSON.stringify(['channel-authority', context.baseUrl, pseudonym, value.final.channelId]), value.final.authorityIdentity);
      assertCurrent();
      await retainChannelManifestPin(pseudonym, value.history, value.final, firstUse);
      assertCurrent();
    },
  });
  const inspect = async (id: string, state?: ChannelKeyState) => {
    state ??= await readState(id);
    await assertPinnedEncryptionServer(JSON.stringify(['channel-authority', context.baseUrl, pseudonym, id]), state.authority_identity);
    assertCurrent();
    const lineage = await manifests.inspect(id, state.authority_identity);
    if (lineage.final.head && lineage.final.enabled !== state.e2e_enabled) throw new E2eError('The server encryption flag conflicts with the signed channel policy');
    observeVerifiedChannel(pseudonym, lineage);
    return { state, lineage };
  };
  const observe = (lineage: ManifestHistory) => { assertCurrent(); observeVerifiedChannel(pseudonym, lineage); };
  const moderatorLineage = async (id: string) => {
    const { lineage } = await inspect(id);
    manifests.requireTrusted(lineage);
    if (lineage.final.members.get(pseudonym)?.role !== 'moderator') throw new E2eError('This action requires an authorized channel moderator');
    return lineage;
  };
  const certifiedDevices = async (pseudonymId: string) => call(`/api/keys/${encodeURIComponent(pseudonymId)}`, async value => {
    const item = record(value);
    if (item.pseudonym_id !== pseudonymId) throw new E2eError('Server returned another identity directory');
    const devices = array(item.devices, device, 64);
    for (const key of devices) if (key.device_certificate) {
      await verifyDeviceCertificate(key.device_certificate, { serverIdentity: key.device_certificate.server_identity,
        pseudonym: pseudonymId, deviceId: key.device_id, publicKeyHex: key.x25519_pub_hex });
    }
    return { pseudonym_id: pseudonymId, devices };
  });
  const directory = async (id: string, page?: KeyPage) => {
    const value = await call(`${channel(id)}/member-keys${query(page)}`, parseChannelKeyDirectory);
    const { lineage } = await inspect(id, value);
    manifests.requireTrusted(lineage);
    const admitted = await manifests.ensureAdmitted(lineage);
    if (admitted.final.head?.hash !== lineage.final.head?.hash) throw Object.assign(new E2eError('Device admission changed the key directory. Refreshing.'), { status: 409 });
    if (JSON.stringify(value.manifest) !== JSON.stringify(manifestProjection(lineage.final))) throw new E2eError('The server directory conflicts with verified membership history');
    const authorized = [...lineage.final.members].flatMap(([pid, member]) => [...member.devices.keys()].map(id => JSON.stringify([pid, id]))).sort();
    const supplied = value.member_keys.map(key => JSON.stringify([key.pseudonym_id, key.device_id])).sort();
    if (JSON.stringify(authorized) !== JSON.stringify(supplied)) throw new E2eError('The server omitted or added a device outside the signed membership snapshot');
    for (const key of [...value.member_keys, ...value.historical_epochs.flatMap(epoch => epoch.missing_devices), ...value.legacy_candidates.flatMap(epoch => epoch.missing_devices)]) {
      const digest = checkManifestRecipient(lineage.final, key);
      if (key.certificate_sha256 !== digest) throw new E2eError('The server supplied a different device certificate digest');
    }
    return { value, lineage };
  };
  return {
    assertCurrent,
    ensureCertifiedDevice: async () => {
      // Attachment precedes any local replica channel, so this enrollment must
      // use the local registry context rather than a nonexistent key-status URL.
      const own = await signer();
      own.secret.fill(0);
      return { device_id: own.certificate.device_id, server_identity: own.certificate.server_identity };
    },
    inspectChannel: id => inspect(id),
    trustChannel: async (id, authorityIdentity, genesisHash) => {
      const current = await inspect(id);
      if (current.state.authority_identity !== authorityIdentity) throw new E2eError('The channel authority changed while verifying its fingerprint');
      observe(await manifests.trust(id, authorityIdentity, genesisHash));
    },
    setEncryption: async (id, enabled, joinPolicy = 'eligible_self_join') => {
      const { lineage } = await inspect(id);
      if (enabled) observe(await manifests.enable(lineage, joinPolicy));
      else observe(await manifests.apply(lineage, { kind: 'set_encryption', enabled: false }));
    },
    recoverChannel: async id => { const { lineage } = await inspect(id); observe(await manifests.recover(lineage)); },
    leaveManifest: async id => {
      const { lineage } = await inspect(id);
      if (lineage.final.members.has(pseudonym)) observe(await manifests.apply(lineage, { kind: 'leave' }));
    },
    channelDevices: async id => {
      const { lineage } = await inspect(id);
      manifests.requireTrusted(lineage);
      const signing = await signer(); signing.secret.fill(0);
      const devices = await call(`/api/keys/${encodeURIComponent(pseudonym)}`, async value => {
        const item = record(value);
        if (item.pseudonym_id !== pseudonym) throw new E2eError('Another identity was returned');
        const keys = array(item.devices, device, 64);
        for (const key of keys) if (key.device_certificate) await verifyDeviceCertificate(key.device_certificate,
          { serverIdentity: signing.certificate.server_identity, pseudonym, deviceId: key.device_id, publicKeyHex: key.x25519_pub_hex });
        return keys;
      });
      return { currentDeviceId: signing.certificate.device_id, devices: devices.map(key => ({ ...key,
        admitted: lineage.final.members.get(pseudonym)?.devices.has(key.device_id) ?? false,
        revoked: lineage.final.revokedDevices.has(key.device_id) })) };
    },
    approveChannelDevice: async (id, certificate) => {
      if (certificate.pseudonym_id !== pseudonym) throw new E2eError('Only devices of this identity may be approved');
      const { lineage } = await inspect(id);
      observe(await manifests.apply(lineage, { kind: 'admit_device', certificate }));
    },
    revokeChannelDevice: async (id, deviceId) => {
      const { lineage } = await inspect(id);
      observe(await manifests.apply(lineage, { kind: 'revoke_device', device_id: hash(deviceId) }));
    },
    admissionDevices: async (id, target) => {
      await moderatorLineage(id);
      const directory = await certifiedDevices(hash(target));
      return directory.devices.flatMap(device => device.device_certificate ? [device.device_certificate] : []);
    },
    admitChannelMember: async (id, certificate) => {
      const lineage = await moderatorLineage(id);
      observe(await manifests.apply(lineage, { kind: 'admit_member', certificate }));
    },
    setAdmissionPolicy: async (id, join_policy) => {
      const lineage = await moderatorLineage(id);
      observe(await manifests.apply(lineage, { kind: 'set_policy', join_policy }));
    },
    setChannelRole: async (id, target, role) => {
      const lineage = await moderatorLineage(id);
      observe(await manifests.apply(lineage, { kind: 'set_role', pseudonym_id: hash(target), role }));
    },
    removeChannelMember: async (id, target) => {
      const lineage = await moderatorLineage(id);
      observe(await manifests.apply(lineage, { kind: 'remove_member', pseudonym_id: hash(target) }));
    },
    getState: async id => (await inspect(id)).state,
    publishDevice: publish,
    getDevices: certifiedDevices,
    revokeDevice: deviceId => call(`/api/keys/me/${hash(deviceId)}`, value => {
      const item = record(value);
      if (item.revoked !== true || item.device_id !== deviceId) throw new E2eError('Device revocation was not confirmed');
      return { device_id: deviceId, revoked: true, affected_channels: array(item.affected_channels, affected, 10000) };
    }, { method: 'DELETE' }),
    getDirectory: async (id, page) => (await directory(id, page)).value,
    getWraps: async (id, deviceId, page) => {
      const result = await call(`${channel(id)}/key-wraps${query(page, deviceId)}`, parseOwnChannelWraps);
      const { lineage } = await inspect(id, result.state);
      for (const wrap of [...result.wraps, ...result.legacy_device_wraps]) {
        if (wrap.authorization === null) {
          if (lineage.final.head && wrap.epoch >= result.state.required_epoch) throw new E2eError('An unsigned key cannot authorize the current encryption epoch');
          continue;
        }
        manifests.requireTrusted(lineage);
        const historical = lineage.history.get(wrap.authorization.manifest_seq);
        if (!historical) throw new E2eError('A key authorization refers to missing membership history');
        if (wrap.authorization.sender_pseudonym_id !== wrap.sender_pseudonym_id || wrap.authorization.sender_device_id !== wrap.sender_device_id) throw new E2eError('Key distributor differs from its signed authorization');
        verifyWrapAuthorization(wrap.authorization, historical, { recipient_pseudonym_id: pseudonym,
          recipient_device_id: wrap.recipient_device_id, wrapped_key_b64: wrap.wrapped_key_b64 }, wrap.epoch, wrap.key_id);
      }
      return result;
    },
    postWraps: async (id, payload) => {
      const snapshot = await directory(id);
      if (payload.expected_revision !== snapshot.value.revision) throw Object.assign(new E2eError('Channel key snapshot changed'), { status: 409 });
      const signing = await signer();
      let authorization;
      try { authorization = signWrapAuthorization(snapshot.lineage.final, payload, snapshot.value.member_keys, signing.certificate, signing.secret); }
      finally { signing.secret.fill(0); }
      return call(`${channel(id)}/key-wraps`, value => {
        const item = record(value);
        const inserted = integer(item.inserted);
        if (item.status !== 'ok' || inserted > payload.wraps.length) throw new E2eError('Key distribution was not confirmed');
        return { status: 'ok', inserted, state: parseChannelKeyState(item.state) };
      }, { method: 'POST', body: JSON.stringify({ ...payload, authorization }) });
    },
  };
}
