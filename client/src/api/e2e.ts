/**
 * Content-blind E2E channel key directory API.
 *
 * Thin wrappers over the server endpoints in `crates/annex-server/src/api_e2e.rs`.
 * The server only ever stores public keys and opaque sealed blobs; all sealing /
 * opening happens client-side in `@/lib/e2e`.
 */

import { authHeaders, request } from './core';

export interface MemberKey {
  pseudonym_id: string;
  x25519_pub_hex: string;
}

export interface KeyWrapRecord {
  epoch: number;
  sender_pseudonym_id: string;
  wrapped_key_b64: string;
}

export interface KeyWrapUpload {
  recipient_pseudonym_id: string;
  wrapped_key_b64: string;
}

/** Publish (upsert) my own X25519 public key into the directory. */
export async function publishMyKey(pseudonymId: string, x25519PubHex: string): Promise<void> {
  await request<unknown>('/api/keys/me', {
    method: 'PUT',
    headers: authHeaders(pseudonymId),
    body: JSON.stringify({ x25519_pub_hex: x25519PubHex }),
  });
}

/** Fetch a single member's advertised public key. */
export async function getMemberKey(pseudonymId: string, target: string): Promise<MemberKey> {
  return request<MemberKey>(`/api/keys/${encodeURIComponent(target)}`, {
    headers: authHeaders(pseudonymId),
  });
}

/** The public-key directory for everyone in a channel (members only). */
export async function getChannelMemberKeys(
  pseudonymId: string,
  channelId: string,
): Promise<MemberKey[]> {
  const r = await request<{ member_keys: MemberKey[] }>(
    `/api/channels/${encodeURIComponent(channelId)}/member-keys`,
    { headers: authHeaders(pseudonymId) },
  );
  return r.member_keys;
}

/** Upload sealed channel-key blobs for members. First write per recipient wins. */
export async function postChannelKeyWraps(
  pseudonymId: string,
  channelId: string,
  epoch: number,
  wraps: KeyWrapUpload[],
): Promise<number> {
  const r = await request<{ inserted: number }>(
    `/api/channels/${encodeURIComponent(channelId)}/key-wraps`,
    {
      method: 'POST',
      headers: authHeaders(pseudonymId),
      body: JSON.stringify({ epoch, wraps }),
    },
  );
  return r.inserted;
}

/** The sealed channel-key blobs addressed to me (only mine). */
export async function getChannelKeyWraps(
  pseudonymId: string,
  channelId: string,
): Promise<KeyWrapRecord[]> {
  const r = await request<{ wraps: KeyWrapRecord[] }>(
    `/api/channels/${encodeURIComponent(channelId)}/key-wraps`,
    { headers: authHeaders(pseudonymId) },
  );
  return r.wraps;
}

/** Whether the channel already holds sealed key material, and the top epoch. */
export async function getChannelKeyStatus(
  pseudonymId: string,
  channelId: string,
): Promise<{ has_key: boolean; max_epoch: number }> {
  return request<{ has_key: boolean; max_epoch: number }>(
    `/api/channels/${encodeURIComponent(channelId)}/key-status`,
    { headers: authHeaders(pseudonymId) },
  );
}

/** Whether a channel is end-to-end encrypted. */
export async function getChannelE2e(pseudonymId: string, channelId: string): Promise<boolean> {
  const r = await request<{ e2e_enabled: boolean }>(
    `/api/channels/${encodeURIComponent(channelId)}/e2e`,
    { headers: authHeaders(pseudonymId) },
  );
  return r.e2e_enabled;
}

/** Toggle E2E on a channel (requires moderation capability server-side). */
export async function setChannelE2e(
  pseudonymId: string,
  channelId: string,
  enabled: boolean,
): Promise<void> {
  await request<unknown>(`/api/channels/${encodeURIComponent(channelId)}/e2e`, {
    method: 'PUT',
    headers: authHeaders(pseudonymId),
    body: JSON.stringify({ enabled }),
  });
}
