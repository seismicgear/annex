/** Independent member authorization. A server directory alone never authorizes a key recipient. */
import { ed25519 } from '@noble/curves/ed25519.js';
import { sha256 } from '@noble/hashes/sha2.js';
import { E2eError, toHex, utf8 } from './e2e';
import { decodeCanonicalBase64, decodeHex32, requireSafeInteger } from './e2e-v2';
import { certificateDigest, parseDeviceCertificate, verifyDeviceCertificate, type DeviceCertificate } from './device-certificate';
import type { ChannelWrapCommit, ChannelWrapUpload, MemberDeviceKey } from './e2e-v2-protocol';

export type JoinPolicy = 'eligible_self_join' | 'moderator_admission';
export type ManifestOperation =
  | { kind: 'genesis'; join_policy: JoinPolicy; history_policy: 'all_members'; certificate: DeviceCertificate }
  | { kind: 'join' | 'admit_member' | 'admit_device' | 'recover_device'; certificate: DeviceCertificate }
  | { kind: 'leave' }
  | { kind: 'remove_member'; pseudonym_id: string }
  | { kind: 'set_role'; pseudonym_id: string; role: 'member' | 'moderator' }
  | { kind: 'revoke_device'; device_id: string }
  | { kind: 'set_policy'; join_policy: JoinPolicy }
  | { kind: 'set_encryption'; enabled: boolean };
export interface SignedManifestEvent {
  version: 1; authority_identity: string; channel_id: string; seq: number; previous_hash: string;
  operation: ManifestOperation; signer_pseudonym_id: string; signer_device_id: string;
  signer_certificate_sha256: string; event_hash: string; signature_hex: string;
}
export interface ManifestProjection {
  head_seq: number; head_hash: string; genesis_hash: string | null; join_policy: JoinPolicy; history_policy: 'all_members';
  e2e_enabled: boolean;
  revoked_devices: [string, string][];
  members: { pseudonym_id: string; role: 'member' | 'moderator'; devices: { device_id: string; certificate_sha256: string }[] }[];
}
export interface ManifestPage {
  version: 1; authority_identity: string; channel_id: string; head_seq: number;
  head_hash: string; genesis_hash: string | null; events: SignedManifestEvent[]; next_after_seq: number | null;
}
export interface ManifestPin { seq: number; hash: string; genesisHash: string }
export type WrapDigest = [string, string, string, string];
export interface WrapAuthorizationReceipt {
  version: 1; authority_identity: string; channel_id: string;
  sender_pseudonym_id: string; sender_device_id: string; sender_certificate_sha256: string;
  manifest_seq: number; manifest_hash: string; operation: ChannelWrapCommit['operation'];
  expected_revision: number; epoch: number; key_id: string; wrap_digests: WrapDigest[];
  signature_hex: string; authorization_sha256: string;
}
interface Member { role: 'member' | 'moderator'; devices: Map<string, DeviceCertificate> }
export interface VerifiedManifest {
  authorityIdentity: string; channelId: string; head: ManifestPin | null; joinPolicy: JoinPolicy; enabled: boolean;
  members: Map<string, Member>; revokedDevices: Set<string>; certificates: Map<string, DeviceCertificate>;
}

