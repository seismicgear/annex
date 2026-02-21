/**
 * HTTP API client for the Annex server.
 *
 * All endpoints are accessed through this module. The Vite dev server
 * proxies /api/* and /events/* to the backend at port 3000.
 */

import type {
  RegistrationResponse,
  VerifyMembershipResponse,
  IdentityInfo,
  Channel,
  Message,
  ServerSummary,
  ServerPolicy,
  FederationPeer,
  AgentInfo,
  PublicEvent,
} from '@/types';

/** Base error class for API responses. */
export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

/**
 * Active base URL for multi-server connections.
 * Empty string = current origin (relative paths). Otherwise, full URL prefix.
 */
let _apiBaseUrl = '';

/** Set the API base URL for cross-server requests. Empty string for current origin. */
export function setApiBaseUrl(baseUrl: string): void {
  _apiBaseUrl = baseUrl.replace(/\/+$/, '');
}

/** Get the current API base URL. */
export function getApiBaseUrl(): string {
  return _apiBaseUrl;
}

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const url = _apiBaseUrl ? `${_apiBaseUrl}${path}` : path;
  const res = await fetch(url, {
    ...options,
    headers: {
      'Content-Type': 'application/json',
      ...options?.headers,
    },
  });
  if (!res.ok) {
    const body = await res.text();
    throw new ApiError(res.status, body);
  }
  return res.json() as Promise<T>;
}

/**
 * Fetch from a specific remote server (for federation hopping / discovery).
 * Does NOT use the global _apiBaseUrl — targets the given URL directly.
 */
export async function requestRemote<T>(
  baseUrl: string,
  path: string,
  options?: RequestInit,
): Promise<T> {
  const url = `${baseUrl.replace(/\/+$/, '')}${path}`;
  const res = await fetch(url, {
    ...options,
    headers: {
      'Content-Type': 'application/json',
      ...options?.headers,
    },
  });
  if (!res.ok) {
    const body = await res.text();
    throw new ApiError(res.status, body);
  }
  return res.json() as Promise<T>;
}

function authHeaders(pseudonymId: string): Record<string, string> {
  return { 'X-Annex-Pseudonym': pseudonymId };
}

// ── Identity & Registration ──

export async function register(
  commitmentHex: string,
  roleCode: number,
  nodeId: number,
): Promise<RegistrationResponse> {
  return request<RegistrationResponse>('/api/registry/register', {
    method: 'POST',
    body: JSON.stringify({ commitmentHex, roleCode, nodeId }),
  });
}

export async function verifyMembership(
  root: string,
  commitment: string,
  topic: string,
  proof: unknown,
  publicSignals: string[],
): Promise<VerifyMembershipResponse> {
  return request<VerifyMembershipResponse>('/api/zk/verify-membership', {
    method: 'POST',
    body: JSON.stringify({ root, commitment, topic, proof, publicSignals }),
  });
}

export async function getIdentityInfo(
  pseudonymId: string,
): Promise<IdentityInfo> {
  return request<IdentityInfo>(`/api/identity/${pseudonymId}`, {
    headers: authHeaders(pseudonymId),
  });
}

// ── Channels ──

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
  await request<unknown>(`/api/channels/${channelId}/leave`, {
    method: 'POST',
    headers: authHeaders(pseudonymId),
  });
}

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

// ── Public APIs (no auth required) ──

export async function getServerSummary(): Promise<ServerSummary> {
  return request<ServerSummary>('/api/public/server/summary');
}

export async function getFederationPeers(): Promise<{ peers: FederationPeer[] }> {
  return request<{ peers: FederationPeer[] }>('/api/public/federation/peers');
}

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

// ── Admin ──

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

export async function deleteChannel(
  pseudonymId: string,
  channelId: string,
): Promise<void> {
  await request<unknown>(`/api/channels/${channelId}`, {
    method: 'DELETE',
    headers: authHeaders(pseudonymId),
  });
}

// ── Remote Server Discovery (federation hopping) ──

export async function getRemoteServerSummary(
  baseUrl: string,
): Promise<ServerSummary> {
  return requestRemote<ServerSummary>(baseUrl, '/api/public/server/summary');
}

export async function getRemoteFederationPeers(
  baseUrl: string,
): Promise<{ peers: FederationPeer[] }> {
  return requestRemote<{ peers: FederationPeer[] }>(baseUrl, '/api/public/federation/peers');
}

// ── Voice ──

export async function joinVoice(
  pseudonymId: string,
  channelId: string,
): Promise<{ token: string; url: string }> {
  return request<{ token: string; url: string }>(
    `/api/channels/${channelId}/voice/join`,
    {
      method: 'POST',
      headers: authHeaders(pseudonymId),
    },
  );
}

export async function leaveVoice(
  pseudonymId: string,
  channelId: string,
): Promise<void> {
  await request<unknown>(`/api/channels/${channelId}/voice/leave`, {
    method: 'POST',
    headers: authHeaders(pseudonymId),
  });
}

export async function getVoiceStatus(
  pseudonymId: string,
  channelId: string,
): Promise<{ participants: number; active: boolean }> {
  return request<{ participants: number; active: boolean }>(
    `/api/channels/${channelId}/voice/status`,
    { headers: authHeaders(pseudonymId) },
  );
}
