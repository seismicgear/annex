/**
 * Identity, registration, ZK membership verification, and invite flows.
 */

import type {
  RegistrationResponse,
  VerifyMembershipResponse,
  IdentityInfo,
} from '@/types';
import { authHeaders, request } from './core';

export async function register(
  commitmentHex: string,
  roleCode: number,
  nodeId: number,
  inviteCode?: string,
  serverPassword?: string,
): Promise<RegistrationResponse> {
  return request<RegistrationResponse>('/api/registry/register', {
    method: 'POST',
    body: JSON.stringify({ commitmentHex, roleCode, nodeId, inviteCode, serverPassword }),
  });
}

export async function redeemInvite(
  baseUrl: string,
  code: string,
): Promise<{ valid: boolean; serverName: string; serverSlug: string }> {
  const resp = await fetch(`${baseUrl}/api/invites/redeem`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ code }),
  });
  if (!resp.ok) {
    const body = await resp.json().catch(() => ({}));
    throw new Error(body.error || `Invite validation failed: ${resp.status}`);
  }
  return resp.json();
}

export async function verifyMembership(
  root: string,
  commitment: string,
  topic: string,
  proof: unknown,
  publicSignals: string[],
  v2?: { nullifierHex: string; topicHashHex: string },
): Promise<VerifyMembershipResponse> {
  const body: Record<string, unknown> = { root, commitment, topic, proof, publicSignals };
  if (v2) {
    // v2 (secret-derived nullifier): the server checks publicSignals[2]/[3]
    // against these and recomputes topicHash from `topic`.
    body.protocolVersion = 'v2';
    body.nullifierHex = v2.nullifierHex;
    body.topicHashHex = v2.topicHashHex;
  }
  return request<VerifyMembershipResponse>('/api/zk/verify-membership', {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

/** The server's currently-active Merkle root (for proof-freshness checks). */
export async function getCurrentRoot(): Promise<{ rootHex: string; leafCount: number }> {
  return request<{ rootHex: string; leafCount: number }>('/api/registry/current-root');
}

// ───────── capability / linkage / federation ZK circuits (AUDIT P4-ID-1) ─────────

/** Verify a channel-eligibility proof. Returns the channel-scoped nullifier. */
export async function verifyChannelEligibility(
  root: string,
  channelTopic: string,
  requiredRoleCode: number,
  proof: unknown,
  publicSignals: string[],
): Promise<{ ok: boolean; nullifierHex: string }> {
  return request('/api/zk/channel-eligibility', {
    method: 'POST',
    body: JSON.stringify({ root, channelTopic, requiredRoleCode, proof, publicSignals }),
  });
}

/** Verify a voluntary pseudonym-linkage proof between two topics. */
export async function verifyLinkPseudonyms(
  topicA: string,
  topicB: string,
  proof: unknown,
  publicSignals: string[],
): Promise<{
  ok: boolean;
  linked: boolean;
  nullifierAHex: string;
  nullifierBHex: string;
  nullifierAKnownLocally: boolean;
  nullifierBKnownLocally: boolean;
}> {
  return request('/api/zk/link-pseudonyms', {
    method: 'POST',
    body: JSON.stringify({ topicA, topicB, proof, publicSignals }),
  });
}

/** Verify a federation-attestation proof against the server's published root. */
export async function verifyFederationAttestation(
  root: string,
  federationContext: string,
  proof: unknown,
  publicSignals: string[],
): Promise<{ ok: boolean; nullifierHex: string; root: string }> {
  return request('/api/zk/federation-attestation', {
    method: 'POST',
    body: JSON.stringify({ root, federationContext, proof, publicSignals }),
  });
}

export async function getIdentityInfo(
  pseudonymId: string,
): Promise<IdentityInfo> {
  return request<IdentityInfo>(`/api/identity/${pseudonymId}`, {
    headers: authHeaders(pseudonymId),
  });
}

export async function createInvite(
  pseudonymId: string,
  options?: { maxUses?: number; expiresInHours?: number },
): Promise<{ code: string; url: string; expiresAt?: string }> {
  return request<{ code: string; url: string; expiresAt?: string }>('/api/invites', {
    method: 'POST',
    headers: authHeaders(pseudonymId),
    body: JSON.stringify({
      maxUses: options?.maxUses,
      expiresInHours: options?.expiresInHours,
    }),
  });
}
