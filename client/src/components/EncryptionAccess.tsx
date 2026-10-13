import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { getE2eControls } from '@/lib/e2e-store';
import { getVerifiedChannel, subscribeVerifiedChannels } from '@/lib/verified-channel';
import type { DeviceCertificate } from '@/lib/device-certificate';
import type { JoinPolicy } from '@/lib/e2e-manifest';
import { useChannelsStore } from '@/stores/channels';
import { Modal } from './Modal';

type Confirmation = { kind: 'remove'; pseudonym: string } | { kind: 'role'; pseudonym: string; role: 'member' | 'moderator' };

export function EncryptionAccess({ channelId, pseudonym, onClose }: { channelId: string; pseudonym: string; onClose: () => void }) {
  const view = useSyncExternalStore(subscribeVerifiedChannels, () => getVerifiedChannel(pseudonym, channelId));
  const session = useChannelsStore(state => state.wsSessionState);
  const retry = useChannelsStore(state => state.retryChannelKey);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [policy, setPolicy] = useState<JoinPolicy>(view?.joinPolicy ?? 'eligible_self_join');
  const [candidate, setCandidate] = useState('');
  const [devices, setDevices] = useState<DeviceCertificate[]>([]);
  const [selectedDevice, setSelectedDevice] = useState('');
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const alive = useRef(true);
  const isModerator = view?.trusted && view.members.some(member => member.pseudonym === pseudonym && member.role === 'moderator');
  const blocked = busy || session === 'taken_over' || session === 'revoked';
  const currentPolicy = view?.joinPolicy;
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { if (currentPolicy) setPolicy(currentPolicy); }, [currentPolicy]);
  const perform = async (action: () => Promise<void>) => {
    if (blocked) return;
    setBusy(true); setError(null);
    try { await action(); }
    catch (reason) { if (alive.current) setError(reason instanceof Error && reason.message ? reason.message : 'The encryption access change failed. Retry.'); }
    finally { if (alive.current) setBusy(false); }
  };
  const refresh = async () => { await getE2eControls(pseudonym).inspectChannel(channelId); };
  const changed = async () => { await refresh(); await retry(pseudonym); };
  return <Modal label="Encryption access" onClose={() => { if (!busy) onClose(); }}>
    <h2>Encryption access</h2>
    {view?.trusted && view.genesisHash ? <>
      <p>These fingerprints identify the signed channel history verified on this device. Compare them with a member before trusting a new device.</p>
      <dl className="encryption-fingerprints">
        <dt>Authority</dt><dd><code className="encryption-fingerprint">{view.authorityIdentity}</code></dd>
        <dt>Channel fingerprint</dt><dd><code className="encryption-fingerprint">{view.genesisHash}</code></dd>
      </dl>
      <p>History access: current admitted members can receive retained history keys.</p>
      <label>Admission policy <select aria-label="Admission policy" value={policy} disabled={blocked || !isModerator}
        onChange={event => setPolicy(event.target.value as JoinPolicy)}>
        <option value="eligible_self_join">Eligible channel members may join encryption</option>
        <option value="moderator_admission">A channel moderator must approve each identity</option>
      </select></label>
      {isModerator && <button type="button" disabled={blocked || policy === view.joinPolicy} onClick={() => void perform(async () => {
        await getE2eControls(pseudonym).setAdmissionPolicy(channelId, policy); await changed();
      })}>Save admission policy</button>}
      <h3>Admitted identities</h3>
      <ul>{view.members.map(member => <li key={member.pseudonym} data-encryption-member={member.pseudonym}>
        <code className="encryption-fingerprint">{member.pseudonym}</code>
        <p>{member.pseudonym === pseudonym ? 'Your identity · ' : ''}{member.role === 'moderator' ? 'Channel moderator' : 'Channel member'} · {member.devices.length} approved device{member.devices.length === 1 ? '' : 's'}</p>
        {isModerator && <>
          <button type="button" disabled={blocked} onClick={() => setConfirmation({ kind: 'role', pseudonym: member.pseudonym,
            role: member.role === 'moderator' ? 'member' : 'moderator' })}>{member.role === 'moderator' ? 'Make member' : 'Make moderator'}</button>
          {member.pseudonym !== pseudonym && <button type="button" disabled={blocked} onClick={() => setConfirmation({ kind: 'remove', pseudonym: member.pseudonym })}>Remove member</button>}
        </>}
      </li>)}</ul>
      {isModerator && <fieldset disabled={blocked}>
        <legend>Admit an identity</legend>
        <p>The identity must already be eligible for this channel. Verify its identity and device fingerprints before approving access to new and retained messages.</p>
        <label>Identity pseudonym <input value={candidate} onChange={event => { setCandidate(event.target.value.trim()); setDevices([]); setSelectedDevice(''); }} /></label>
        <button type="button" disabled={!/^[0-9a-f]{64}$/.test(candidate)} onClick={() => void perform(async () => {
          const values = await getE2eControls(pseudonym).admissionDevices(channelId, candidate);
          if (!alive.current) return;
          setDevices(values); setSelectedDevice('');
          if (!values.length) setError('This identity has no certified device available. It must publish its encryption device first.');
        })}>Load certified devices</button>
        {devices.map(certificate => <label key={certificate.device_id} style={{ display: 'block' }}>
          <input type="radio" name="admission-device" checked={selectedDevice === certificate.device_id} onChange={() => setSelectedDevice(certificate.device_id)} />
          <code className="encryption-fingerprint">{certificate.device_id}</code>
          <span className="encryption-fingerprint"> Origin: {certificate.server_identity}</span>
        </label>)}
        <button type="button" disabled={!selectedDevice} onClick={() => void perform(async () => {
          const certificate = devices.find(device => device.device_id === selectedDevice);
          if (!certificate) throw new Error('Select a verified device first.');
          await getE2eControls(pseudonym).admitChannelMember(channelId, certificate);
          setCandidate(''); setDevices([]); setSelectedDevice(''); await changed();
        })}>Admit member</button>
      </fieldset>}
      {confirmation && <section aria-label="Confirm encryption access change">
        <h3>{confirmation.kind === 'remove' ? 'Remove this channel member?' : `Make this identity a channel ${confirmation.role}?`}</h3>
        <code className="encryption-fingerprint">{confirmation.pseudonym}</code>
        <p>{confirmation.kind === 'remove' ? 'This ends current channel membership and rotates future encryption keys. Keys and messages already received cannot be recalled. The admission policy controls whether the identity may explicitly join again.'
          : confirmation.role === 'moderator' ? 'A channel moderator may approve identities, change roles and admission policy, and enable or disable encryption.'
            : 'This identity will lose permission to manage encrypted channel access. Another moderator must remain.'}</p>
        <button type="button" disabled={blocked} onClick={() => setConfirmation(null)}>Cancel change</button>
        <button type="button" disabled={blocked} onClick={() => void perform(async () => {
          if (confirmation.kind === 'remove') await getE2eControls(pseudonym).removeChannelMember(channelId, confirmation.pseudonym);
          else await getE2eControls(pseudonym).setChannelRole(channelId, confirmation.pseudonym, confirmation.role);
          setConfirmation(null); await changed();
        })}>Confirm change</button>
      </section>}
    </> : <p role="status">Verify this channel’s encryption before managing its access.</p>}
    {error && <p role="alert">{error}</p>}
    <div className="dialog-actions">
      <button type="button" disabled={blocked} onClick={() => void perform(refresh)}>Refresh access</button>
      <button type="button" disabled={busy} onClick={onClose}>Done</button>
    </div>
  </Modal>;
}
