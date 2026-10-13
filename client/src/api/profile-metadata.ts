/** Explicit owner-scoped grants for additional graph profile metadata. */
import { authHeaders, getCredentialContext, isCredentialContextCurrent, request, StaleCredentialContextError } from './core';

export interface ProfileMetadataGrants {
  owner_pseudonym: string;
  metadata_json: string | null;
  grantees: string[];
}

function context() {
  const value = getCredentialContext();
  if (!value.pseudonymId) throw new Error('Sign in before managing profile sharing.');
  return value;
}

export async function listProfileMetadataGrants(): Promise<ProfileMetadataGrants> {
  const owner = context();
  const value = await request<unknown>('/api/graph/metadata-grants', { headers: authHeaders(owner.pseudonymId!) });
  if (!isCredentialContextCurrent(owner)) throw new StaleCredentialContextError('Profile sharing read');
  const result = value as Partial<ProfileMetadataGrants> | null;
  if (!result || result.owner_pseudonym !== owner.pseudonymId
    || !(result.metadata_json === null || typeof result.metadata_json === 'string')
    || !Array.isArray(result.grantees) || result.grantees.some(item => typeof item !== 'string' || !item || item === owner.pseudonymId)
    || new Set(result.grantees).size !== result.grantees.length) {
    throw new Error('The server returned an invalid profile sharing state.');
  }
  if (result.metadata_json !== null) {
    try { JSON.parse(result.metadata_json); }
    catch { throw new Error('The server returned invalid profile details. Sharing state was not loaded.'); }
  }
  return result as ProfileMetadataGrants;
}

export async function setProfileMetadataGrant(grantee: string, granted: boolean): Promise<void> {
  const owner = context();
  if (!grantee || grantee === owner.pseudonymId) throw new Error('Choose another participant.');
  const result = await request<{ granted?: unknown }>(`/api/graph/metadata-grants/${encodeURIComponent(grantee)}`, {
    method: granted ? 'PUT' : 'DELETE', headers: authHeaders(owner.pseudonymId!),
  });
  if (!isCredentialContextCurrent(owner)) throw new StaleCredentialContextError('Profile sharing change');
  if (result.granted !== granted) throw new Error('The server did not confirm the requested profile sharing change.');
}
