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
