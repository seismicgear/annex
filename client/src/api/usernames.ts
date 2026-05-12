/**
 * Username profile management and visibility grants.
 */

import { authHeaders, request } from './core';

export async function setUsername(
  pseudonymId: string,
  username: string,
): Promise<{ status: string }> {
  return request<{ status: string }>('/api/profile/username', {
    method: 'PUT',
    headers: authHeaders(pseudonymId),
    body: JSON.stringify({ username }),
  });
}

export async function deleteUsername(
  pseudonymId: string,
): Promise<{ status: string }> {
  return request<{ status: string }>('/api/profile/username', {
    method: 'DELETE',
    headers: authHeaders(pseudonymId),
  });
}

export async function grantUsername(
  pseudonymId: string,
  granteePseudonym: string,
): Promise<{ status: string }> {
  return request<{ status: string }>('/api/profile/username/grant', {
    method: 'POST',
    headers: authHeaders(pseudonymId),
    body: JSON.stringify({ grantee_pseudonym: granteePseudonym }),
  });
}

export async function revokeUsernameGrant(
  pseudonymId: string,
  granteePseudonym: string,
): Promise<{ status: string }> {
  return request<{ status: string }>(`/api/profile/username/grant/${granteePseudonym}`, {
    method: 'DELETE',
    headers: authHeaders(pseudonymId),
  });
}

export async function listUsernameGrants(
  pseudonymId: string,
): Promise<{ grantees: string[] }> {
  return request<{ grantees: string[] }>('/api/profile/username/grants', {
    headers: authHeaders(pseudonymId),
  });
}

export async function getVisibleUsernames(
  pseudonymId: string,
): Promise<{ usernames: Record<string, string> }> {
  return request<{ usernames: Record<string, string> }>('/api/usernames/visible', {
    headers: authHeaders(pseudonymId),
  });
}
