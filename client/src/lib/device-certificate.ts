/** Proof of device-key possession. Channel authorization is a separate signed manifest. */
import { ed25519 } from '@noble/curves/ed25519.js';
import { sha256 } from '@noble/hashes/sha2.js';
import type { Groth16Proof } from 'snarkjs';
import { E2eError, toHex, utf8 } from './e2e';
import { decodeHex32, deviceIdForPublicKey, requireSafeInteger } from './e2e-v2';
import pinnedVerifier from './membership-v2-verifier';
import { verifyMembershipProof } from './membership-verification';
import { computeTopicHashV2, computeTopicNullifierV2, generateMembershipProofV2 } from './zk';
import type { MembershipProofInput } from './zk';

const FR = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;
const FQ = 21888242871839275222246405745257275088696311157297823662689037894645226208583n;

export interface DeviceCertificate {
  version: 1;
  server_identity: string;
  topic: string;
  pseudonym_id: string;
  device_id: string;
  x25519_pub_hex: string;
  ed25519_pub_hex: string;
  root_hex: string;
  commitment_hex: string;
  nullifier_hex: string;
  generation: number;
  previous_certificate_sha256: string | null;
  proof: Groth16Proof;
  public_signals: string[];
  signature_hex: string;
}
export interface CertificateContext {
  version: 1;
  server_identity: string;
  server_ed25519_pub_hex: string;
  topic: string;
  commitment_hex: string;
  nullifier_hex: string;
  verifier_sha256: string;
}
export type CertificateBody = Omit<DeviceCertificate, 'proof' | 'public_signals' | 'signature_hex'>;

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new E2eError('Malformed device certificate');
  return value as Record<string, unknown>;
}
function text(value: unknown, maximum: number): string {
  if (typeof value !== 'string' || !value.length || value.length > maximum) throw new E2eError('Invalid certificate text');
  return value;
}
function hex(value: unknown): string { const result = text(value, 64); decodeHex32(result); return result; }
function field(value: unknown): string {
  const result = hex(value);
  if (BigInt(`0x${result}`) >= FR) throw new E2eError('Certificate scalar is outside the field');
  return result;
}
function coordinate(value: unknown): string {
  if (typeof value !== 'string' || !/^(0|[1-9][0-9]{0,76})$/.test(value) || BigInt(value) >= FQ) {
    throw new E2eError('Invalid proof coordinate');
  }
  return value;
}
function g1(value: unknown): [string, string, string] {
  if (!Array.isArray(value) || value.length !== 3 || value[2] !== '1') throw new E2eError('Invalid proof G1 point');
  return [coordinate(value[0]), coordinate(value[1]), '1'];
}
function g2(value: unknown): string[][] {
  if (!Array.isArray(value) || value.length !== 3 || !Array.isArray(value[2])
    || value[2].length !== 2 || value[2][0] !== '1' || value[2][1] !== '0') throw new E2eError('Invalid proof G2 point');
  const pair = (part: unknown): string[] => {
    if (!Array.isArray(part) || part.length !== 2) throw new E2eError('Invalid proof extension coordinate');
    return part.map(coordinate);
  };
  return [pair(value[0]), pair(value[1]), ['1', '0']];
}
function parseProof(value: unknown): Groth16Proof {
  const proof = object(value);
  if (proof.protocol !== 'groth16' || proof.curve !== 'bn128') throw new E2eError('Unsupported certificate proof');
  return { protocol: 'groth16', curve: 'bn128', pi_a: g1(proof.pi_a), pi_b: g2(proof.pi_b), pi_c: g1(proof.pi_c) };
}
export function parseDeviceCertificate(value: unknown): DeviceCertificate {
  const item = object(value);
  if (item.version !== 1 || typeof item.server_identity !== 'string'
    || !/^annex-server:[0-9a-f]{64}$/.test(item.server_identity)) throw new E2eError('Unsupported device certificate');
  const generation = typeof item.generation === 'number' ? requireSafeInteger(item.generation, 1) : 0;
  const previous = item.previous_certificate_sha256 === null ? null : hex(item.previous_certificate_sha256);
  if (!generation || (generation === 1) !== (previous === null)) throw new E2eError('Invalid device certificate lineage');
  if (!Array.isArray(item.public_signals) || item.public_signals.length !== 5) throw new E2eError('Certificate must have five public signals');
  const signature = text(item.signature_hex, 128);
  if (!/^[0-9a-f]{128}$/.test(signature)) throw new E2eError('Invalid device possession signature');
  return { version: 1, server_identity: item.server_identity, topic: text(item.topic, 2048),
    pseudonym_id: hex(item.pseudonym_id), device_id: hex(item.device_id), x25519_pub_hex: hex(item.x25519_pub_hex),
    ed25519_pub_hex: hex(item.ed25519_pub_hex), root_hex: field(item.root_hex), commitment_hex: field(item.commitment_hex),
    nullifier_hex: field(item.nullifier_hex), generation, previous_certificate_sha256: previous,
    proof: parseProof(item.proof), public_signals: item.public_signals.map(field), signature_hex: signature };
}

