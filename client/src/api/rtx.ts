/**
 * Public observability endpoints surfacing the RTX agent ecosystem:
 * agent listings and the public event log.
 */

import type { AgentInfo, PublicEvent } from '@/types';
import { request } from './core';

export async function getPublicAgents(): Promise<{ agents: AgentInfo[] }> {
  return request<{ agents: AgentInfo[] }>('/api/public/agents');
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
  const resp = await request<{ events: PublicEvent[]; count: number }>(
    `/api/public/events${qs ? '?' + qs : ''}`,
  );
  return resp.events;
}