function fail(message: string): never { throw new E2eError(message); }
function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return fail('Invalid manifest object');
  return value as Record<string, unknown>;
}
function exact(value: Record<string, unknown>, keys: string[]): void {
  if (Object.keys(value).length !== keys.length || Object.keys(value).some(key => !keys.includes(key))) fail('Unrecognized manifest fields');
}
function string(value: unknown, max = 256): string {
  if (typeof value !== 'string' || !value.length || value.length > max) return fail('Invalid manifest text');
  return value;
}
function hash(value: unknown): string { const result = string(value, 64); decodeHex32(result); return result; }
function number(value: unknown, minimum = 0): number {
  if (typeof value !== 'number') return fail('Invalid manifest counter');
  return requireSafeInteger(value, minimum);
}
function signature(value: unknown): string {
  if (typeof value !== 'string' || !/^[0-9a-f]{128}$/.test(value)) return fail('Invalid manifest signature encoding');
  return value;
}
function authority(value: unknown): string {
  if (typeof value !== 'string' || !/^annex-server:[0-9a-f]{64}$/.test(value)) return fail('Invalid manifest authority');
  return value;
}
function policy(value: unknown): JoinPolicy {
  if (value !== 'eligible_self_join' && value !== 'moderator_admission') return fail('Unsupported channel admission policy');
  return value;
}
export function canonicalOperation(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalOperation);
  if (value !== null && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonicalOperation((value as Record<string, unknown>)[key])]));
  if (typeof value === 'number') requireSafeInteger(value);
  if (!['string', 'number', 'boolean'].includes(typeof value) && value !== null) fail('Invalid canonical manifest value');
  return value;
}
export function parseManifestOperation(value: unknown): ManifestOperation {
  const item = record(value);
  switch (item.kind) {
    case 'genesis':
      exact(item, ['kind', 'join_policy', 'history_policy', 'certificate']);
      if (item.history_policy !== 'all_members') return fail('Unsupported channel history policy');
      return { kind: 'genesis', join_policy: policy(item.join_policy), history_policy: 'all_members', certificate: parseDeviceCertificate(item.certificate) };
    case 'join': case 'admit_member': case 'admit_device': case 'recover_device':
      exact(item, ['kind', 'certificate']);
      return { kind: item.kind, certificate: parseDeviceCertificate(item.certificate) };
    case 'leave': exact(item, ['kind']); return { kind: 'leave' };
    case 'remove_member': exact(item, ['kind', 'pseudonym_id']); return { kind: 'remove_member', pseudonym_id: hash(item.pseudonym_id) };
    case 'set_role':
      exact(item, ['kind', 'pseudonym_id', 'role']);
      if (item.role !== 'member' && item.role !== 'moderator') return fail('Invalid manifest member role');
      return { kind: 'set_role', pseudonym_id: hash(item.pseudonym_id), role: item.role };
    case 'revoke_device': exact(item, ['kind', 'device_id']); return { kind: 'revoke_device', device_id: hash(item.device_id) };
    case 'set_policy': exact(item, ['kind', 'join_policy']); return { kind: 'set_policy', join_policy: policy(item.join_policy) };
    case 'set_encryption':
      exact(item, ['kind', 'enabled']);
      if (typeof item.enabled !== 'boolean') return fail('Invalid signed encryption policy');
      return { kind: 'set_encryption', enabled: item.enabled };
    default: return fail('Unsupported manifest operation');
  }
}
export function parseManifestEvent(value: unknown): SignedManifestEvent {
  const item = record(value);
  exact(item, ['version', 'authority_identity', 'channel_id', 'seq', 'previous_hash', 'operation',
    'signer_pseudonym_id', 'signer_device_id', 'signer_certificate_sha256', 'event_hash', 'signature_hex']);
  if (item.version !== 1) return fail('Unsupported manifest version');
  const seq = number(item.seq, 1);
  if (seq === 1 && item.previous_hash !== '') return fail('Genesis must not have a predecessor');
  return { version: 1, authority_identity: authority(item.authority_identity), channel_id: string(item.channel_id),
    seq, previous_hash: seq === 1 ? '' : hash(item.previous_hash), operation: parseManifestOperation(item.operation),
    signer_pseudonym_id: hash(item.signer_pseudonym_id), signer_device_id: hash(item.signer_device_id),
    signer_certificate_sha256: hash(item.signer_certificate_sha256), event_hash: hash(item.event_hash), signature_hex: signature(item.signature_hex) };
}
export function parseManifestPage(value: unknown): ManifestPage {
  const item = record(value);
  if (item.version !== 1 || !Array.isArray(item.events) || item.events.length > 200) return fail('Invalid manifest history page');
  const head = number(item.head_seq);
  const result: ManifestPage = { version: 1, authority_identity: authority(item.authority_identity), channel_id: string(item.channel_id),
    head_seq: head, head_hash: head === 0 && item.head_hash === '' ? '' : hash(item.head_hash),
    genesis_hash: item.genesis_hash === null ? null : hash(item.genesis_hash), events: item.events.map(parseManifestEvent),
    next_after_seq: item.next_after_seq === null ? null : number(item.next_after_seq, 1) };
  if ((head === 0) !== (result.genesis_hash === null)) return fail('Invalid manifest genesis reference');
  return result;
}
export function parseManifestProjection(value: unknown): ManifestProjection {
  const item = record(value);
  if (item.history_policy !== 'all_members' || typeof item.e2e_enabled !== 'boolean'
    || !Array.isArray(item.members) || item.members.length > 10000
    || !Array.isArray(item.revoked_devices) || item.revoked_devices.length > 100000) return fail('Invalid channel manifest projection');
  const head = number(item.head_seq);
  return { head_seq: head, head_hash: head === 0 && item.head_hash === '' ? '' : hash(item.head_hash),
    genesis_hash: item.genesis_hash === null ? null : hash(item.genesis_hash), join_policy: policy(item.join_policy), history_policy: 'all_members',
    e2e_enabled: item.e2e_enabled, revoked_devices: item.revoked_devices.map(value => {
      if (!Array.isArray(value) || value.length !== 2) return fail('Invalid revoked device tuple');
      return [hash(value[0]), hash(value[1])] as [string, string];
    }).sort((a, b) => a[0].localeCompare(b[0]) || a[1].localeCompare(b[1])),
    members: item.members.map(value => {
      const member = record(value);
      if (!Array.isArray(member.devices) || member.devices.length > 10000 || (member.role !== 'member' && member.role !== 'moderator')) return fail('Invalid manifest member');
      return { pseudonym_id: hash(member.pseudonym_id), role: member.role as 'member' | 'moderator', devices: member.devices.map(value => {
        const device = record(value); return { device_id: hash(device.device_id), certificate_sha256: hash(device.certificate_sha256) };
      }).sort((a, b) => a.device_id.localeCompare(b.device_id)) };
    }).sort((a, b) => a.pseudonym_id.localeCompare(b.pseudonym_id)) };
}
export function parseWrapAuthorization(value: unknown): WrapAuthorizationReceipt {
  const item = record(value);
  if (item.version !== 1 || !['commit_epoch', 'admit_devices', 'admit_history_devices', 'admit_legacy_devices'].includes(String(item.operation))
    || !Array.isArray(item.wrap_digests) || item.wrap_digests.length < 1 || item.wrap_digests.length > 10000) return fail('Invalid key authorization receipt');
  return { version: 1, authority_identity: authority(item.authority_identity), channel_id: string(item.channel_id),
    sender_pseudonym_id: hash(item.sender_pseudonym_id), sender_device_id: hash(item.sender_device_id), sender_certificate_sha256: hash(item.sender_certificate_sha256),
    manifest_seq: number(item.manifest_seq, 1), manifest_hash: hash(item.manifest_hash), operation: item.operation as ChannelWrapCommit['operation'],
    expected_revision: number(item.expected_revision), epoch: number(item.epoch, 1), key_id: hash(item.key_id),
    wrap_digests: item.wrap_digests.map(value => {
      if (!Array.isArray(value) || value.length !== 4) return fail('Invalid key authorization tuple');
      return value.map(hash) as WrapDigest;
    }), signature_hex: signature(item.signature_hex), authorization_sha256: hash(item.authorization_sha256) };
}
export function manifestSigningInput(event: Omit<SignedManifestEvent, 'event_hash' | 'signature_hex'>): Uint8Array {
  return utf8.encode(JSON.stringify(['annex-channel-manifest-v1', 1, event.authority_identity, event.channel_id,
    event.seq, event.previous_hash, canonicalOperation(event.operation), event.signer_pseudonym_id,
    event.signer_device_id, event.signer_certificate_sha256]));
}
export function signManifestEvent(
  base: Pick<SignedManifestEvent, 'authority_identity' | 'channel_id' | 'seq' | 'previous_hash' | 'operation'>,
  certificate: DeviceCertificate, secret: Uint8Array,
): SignedManifestEvent {
  if (toHex(ed25519.getPublicKey(secret)) !== certificate.ed25519_pub_hex) return fail('Manifest signer does not own its certificate');
  const event = { version: 1 as const, ...base, signer_pseudonym_id: certificate.pseudonym_id,
    signer_device_id: certificate.device_id, signer_certificate_sha256: certificateDigest(certificate) };
  const input = manifestSigningInput(event);
  return parseManifestEvent({ ...event, event_hash: toHex(sha256(input)), signature_hex: toHex(ed25519.sign(input, secret)) });
}
function verifySignature(input: Uint8Array, encoded: string, certificate: DeviceCertificate): void {
  const bytes = Uint8Array.from(encoded.match(/../g)!, part => Number.parseInt(part, 16));
  if (!ed25519.verify(bytes, input, decodeHex32(certificate.ed25519_pub_hex), { zip215: false })) fail('Member signature is invalid');
}
export function emptyManifest(authorityIdentity: string, channelId: string): VerifiedManifest {
  return { authorityIdentity: authority(authorityIdentity), channelId: string(channelId), head: null,
    joinPolicy: 'moderator_admission', enabled: false, members: new Map(), revokedDevices: new Set(), certificates: new Map() };
}
function cloneManifest(previous: VerifiedManifest): VerifiedManifest {
  return { ...previous, members: new Map([...previous.members].map(([id, member]) => [id, { ...member, devices: new Map(member.devices) }])),
    revokedDevices: new Set(previous.revokedDevices), certificates: new Map(previous.certificates) };
}
function protectLastModerator(state: VerifiedManifest, pseudonym: string): void {
  if (state.members.get(pseudonym)?.role === 'moderator' && state.members.size > 1
    && [...state.members.values()].filter(member => member.role === 'moderator').length === 1) fail('The last channel moderator cannot be removed');
}