export function certificateDigest(certificate: CertificateBody): string {
  return toHex(sha256(utf8.encode(JSON.stringify(['annex-device-certificate-v1', certificate.server_identity,
    certificate.topic, certificate.pseudonym_id, certificate.device_id, certificate.x25519_pub_hex,
    certificate.ed25519_pub_hex, certificate.root_hex, certificate.commitment_hex, certificate.nullifier_hex,
    certificate.generation, certificate.previous_certificate_sha256]))));
}
export function certificateChallenge(certificate: CertificateBody): string {
  return (BigInt(`0x${certificateDigest(certificate)}`) % FR).toString(16).padStart(64, '0');
}
export function certificatePossessionPayload(certificate: CertificateBody): Uint8Array {
  return utf8.encode(JSON.stringify(['annex-device-possession-v1', certificateDigest(certificate)]));
}

/** Semantic digest of this application's embedded verifier, never a fetched key. */
export function membershipVerifierFingerprint(): string {
  if (pinnedVerifier.protocol !== 'groth16' || pinnedVerifier.curve !== 'bn128'
    || pinnedVerifier.nPublic !== 5 || pinnedVerifier.IC.length !== 6) throw new E2eError('Invalid shipped membership verifier');
  return toHex(sha256(utf8.encode(JSON.stringify(['annex-membership-v2-verifier-v1',
    g1(pinnedVerifier.vk_alpha_1).slice(0, 2), g2(pinnedVerifier.vk_beta_2).slice(0, 2),
    g2(pinnedVerifier.vk_gamma_2).slice(0, 2), g2(pinnedVerifier.vk_delta_2).slice(0, 2),
    pinnedVerifier.IC.map(point => g1(point).slice(0, 2))]))));
}
export function parseCertificateContext(value: unknown): CertificateContext {
  const item = object(value);
  const publicKey = hex(item.server_ed25519_pub_hex);
  const identity = `annex-server:${toHex(sha256(decodeHex32(publicKey)))}`;
  if (item.version !== 1 || item.server_identity !== identity
    || item.verifier_sha256 !== membershipVerifierFingerprint()) {
    throw new E2eError('The server encryption identity or membership verifier does not match this application');
  }
  return { version: 1, server_identity: identity, server_ed25519_pub_hex: publicKey,
    topic: text(item.topic, 2048), commitment_hex: field(item.commitment_hex), nullifier_hex: field(item.nullifier_hex),
    verifier_sha256: item.verifier_sha256 as string };
}

