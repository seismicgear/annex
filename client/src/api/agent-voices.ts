import { authHeaders, request } from './core';

export type VoiceBackend = 'piper' | 'bark' | 'system';
export interface AvailableVoiceProfile {
  id: string;
  name: string;
  model: VoiceBackend | null;
  speed: number | null;
  pitch: number | null;
  speaker_id: number | null;
  available: boolean;
}
export interface AgentVoiceAssignment {
  pseudonym_id: string;
  voice_profile_id: string | null;
  voice_backend: VoiceBackend | null;
}

export function listVoiceProfiles(pseudonymId: string) {
  return request<{ profiles: AvailableVoiceProfile[] }>('/api/voice-profiles', {
    headers: authHeaders(pseudonymId),
  });
}

export function getAgentVoice(pseudonymId: string, agentId: string) {
  return request<AgentVoiceAssignment>(`/api/agents/${encodeURIComponent(agentId)}`, {
    headers: authHeaders(pseudonymId),
  });
}

export function assignAgentVoice(pseudonymId: string, agentId: string, profileId: string | null) {
  return request<{ status: string }>(`/api/agents/${encodeURIComponent(agentId)}/voice-profile`, {
    method: 'PUT', headers: authHeaders(pseudonymId),
    body: JSON.stringify({ voice_profile_id: profileId }),
  });
}
