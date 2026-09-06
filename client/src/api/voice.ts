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
  /** Whether a (possibly loopback-only) URL exists for local clients. */
  has_local_url?: boolean;
  /**
   * Whether the whisper.cpp binary AND GGML model file are both present
   * on disk. The Docker image ships the binary but does NOT bundle a
   * model — operators must mount one and set ANNEX_STT_MODEL_PATH.
   * Older servers may not include this field; treat undefined as
   * "unknown" rather than "ready".
   */
  stt_ready?: boolean;
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

export interface VoiceStatus {
  participants: number;
  /** Pseudonyms of everyone currently in the call, sorted. */
  participant_ids: string[];
  active: boolean;
}

export async function getVoiceStatus(
  pseudonymId: string,
  channelId: string,
): Promise<VoiceStatus> {
  const url = voiceUrl(channelId, 'status');
  const resp = await fetch(url, {
    headers: new Headers(authHeaders(pseudonymId)),
  });
  if (!resp.ok) {
    const body = await resp.text();
    throw new ApiError(resp.status, body);
  }
  const body = (await resp.json()) as Partial<VoiceStatus>;
  // A server predating the roster sends only the count. Default rather than
  // let `undefined` reach a `.map` in the participant grid.
  return {
    participants: body.participants ?? 0,
    participant_ids: body.participant_ids ?? [],
    active: body.active ?? false,
  };
}

/** Get the server's voice (WebRTC) configuration status (public, no auth). */
export async function getVoiceConfigStatus(): Promise<VoiceConfigStatus> {
  return request<VoiceConfigStatus>('/api/voice/config-status');
}