const verified = new Map<string, Promise<DeviceCertificate>>();
/** This proves possession only; callers must also verify the channel's signed membership lineage. */
export async function verifyDeviceCertificate(value: unknown, expected: {
  serverIdentity: string; pseudonym: string; deviceId: string; publicKeyHex: string;
}): Promise<DeviceCertificate> {
  const cert = parseDeviceCertificate(value);
  if (cert.server_identity !== expected.serverIdentity || cert.pseudonym_id !== expected.pseudonym
    || cert.device_id !== expected.deviceId || cert.x25519_pub_hex !== expected.publicKeyHex
    || deviceIdForPublicKey(decodeHex32(cert.x25519_pub_hex)) !== cert.device_id
    || toHex(sha256(utf8.encode(`${cert.topic}:${cert.nullifier_hex}`))) !== cert.pseudonym_id) {
    throw new E2eError('Device certificate is bound to another identity or key');
  }
  const cacheKey = toHex(sha256(utf8.encode(JSON.stringify(cert))));
  const prior = verified.get(cacheKey);
  if (prior) return prior;
  const promise = (async () => {
    const topic = (await computeTopicHashV2(cert.topic)).toString(16).padStart(64, '0');
    const signals = [cert.root_hex, cert.commitment_hex, cert.nullifier_hex, topic, certificateChallenge(cert)];
    if (signals.some((signal, index) => cert.public_signals[index] !== signal)) throw new E2eError('Device proof does not bind every certificate field');
    const signature = Uint8Array.from(cert.signature_hex.match(/../g)!, part => Number.parseInt(part, 16));
    if (!ed25519.verify(signature, certificatePossessionPayload(cert), decodeHex32(cert.ed25519_pub_hex), { zip215: false })) {
      throw new E2eError('Device possession signature is invalid');
    }
    if (!await verifyMembershipProof(signals.map(signal => BigInt(`0x${signal}`).toString()), cert.proof)) {
      throw new E2eError('Device membership proof is invalid');
    }
    return cert;
  })();
  verified.set(cacheKey, promise);
  try { return await promise; }
  catch (error) { verified.delete(cacheKey); throw error; }
  finally { if (verified.size > 1024) verified.delete(verified.keys().next().value!); }
}

/** Generates a static ownership proof, without requesting or consuming a login challenge. */
export async function createDeviceCertificate(input: {
  context: CertificateContext; pseudonym: string; publicKeyHex: string; signingSecret: Uint8Array;
  membership: MembershipProofInput; rootHex: string; previous?: DeviceCertificate;
}): Promise<DeviceCertificate> {
  const context = parseCertificateContext(input.context);
  const nullifier = await computeTopicNullifierV2(input.membership.sk, context.topic);
  if (context.nullifier_hex !== nullifier) throw new E2eError('The registered identity nullifier does not match the local secret');
  const body: CertificateBody = { version: 1, server_identity: context.server_identity, topic: context.topic,
    pseudonym_id: input.pseudonym, device_id: deviceIdForPublicKey(decodeHex32(input.publicKeyHex)),
    x25519_pub_hex: input.publicKeyHex, ed25519_pub_hex: toHex(ed25519.getPublicKey(input.signingSecret)),
    root_hex: field(input.rootHex), commitment_hex: context.commitment_hex, nullifier_hex: nullifier,
    generation: input.previous ? input.previous.generation + 1 : 1,
    previous_certificate_sha256: input.previous ? certificateDigest(input.previous) : null };
  if (toHex(sha256(utf8.encode(`${context.topic}:${nullifier}`))) !== input.pseudonym) throw new E2eError('The local identity does not own this pseudonym');
  if (input.previous && (input.previous.pseudonym_id !== input.pseudonym
    || input.previous.server_identity !== context.server_identity)) throw new E2eError('Invalid replacement certificate context');
  const proof = await generateMembershipProofV2(input.membership, context.topic, certificateChallenge(body));
  const cert: DeviceCertificate = { ...body, proof: proof.proof,
    public_signals: proof.publicSignals.map(signal => BigInt(signal).toString(16).padStart(64, '0')),
    signature_hex: toHex(ed25519.sign(certificatePossessionPayload(body), input.signingSecret)) };
  return verifyDeviceCertificate(cert, { serverIdentity: context.server_identity, pseudonym: input.pseudonym,
    deviceId: body.device_id, publicKeyHex: input.publicKeyHex });
}
