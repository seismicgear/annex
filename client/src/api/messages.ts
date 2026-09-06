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

/**
 * What a search covered, alongside what it found.
 *
 * Message bodies are encrypted at rest, so the server cannot match them in
 * SQL. It decrypts a bounded recent window per channel and filters there,
 * which means an empty `results` has two very different causes: the term is
 * not in the archive, or the term is older than the window. `complete`
 * separates them, and `scanned_per_channel` is how far back the window
 * reached — reported by the server so the client never has to name a number
 * the server owns.
 */
export interface MessageSearchResult {
  results: Message[];
  complete: boolean;
  scanned_per_channel: number;
}

export async function searchMessages(
  pseudonymId: string,
  query: string,
  channelId?: string,
  limit?: number,
): Promise<MessageSearchResult> {
  const params = new URLSearchParams({ q: query });
  if (channelId) params.set('channel_id', channelId);
  if (limit) params.set('limit', limit.toString());
  return request<MessageSearchResult>(
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
