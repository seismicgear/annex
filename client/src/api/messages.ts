/**
 * Message reads: paginated history, full-text search, and edit history.
 */

import type { Message, MessageEdit } from '@/types';
import { authHeaders, request } from './core';

export async function getMessages(
  pseudonymId: string,
  channelId: string,
  before?: string,
  limit?: number,
): Promise<Message[]> {
  const params = new URLSearchParams();
  if (before) params.set('before', before);
  if (limit) params.set('limit', limit.toString());
  const qs = params.toString();
  return request<Message[]>(
    `/api/channels/${channelId}/messages${qs ? '?' + qs : ''}`,
    { headers: authHeaders(pseudonymId) },
  );
}

export async function searchMessages(
  pseudonymId: string,
  query: string,
  channelId?: string,
  limit?: number,
): Promise<Message[]> {
  const params = new URLSearchParams({ q: query });
  if (channelId) params.set('channel_id', channelId);
  if (limit) params.set('limit', limit.toString());
  return request<Message[]>(
    `/api/messages/search?${params}`,
    { headers: authHeaders(pseudonymId) },
  );
}

export async function getMessageEdits(
  pseudonymId: string,
  channelId: string,
  messageId: string,
): Promise<MessageEdit[]> {
  return request<MessageEdit[]>(
    `/api/channels/${channelId}/messages/${messageId}/edits`,
    { headers: authHeaders(pseudonymId) },
  );
}
