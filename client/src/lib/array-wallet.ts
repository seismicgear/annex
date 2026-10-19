import type { ArrayIdentityBackup } from '@/lib/tauri';

const FIELD_ORDER = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;

/** Explicit portable fields: never transfer local IDs, sessions or cached proofs. */
export function portableArrayIdentity(value: unknown): ArrayIdentityBackup {
  const invalid = () => new Error('This is not a usable Annex identity backup.');
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalid();
  const identity = value as Record<string, unknown>;
  if (typeof identity.sk !== 'string' || !/^[0-9a-f]{1,64}$/i.test(identity.sk)
    || typeof identity.commitmentHex !== 'string' || !/^[0-9a-f]{64}$/i.test(identity.commitmentHex)
    || !Number.isSafeInteger(identity.roleCode) || (identity.roleCode as number) < 1 || (identity.roleCode as number) > 255
    || !Number.isSafeInteger(identity.nodeId) || (identity.nodeId as number) < 0
    || typeof identity.serverSlug !== 'string' || new TextEncoder().encode(identity.serverSlug).length > 512
    || typeof identity.createdAt !== 'string' || !identity.createdAt || identity.createdAt.length > 128
    || (identity.pseudonymId != null && (typeof identity.pseudonymId !== 'string' || identity.pseudonymId.length > 256))) {
    throw invalid();
  }
  const secret = BigInt(`0x${identity.sk}`);
  if (secret === 0n || secret >= FIELD_ORDER) throw invalid();
  return {
    sk: identity.sk.toLowerCase(), roleCode: identity.roleCode as number, nodeId: identity.nodeId as number,
    commitmentHex: identity.commitmentHex.toLowerCase(),
    pseudonymId: (identity.pseudonymId as string | null) ?? null,
    serverSlug: identity.serverSlug, createdAt: identity.createdAt,
  };
}

/** Recompute the commitment before importing recovered key material. */
export async function verifiedArrayIdentity(value: unknown): Promise<ArrayIdentityBackup> {
  const identity = portableArrayIdentity(value);
  const { computeCommitment } = await import('@/lib/zk');
  const commitment = await computeCommitment(BigInt(`0x${identity.sk}`), identity.roleCode, identity.nodeId);
  if (commitment !== identity.commitmentHex) throw new Error('The Annex backup does not match its cryptographic identity.');
  return identity;
}

/** Native commands return curated errors; unexpected runtime details stay local. */
export function arrayWalletMessage(error: unknown, fallback: string): string {
  const message = typeof error === 'string' ? error : error instanceof Error ? error.message : '';
  const allowed = [
    'Array is unavailable. Reopen Annex from Array, or export a backup file.',
    'Array returned an invalid wallet response. Reopen Annex from Array.',
    'The Array wallet is busy. Try again.',
    'Pair Annex in Array Settings to use its credential wallet.',
    'Unlock the credential wallet in Array and try again.',
    "Array did not allow this action. Check Annex's wallet permissions in Array Settings.",
    'This backup is no longer available in Array.',
    'This is not a usable Annex identity backup.',
    'The Annex backup does not match its cryptographic identity.',
    'Annex could not update its Array wallet preference. Check the Annex data folder and try again.',
  ];
  return allowed.includes(message) ? message : fallback;
}
