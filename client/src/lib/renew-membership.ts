import { getCredentialContext, getSessionToken, getZkProofPayload, isCredentialContextCurrent, setZkProofPayload, StaleCredentialContextError } from '@/api/core';
import { getMerklePath } from '@/api/identity';
import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';
import { saveIdentity } from './db';
import { assertSessionOwnership } from './session-ownership';
import { generateMembershipProofV2, isProofGenerationInFlight } from './zk';

/** Refresh only the asserted tree membership, never the authenticated session.
 * The old fifth public signal is already spent. Reusing it for an assertion
 * cannot authenticate again because no verify-membership request is made.
 */
export async function renewMembershipAssertion(): Promise<void> {
  const context = getCredentialContext(), token = getSessionToken(), previous = getZkProofPayload();
  const identity = useIdentityStore.getState().identity;
  if (!identity?.sk || identity.pseudonymId !== context.pseudonymId || !token || !previous) throw new Error('No active identity proof is available to renew.');
  const check = () => {
    const current = useIdentityStore.getState();
    if (!isCredentialContextCurrent(context) || getSessionToken() !== token || current.identity?.id !== identity.id
      || current.identity.pseudonymId !== identity.pseudonymId || current.phase !== 'ready'
      || useChannelsStore.getState().wsSessionState === 'revoked') throw new StaleCredentialContextError('Membership assertion renewal');
    assertSessionOwnership(context.baseUrl, identity.pseudonymId!);
  };
  check();
  const cached = JSON.parse(previous) as { protocolVersion?: string; publicSignals?: string[]; topic?: string; commitment_hex?: string };
  const topic = `annex:server:${identity.serverSlug}:v2`;
  if (cached.protocolVersion !== 'v2' || cached.topic !== topic || cached.commitment_hex !== identity.commitmentHex
    || !Array.isArray(cached.publicSignals) || cached.publicSignals.length !== 5
    || cached.publicSignals.some(signal => typeof signal !== 'string' || !/^(0|[1-9][0-9]*)$/.test(signal))) throw new Error('The cached membership assertion is invalid.');
  if (isProofGenerationInFlight()) throw new Error('Another identity proof is running. Retry this action when it finishes.');
  const path = await getMerklePath(identity.commitmentHex); check();
  const challenge = BigInt(cached.publicSignals[4]).toString(16).padStart(64, '0');
  const next = await generateMembershipProofV2({ sk: BigInt(`0x${identity.sk}`), roleCode: identity.roleCode,
    nodeId: identity.nodeId, leafIndex: path.leafIndex, pathElements: path.pathElements, pathIndexBits: path.pathIndexBits }, topic, challenge);
  check();
  if (next.publicSignals.length !== 5 || BigInt(next.publicSignals[0]) !== BigInt(`0x${path.rootHex}`)
    || BigInt(next.publicSignals[1]) !== BigInt(`0x${identity.commitmentHex}`)
    || next.publicSignals.slice(2).some((signal, index) => signal !== cached.publicSignals![index + 2])) throw new Error('The renewed membership assertion changed its identity binding.');
  const payload = JSON.stringify({ proof: next.proof, root_hex: path.rootHex, commitment_hex: identity.commitmentHex,
    protocolVersion: 'v2', publicSignals: next.publicSignals, topic });
  const updated = { ...useIdentityStore.getState().identity!, leafIndex: path.leafIndex, zkProofPayload: payload };
  await saveIdentity(updated); check();
  setZkProofPayload(payload);
  useIdentityStore.setState({ identity: updated });
}
