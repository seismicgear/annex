/**
 * Admin endpoints: server policy, server settings (label, public URLs),
 * and member capability management.
 */

import type { ServerPolicy } from '@/types';
import { authHeaders, request } from './core';

export async function getPolicy(
  pseudonymId: string,
): Promise<ServerPolicy> {
  return request<ServerPolicy>('/api/admin/policy', {
    headers: authHeaders(pseudonymId),
  });
}

export async function updatePolicy(
  pseudonymId: string,
  policy: ServerPolicy,
): Promise<{ status: string; version_id: string }> {
  return request<{ status: string; version_id: string }>('/api/admin/policy', {
    method: 'PUT',
    headers: authHeaders(pseudonymId),
    body: JSON.stringify(policy),
  });
}

// ── Storage health gate (ADR-0009) ──

/**
 * The storage gate's current state.
 *
 * `degraded` means the server is answering every mutating request with a 507
 * and will keep doing so until an operator clears it — there is no automatic
 * recovery, by design. Both this and the clear below existed on the server
 * with no caller here, so from the UI the only recovery was still a process
 * restart.
 */
export interface StorageHealth {
  /** `"healthy"`, `"warn"`, or `"degraded"`. */
  state: string;
  /** Why the gate left `healthy`. Empty while healthy. */
  reason: string;
  /** True while mutating requests are being rejected with HTTP 507. */
  writes_blocked: boolean;
}

export async function getStorageHealth(pseudonymId: string): Promise<StorageHealth> {
  return request<StorageHealth>('/api/admin/storage', {
    headers: authHeaders(pseudonymId),
  });
}

export async function clearStorageGate(
  pseudonymId: string,
): Promise<{ status: string; previous_state: string; state: string }> {
  return request<{ status: string; previous_state: string; state: string }>(
    '/api/admin/storage/clear',
    { method: 'POST', headers: authHeaders(pseudonymId) },
  );
}

// ── Server Settings ──

export async function getServer(
  pseudonymId: string,
): Promise<{ slug: string; label: string; public_url: string }> {
  return request<{ slug: string; label: string; public_url: string }>('/api/admin/server', {
    headers: authHeaders(pseudonymId),
  });
}

export async function renameServer(
  pseudonymId: string,
  label: string,
): Promise<{ status: string; label: string }> {
  return request<{ status: string; label: string }>('/api/admin/server', {
    method: 'PATCH',
    headers: authHeaders(pseudonymId),
    body: JSON.stringify({ label }),
  });
}

export async function setPublicUrl(
  pseudonymId: string,
  publicUrl: string,
): Promise<{ status: string; public_url: string }> {
  return request<{ status: string; public_url: string }>('/api/admin/public-url', {
    method: 'PUT',
    headers: authHeaders(pseudonymId),
    body: JSON.stringify({ public_url: publicUrl }),
  });
}

export async function setWebrtcPublicUrl(
  pseudonymId: string,
  publicWebrtcUrl: string,
): Promise<{ status: string; public_webrtc_url: string }> {
  return request<{ status: string; public_webrtc_url: string }>('/api/admin/webrtc-public-url', {
    method: 'PUT',
    headers: authHeaders(pseudonymId),
    body: JSON.stringify({ public_webrtc_url: publicWebrtcUrl }),
  });
}

// ── Member Management ──

export interface MemberInfo {
  pseudonym_id: string;
  participant_type: string;
  can_voice: boolean;
  can_moderate: boolean;
  can_invite: boolean;
  can_federate: boolean;
  can_bridge: boolean;
  active: boolean;
  created_at: string;
}

export async function listMembers(
  pseudonymId: string,
): Promise<MemberInfo[]> {
  const resp = await request<{ members: MemberInfo[] }>('/api/admin/members', {
    headers: authHeaders(pseudonymId),
  });
  return resp.members;
}

export async function updateMemberCapabilities(
  pseudonymId: string,
  targetPseudonym: string,
  caps: {
    can_voice: boolean;
    can_moderate: boolean;
    can_invite: boolean;
    can_federate: boolean;
    can_bridge: boolean;
  },
): Promise<void> {
  await request<unknown>(`/api/admin/members/${targetPseudonym}/capabilities`, {
    method: 'PATCH',
    headers: authHeaders(pseudonymId),
    body: JSON.stringify(caps),
  });
}