/** Purely reconstructs authorization from member-signed history; no server projection is trusted. */
export async function applyManifestEvent(previous: VerifiedManifest, value: unknown): Promise<VerifiedManifest> {
  const event = parseManifestEvent(value);
  if (event.authority_identity !== previous.authorityIdentity || event.channel_id !== previous.channelId
    || event.seq !== (previous.head?.seq ?? 0) + 1 || event.previous_hash !== (previous.head?.hash ?? '')) fail('Manifest ancestry or channel binding does not match');
  const input = manifestSigningInput(event);
  if (toHex(sha256(input)) !== event.event_hash) fail('Manifest event digest is invalid');
  const operation = event.operation;
  const certificate = 'certificate' in operation ? operation.certificate : null;
  if (certificate) {
    // A federated participant keeps its origin registry identity. Channel
    // authority is established separately by the signed admission lineage.
    await verifyDeviceCertificate(certificate, { serverIdentity: certificate.server_identity,
      pseudonym: certificate.pseudonym_id, deviceId: certificate.device_id, publicKeyHex: certificate.x25519_pub_hex });
    if (previous.revokedDevices.has(certificate.device_id)) fail('A revoked encryption device cannot be readmitted');
    if (certificate.previous_certificate_sha256 !== null) {
      const predecessor = previous.certificates.get(certificate.previous_certificate_sha256);
      if (!predecessor || predecessor.pseudonym_id !== certificate.pseudonym_id
        || predecessor.generation + 1 !== certificate.generation) fail('Certificate replacement lineage is not established');
    }
  }
  const selfCertified = operation.kind === 'genesis' || operation.kind === 'join' || operation.kind === 'recover_device';
  const signerMember = previous.members.get(event.signer_pseudonym_id);
  const signer = selfCertified ? certificate : signerMember?.devices.get(event.signer_device_id);
  if (!signer || signer.pseudonym_id !== event.signer_pseudonym_id || signer.device_id !== event.signer_device_id
    || certificateDigest(signer) !== event.signer_certificate_sha256) fail('Manifest signer is not an authorized device');
  verifySignature(input, event.signature_hex, signer);
  const state = cloneManifest(previous);
  const needModerator = () => { if (signerMember?.role !== 'moderator') fail('This action requires an authorized channel moderator'); };
  switch (operation.kind) {
    case 'genesis':
      if (previous.head || state.members.size) fail('A channel cannot replace its genesis');
      state.joinPolicy = operation.join_policy;
      state.enabled = true;
      state.members.set(signer.pseudonym_id, { role: 'moderator', devices: new Map([[signer.device_id, signer]]) });
      break;
    case 'join':
      if (!previous.head || previous.joinPolicy !== 'eligible_self_join' || signerMember) fail('This channel does not authorize self-admission');
      state.members.set(signer.pseudonym_id, { role: state.members.size === 0 ? 'moderator' : 'member', devices: new Map([[signer.device_id, signer]]) });
      break;
    case 'admit_member':
      needModerator();
      if (state.members.has(operation.certificate.pseudonym_id)) fail('The identity is already a channel member');
      state.members.set(operation.certificate.pseudonym_id, { role: 'member', devices: new Map([[operation.certificate.device_id, operation.certificate]]) });
      break;
    case 'leave': protectLastModerator(state, signer.pseudonym_id); state.members.delete(signer.pseudonym_id); break;
    case 'remove_member':
      needModerator();
      if (operation.pseudonym_id === signer.pseudonym_id || !state.members.has(operation.pseudonym_id)) fail('Invalid member removal');
      protectLastModerator(state, operation.pseudonym_id); state.members.delete(operation.pseudonym_id); break;
    case 'set_role': {
      needModerator();
      const member = state.members.get(operation.pseudonym_id);
      if (!member) return fail('Role target is not a channel member');
      if (operation.role === 'member') protectLastModerator(state, operation.pseudonym_id);
      member.role = operation.role; break;
    }
    case 'admit_device': {
      const member = state.members.get(signer.pseudonym_id);
      if (!member || operation.certificate.pseudonym_id !== signer.pseudonym_id) fail('A device can only admit its own identity');
      member!.devices.set(operation.certificate.device_id, operation.certificate); break;
    }
    case 'recover_device': {
      const member = state.members.get(signer.pseudonym_id);
      if (!member || member.devices.has(signer.device_id)) fail('Identity recovery requires a new device of an existing member');
      for (const id of member.devices.keys()) state.revokedDevices.add(id);
      member.devices = new Map([[signer.device_id, signer]]); break;
    }
    case 'revoke_device': {
      const member = state.members.get(signer.pseudonym_id);
      if (!member?.devices.has(operation.device_id)) fail('A device may only revoke a device of its own identity');
      member!.devices.delete(operation.device_id); state.revokedDevices.add(operation.device_id); break;
    }
    case 'set_policy': needModerator(); state.joinPolicy = operation.join_policy; break;
    case 'set_encryption': needModerator(); state.enabled = operation.enabled; break;
  }
  if (state.members.size > 0 && ![...state.members.values()].some(member => member.role === 'moderator')) {
    fail('An encrypted channel must retain an authorized moderator');
  }
  if (state.members.size === 0 && state.joinPolicy !== 'eligible_self_join') {
    fail('The final moderator must allow eligible self-join before leaving an empty channel');
  }
  if (certificate) state.certificates.set(certificateDigest(certificate), certificate);
  state.head = { seq: event.seq, hash: event.event_hash, genesisHash: previous.head?.genesisHash ?? event.event_hash };
  return state;
}

