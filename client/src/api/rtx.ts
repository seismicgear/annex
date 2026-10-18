/**
 * Public observability endpoints surfacing the RTX agent ecosystem:
 * agent listings and the public event log.
 */

import type { AgentInfo, PublicEvent } from '@/types';
import { request } from './core';

/**
 * Pull a list out of a wrapper object without trusting its shape.
 *
 * These endpoints return `{ items: [...] }` envelopes, and the client used to
 * index straight into them. When a response did not carry the expected field —
 * a proxy returning its own JSON, a version skew, a federated server with a
 * different envelope — the caller got `undefined` and the panel crashed on
 * `.length` or `.map`, taking the whole view down with an uncaught TypeError.
 *
 * The WebSocket layer already refuses to fail closed on unrecognised frames
 * (see invariant I-WS-1); this brings the HTTP layer in line. A malformed
 * response degrades to "nothing to show" rather than a blank screen.
 */
function listFrom<T>(resp: unknown, key: string): T[] {
  if (resp && typeof resp === 'object') {
    const value = (resp as Record<string, unknown>)[key];
    if (Array.isArray(value)) return value as T[];
  }
  // A bare array is also accepted: harmless to support, and it is the shape a
  // hand-rolled proxy or fixture is most likely to return.
  if (Array.isArray(resp)) return resp as T[];
  console.warn(`[api] expected an array at "${key}" in the response; got`, resp);
  return [];
}

export async function getPublicAgents(): Promise<{ agents: AgentInfo[] }> {
  const resp = await request<unknown>('/api/public/agents');
  const agents = listFrom<Record<string, unknown>>(resp, 'agents').map((entry): AgentInfo => {
    const contract = entry?.capability_contract;
    // Public observations expose the negotiated contract. Older endpoints
    // may already flatten its offered capabilities; preserve that shape too.
    const capabilities = contract && typeof contract === 'object'
      ? (contract as Record<string, unknown>).offered_capabilities : entry?.capabilities;
    if (!entry || typeof entry.pseudonym_id !== 'string'
      || typeof entry.alignment_status !== 'string' || typeof entry.transfer_scope !== 'string'
      || typeof entry.reputation_score !== 'number' || !Number.isFinite(entry.reputation_score)
      || !Array.isArray(capabilities) || !capabilities.every(value => typeof value === 'string')) {
      throw new Error('The server returned incomplete agent details. Try refreshing the member list.');
    }
    return {
      pseudonym_id: entry.pseudonym_id,
      alignment_status: entry.alignment_status as AgentInfo['alignment_status'],
      transfer_scope: entry.transfer_scope as AgentInfo['transfer_scope'],
      reputation_score: entry.reputation_score,
      capabilities,
      active: true,
    };
  });
  return { agents };
}

export async function getPublicEvents(
  domain?: string,
  since?: number,
  limit?: number,
): Promise<PublicEvent[]> {
  const params = new URLSearchParams();
  if (domain) params.set('domain', domain);
  if (since) params.set('since', since.toString());
  if (limit) params.set('limit', limit.toString());
  const qs = params.toString();
  const resp = await request<unknown>(`/api/public/events${qs ? '?' + qs : ''}`);
  return listFrom<PublicEvent>(resp, 'events');
}
