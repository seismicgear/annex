/**
 * Channel CRUD: list, create, join/leave, delete.
 */

import type { Channel } from '@/types';
import { authHeaders, request } from './core';
import { getE2eControls } from '@/lib/e2e-store';

export async function listChannels(pseudonymId: string): Promise<Channel[]> {
  return request<Channel[]>('/api/channels', {
    headers: authHeaders(pseudonymId),
  });
}

export async function getChannel(
  pseudonymId: string,
  channelId: string,
): Promise<Channel> {
  return request<Channel>(`/api/channels/${channelId}`, {
    headers: authHeaders(pseudonymId),
  });
}

export async function createChannel(
  pseudonymId: string,
  name: string,
  channelType: string,
  topic?: string,
  federated?: boolean,
): Promise<Channel> {
  // Generate a channel_id from the name (lowercase, alphanumeric + hyphens)
  const channel_id = name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-|-$/g, '')
    || `ch-${Date.now()}`;
  return request<Channel>('/api/channels', {
    method: 'POST',
    headers: authHeaders(pseudonymId),
    body: JSON.stringify({
      channel_id,
      name,
      channel_type: channelType,
      topic,
      federation_scope: federated ? 'Federated' : 'Local',
    }),
  });
}

export async function joinChannel(
  pseudonymId: string,
  channelId: string,
): Promise<void> {
  await request<unknown>(`/api/channels/${channelId}/join`, {
    method: 'POST',
    headers: authHeaders(pseudonymId),
  });
}

export async function leaveChannel(
  pseudonymId: string,
  channelId: string,
): Promise<void> {
  // The signed membership departure must precede the ordinary channel write.
  // Failed signing/trust keeps membership intact rather than leaving a ghost
  // recipient in the independently verified encryption directory.
  await getE2eControls(pseudonymId).leaveManifest(channelId);
  await request<unknown>(`/api/channels/${channelId}/leave`, {
    method: 'POST',
    headers: authHeaders(pseudonymId),
  });
}

export async function deleteChannel(
  pseudonymId: string,
  channelId: string,
): Promise<void> {
  await request<unknown>(`/api/channels/${channelId}`, {
    method: 'DELETE',
    headers: authHeaders(pseudonymId),
  });
}
