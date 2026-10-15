/** The self-service directory exposes no administrative identity metadata. */
import { authHeaders, request } from './core';

export interface DirectoryMember {
  pseudonym_id: string;
  participant_type: string;
}

export async function listDirectoryMembers(pseudonymId: string): Promise<DirectoryMember[]> {
  const response = await request<{ members: DirectoryMember[] }>('/api/members', {
    headers: authHeaders(pseudonymId),
  });
  return response.members;
}