export function manifestProjection(state: VerifiedManifest): ManifestProjection {
  return { head_seq: state.head?.seq ?? 0, head_hash: state.head?.hash ?? '', genesis_hash: state.head?.genesisHash ?? null,
    join_policy: state.joinPolicy, history_policy: 'all_members', e2e_enabled: state.enabled,
    revoked_devices: [...state.revokedDevices].map(device_id => {
      const cert = [...state.certificates.values()].find(value => value.device_id === device_id);
      if (!cert) return fail('A revoked device has no established certificate');
      return [cert.pseudonym_id, device_id] as [string, string];
    }).sort((a, b) => a[0].localeCompare(b[0]) || a[1].localeCompare(b[1])), members: [...state.members].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)
      .map(([pseudonym_id, member]) => ({ pseudonym_id, role: member.role, devices: [...member.devices].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)
        .map(([device_id, cert]) => ({ device_id, certificate_sha256: certificateDigest(cert) })) })) };
}
export function checkManifestPin(pin: ManifestPin | null, history: Map<number, VerifiedManifest>, final: VerifiedManifest): void {
  if (!pin) return;
  const previous = history.get(pin.seq)?.head;
  if (!final.head || final.head.seq < pin.seq || !previous || previous.hash !== pin.hash || previous.genesisHash !== pin.genesisHash) {
    fail('The channel manifest rolled back or conflicts with the previously trusted history');
  }
}
export function checkManifestRecipient(state: VerifiedManifest, recipient: MemberDeviceKey): string {
  const certificate = state.members.get(recipient.pseudonym_id)?.devices.get(recipient.device_id);
  if (!certificate || recipient.device_certificate === null || certificate.x25519_pub_hex !== recipient.x25519_pub_hex
    || certificateDigest(certificate) !== certificateDigest(recipient.device_certificate)) fail('The server supplied a recipient outside the verified channel manifest');
  return certificateDigest(certificate);
}
export function wrapSigningInput(receipt: Omit<WrapAuthorizationReceipt, 'signature_hex' | 'authorization_sha256' | 'version'>): Uint8Array {
  return utf8.encode(JSON.stringify(['annex-channel-key-authorization-v1', receipt.authority_identity, receipt.channel_id,
    receipt.sender_pseudonym_id, receipt.sender_device_id, receipt.sender_certificate_sha256, receipt.manifest_seq,
    receipt.manifest_hash, receipt.operation, receipt.expected_revision, receipt.epoch, receipt.key_id, receipt.wrap_digests]));
}
function digestOrder(a: WrapDigest, b: WrapDigest): number { return a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : a[1] < b[1] ? -1 : a[1] > b[1] ? 1 : 0; }
export function signWrapAuthorization(
  state: VerifiedManifest, request: ChannelWrapCommit, recipients: MemberDeviceKey[], certificate: DeviceCertificate, secret: Uint8Array,
): { manifest_seq: number; manifest_hash: string; signature_hex: string } {
  if (!state.head || request.sender_device_id !== certificate.device_id
    || state.members.get(certificate.pseudonym_id)?.devices.get(certificate.device_id)?.ed25519_pub_hex !== certificate.ed25519_pub_hex
    || toHex(ed25519.getPublicKey(secret)) !== certificate.ed25519_pub_hex) return fail('This device is not an authorized key distributor');
  const tuples = request.wraps.map((wrap): WrapDigest => {
    const recipient = recipients.find(value => value.pseudonym_id === wrap.recipient_pseudonym_id && value.device_id === wrap.recipient_device_id);
    if (!recipient) return fail('Key wrap recipient is absent from the captured directory');
    return [recipient.pseudonym_id, recipient.device_id, checkManifestRecipient(state, recipient), toHex(sha256(decodeCanonicalBase64(wrap.wrapped_key_b64, 3072)))];
  }).sort(digestOrder);
  if (tuples.some((value, index) => index > 0 && digestOrder(tuples[index - 1], value) === 0)) fail('Duplicate key wrap recipient');
  const input = wrapSigningInput({ authority_identity: state.authorityIdentity, channel_id: state.channelId,
    sender_pseudonym_id: certificate.pseudonym_id, sender_device_id: certificate.device_id,
    sender_certificate_sha256: certificateDigest(certificate), manifest_seq: state.head.seq, manifest_hash: state.head.hash,
    operation: request.operation, expected_revision: request.expected_revision, epoch: request.epoch, key_id: request.key_id, wrap_digests: tuples });
  return { manifest_seq: state.head.seq, manifest_hash: state.head.hash, signature_hex: toHex(ed25519.sign(input, secret)) };
}
export function verifyWrapAuthorization(receipt: WrapAuthorizationReceipt, state: VerifiedManifest, wrap: ChannelWrapUpload, epoch: number, keyId: string): void {
  if (receipt.version !== 1 || receipt.authority_identity !== state.authorityIdentity || receipt.channel_id !== state.channelId
    || receipt.manifest_seq !== state.head?.seq || receipt.manifest_hash !== state.head.hash
    || receipt.epoch !== epoch || receipt.key_id !== keyId) fail('Key authorization is bound to another channel, epoch or manifest');
  number(receipt.expected_revision); number(receipt.epoch, 1); hash(receipt.key_id); signature(receipt.signature_hex);
  if (!['commit_epoch', 'admit_devices', 'admit_history_devices', 'admit_legacy_devices'].includes(receipt.operation)) fail('Unknown key authorization operation');
  const signer = state.members.get(receipt.sender_pseudonym_id)?.devices.get(receipt.sender_device_id);
  if (!signer || certificateDigest(signer) !== receipt.sender_certificate_sha256) fail('Key authorization signer was not admitted at this manifest');
  if (!Array.isArray(receipt.wrap_digests) || !receipt.wrap_digests.length || receipt.wrap_digests.length > 10000) fail('Invalid key authorization recipients');
  for (const [index, tuple] of receipt.wrap_digests.entries()) {
    if (!Array.isArray(tuple) || tuple.length !== 4) fail('Malformed key authorization tuple');
    tuple.forEach(hash);
    if (index > 0 && digestOrder(receipt.wrap_digests[index - 1], tuple) >= 0) fail('Key authorization recipients are not uniquely ordered');
    const recipient = state.members.get(tuple[0])?.devices.get(tuple[1]);
    if (!recipient || certificateDigest(recipient) !== tuple[2]) fail('Key authorization includes an unauthorized recipient');
  }
  const matched = receipt.wrap_digests.find(tuple => tuple[0] === wrap.recipient_pseudonym_id && tuple[1] === wrap.recipient_device_id);
  if (!matched || matched[3] !== toHex(sha256(decodeCanonicalBase64(wrap.wrapped_key_b64, 3072)))) fail('Key wrap bytes do not match their signed authorization');
  const input = wrapSigningInput(receipt);
  if (toHex(sha256(input)) !== receipt.authorization_sha256) fail('Key authorization digest is invalid');
  verifySignature(input, receipt.signature_hex, signer);
}
