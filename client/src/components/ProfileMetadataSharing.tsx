import { useCallback, useEffect, useRef, useState } from 'react';
import { getCredentialContext, isCredentialContextCurrent } from '@/api/core';
import { listDirectoryMembers, type DirectoryMember } from '@/api/members';
import { listProfileMetadataGrants, setProfileMetadataGrant, type ProfileMetadataGrants } from '@/api/profile-metadata';
import { useChannelsStore } from '@/stores/channels';

const detail = (error: unknown) => error instanceof Error && error.message.trim() ? error.message : 'The server did not confirm the request.';

/** Kept distinct from local personas and the separately granted username. */
export function ProfileMetadataSharing({ pseudonymId }: { pseudonymId: string }) {
  const session = useChannelsStore(state => state.wsSessionState);
  const blocked = session === 'taken_over' || session === 'revoked';
  const [opened, setOpened] = useState(false);
  const [snapshot, setSnapshot] = useState<ProfileMetadataGrants | null>(null);
  const [members, setMembers] = useState<DirectoryMember[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [membersError, setMembersError] = useState<string | null>(null);
  const [pending, setPending] = useState<{ grantee: string; granted: boolean } | null>(null);
  const [saving, setSaving] = useState(false);
  const [success, setSuccess] = useState<string | null>(null);
  const generation = useRef(0);
  useEffect(() => () => { generation.current++; }, []);

  const load = useCallback(async () => {
    const request = ++generation.current;
    const context = getCredentialContext();
    if (context.pseudonymId !== pseudonymId) return;
    const current = () => request === generation.current && isCredentialContextCurrent(context);
    setLoading(true); setError(null); setMembersError(null); setSnapshot(null); setSuccess(null); setPending(null);
    const [grants, directory] = await Promise.allSettled([listProfileMetadataGrants(), listDirectoryMembers(pseudonymId)]);
    if (!current()) return;
    if (grants.status === 'fulfilled') setSnapshot(grants.value);
    else setError(`Could not load profile sharing: ${detail(grants.reason)}`);
    if (directory.status === 'fulfilled') setMembers(directory.value.filter(member => member.pseudonym_id !== pseudonymId));
    else { setMembers([]); setMembersError(`Could not load people: ${detail(directory.reason)}`); }
    setLoading(false);
  }, [pseudonymId]);

  const confirm = async () => {
    if (!pending || saving || blocked) return;
    const change = pending;
    const request = ++generation.current;
    const context = getCredentialContext();
    if (context.pseudonymId !== pseudonymId) return;
    const current = () => request === generation.current && isCredentialContextCurrent(context);
    setSaving(true); setError(null); setSuccess(null);
    let acknowledged = false;
    try {
      await setProfileMetadataGrant(change.grantee, change.granted);
      acknowledged = true;
      const verified = await listProfileMetadataGrants();
      if (!current()) return;
      if (verified.grantees.includes(change.grantee) !== change.granted) throw new Error('The current grant list does not match the requested change.');
      setSnapshot(verified); setPending(null);
      setSuccess(change.granted ? 'Profile details sharing granted.' : 'Profile details sharing revoked.');
    } catch (failure) {
      if (!current()) return;
      // The mutation may have committed even if the reply was lost. Do not
      // keep presenting the previous privacy state as if it were current.
      setSnapshot(null); setPending(null);
      setError(`${acknowledged ? 'The change was acknowledged, but the current sharing state could not be confirmed.' : 'The profile sharing change could not be confirmed.'} ${detail(failure)}`);
    } finally { if (current()) setSaving(false); }
  };

  const roster = new Map(members.map(member => [member.pseudonym_id, member]));
  for (const grantee of snapshot?.grantees ?? []) if (!roster.has(grantee)) roster.set(grantee, { pseudonym_id: grantee, participant_type: '' });
  const emptyDetails = snapshot?.metadata_json === null || snapshot?.metadata_json?.trim() === 'null';
  return (
    <section className="policy-section profile-metadata-sharing" aria-label="Profile details sharing" style={{ marginTop: '1rem' }}>
      <h3>Profile details sharing</h3>
      <p className="field-hint">Additional profile details stored by this server are private by default. Choose who may read them while your profiles are connected in the server graph.</p>
      <p className="field-hint">A grant includes current details and future updates until you revoke it. Local personas and usernames have their own visibility controls.</p>
      {!opened ? <button type="button" onClick={() => { setOpened(true); void load(); }}>Manage profile sharing</button> : <>
        <button type="button" onClick={() => void load()} disabled={loading || saving}>Refresh profile sharing</button>
        {blocked && <p role="status">Reconnect this device before changing profile sharing.</p>}
        {loading && <p role="status">Loading profile sharing…</p>}
        {error && <div role="alert"><p>{error}</p><button type="button" onClick={() => void load()} disabled={loading || saving}>Retry profile sharing</button></div>}
        {membersError && <div role="alert"><p>{membersError}</p><button type="button" onClick={() => void load()} disabled={loading || saving}>Retry people</button></div>}
        {success && <p role="status">{success}</p>}
        {snapshot && <>
          {emptyDetails ? <p>No additional profile details are stored for this identity.</p> : <details open>
            <summary>Current profile details (JSON)</summary>
            <pre aria-label="Current profile details" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{snapshot.metadata_json}</pre>
          </details>}
          {[...roster.values()].map(member => {
            const granted = snapshot.grantees.includes(member.pseudonym_id);
            return <div className="metadata-sharing-person" key={member.pseudonym_id}>
              <p><span title={member.pseudonym_id}>{member.pseudonym_id}</span> · {member.participant_type || 'Not in the active member list'} · {granted ? 'Sharing granted' : 'Not shared'}</p>
              <button type="button" disabled={saving || loading || blocked || !!pending}
                onClick={() => { setSuccess(null); setPending({ grantee: member.pseudonym_id, granted: !granted }); }}>
                {granted ? 'Revoke profile sharing' : 'Grant profile sharing'}
              </button>
            </div>;
          })}
          {!roster.size && !membersError && <p>No other active participants are available.</p>}
        </>}
        {pending && <section role="region" aria-label="Confirm profile sharing change">
          <h4>{pending.granted ? 'Grant profile sharing?' : 'Revoke profile sharing?'}</h4>
          <p>{pending.grantee}</p>
          <p>{pending.granted
            ? 'This person will be allowed to read your current additional profile details and future updates while your profiles remain visible to each other. The grant does not expand graph visibility.'
            : 'This person will no longer be allowed to read your additional profile details through this grant. Information they already received cannot be recalled.'}</p>
          {pending.granted && emptyDetails && <p>There are no additional details now. This grant will also cover details added in the future.</p>}
          <button type="button" onClick={() => setPending(null)} disabled={saving}>Cancel sharing change</button>
          <button type="button" onClick={() => void confirm()} disabled={saving || blocked}>{saving ? 'Saving sharing change…' : 'Confirm sharing change'}</button>
        </section>}
      </>}
    </section>
  );
}
