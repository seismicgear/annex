/**
 * Voice (WebRTC) endpoints: join/leave, status, and config probe.
 */

import {
  ApiError,
  authHeaders,
  fetchWithTimeout,
  getApiBaseUrl,
  request,
} from './core';

export interface IceServerConfig {
  urls: string[];
  username?: string;
  credential?: string;
}

export interface JoinVoiceResponse {
  token: string;
  url: string;
  ice_servers: IceServerConfig[];
}

export interface VoiceConfigStatus {
  voice_enabled: boolean;
  /** Whether voice is enabled in the server policy (admin toggle). */
  policy_enabled: boolean;
  /** Whether the WebRTC infrastructure is configured and reachable. */
  infrastructure_ready: boolean;
  has_public_url: boolean;
  setup_hint: string;
}

/** Build the full URL for a voice endpoint, using the active base URL. */
function voiceUrl(channelId: string, action: 'join' | 'leave' | 'status'): string {
  const path = `/api/channels/${channelId}/voice/${action}`;
  const baseUrl = getApiBaseUrl();
  return baseUrl ? `${baseUrl}${path}` : path;
}

export async function joinVoice(
  pseudonymId: string,
  channelId: string,
  timeoutMs?: number,
): Promise<JoinVoiceResponse> {
  const url = voiceUrl(channelId, 'join');
  const resp = await fetchWithTimeout(url, {
    method: 'POST',
    headers: new Headers({
      ...authHeaders(pseudonymId),
      'Content-Type': 'application/json',
    }),
  }, timeoutMs);
  if (!resp.ok) {
    const body = await resp.text();
    throw new ApiError(resp.status, body);
  }
  return resp.json() as Promise<JoinVoiceResponse>;
}

export async function leaveVoice(
  pseudonymId: string,
  channelId: string,
): Promise<void> {
  const url = voiceUrl(channelId, 'leave');
  const resp = await fetch(url, {
    method: 'POST',
    headers: new Headers(authHeaders(pseudonymId)),
  });
  if (!resp.ok) {
    const body = await resp.text();
    throw new ApiError(resp.status, body);
  }
}

export async function getVoiceStatus(
  pseudonymId: string,
  channelId: string,
): Promise<{ participants: number; active: boolean }> {
  const url = voiceUrl(channelId, 'status');
  const resp = await fetch(url, {
    headers: new Headers(authHeaders(pseudonymId)),
  });
  if (!resp.ok) {
    const body = await resp.text();
    throw new ApiError(resp.status, body);
  }
  return resp.json() as Promise<{ participants: number; active: boolean }>;
}

/** Get the server's voice (WebRTC) configuration status (public, no auth). */
export async function getVoiceConfigStatus(): Promise<VoiceConfigStatus> {
  return request<VoiceConfigStatus>('/api/voice/config-status');
}
