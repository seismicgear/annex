import { useCallback, useEffect, useRef, useState } from 'react';
import { getChannelAuthority, initializeChannelAuthority, type ChannelAuthority } from '@/api/channel-authority';
import { getCredentialContext, isCredentialContextCurrent } from '@/api/core';
import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';
import { Modal } from './Modal';
import { AuthorityReconciliation } from './AuthorityReconciliation';

export function ChannelAuthorityDialog({ pseudonym, channelId, onClose }: { pseudonym: string; channelId: string; onClose: () => void }) {
  const [value, setValue] = useState<ChannelAuthority | null>(null);
  const [loading, setLoading] = useState(false), [busy, setBusy] = useState(false);
  const [reconciliationBusy, setReconciliationBusy] = useState(false);
  const [error, setError] = useState<string | null>(null), [confirm, setConfirm] = useState(false);
  const generation = useRef(0);
  const moderator = useIdentityStore(state => state.permissions?.capabilities.can_moderate ?? false);
  const session = useChannelsStore(state => state.wsSessionState);
  const member = useChannelsStore(state => state.joinedChannelIds.has(channelId));
  const blocked = busy || reconciliationBusy || session === 'taken_over' || session === 'revoked';
  const load = useCallback(async () => {
    const ticket = ++generation.current; setLoading(true); setError(null); setConfirm(false);
    try { const result = await getChannelAuthority(pseudonym, channelId); if (generation.current === ticket) setValue(result); }
    catch (reason) { if (generation.current === ticket) { setValue(null); setError(reason instanceof Error ? reason.message : 'Channel authority could not be read. Retry.'); } }
    finally { if (generation.current === ticket) setLoading(false); }
  }, [pseudonym, channelId]);
  useEffect(() => { void load(); const counter = generation; return () => { counter.current++; }; }, [load]);
  const initialize = async () => {
    if (!value || !moderator || blocked || !confirm) return;
    const ticket = ++generation.current; setBusy(true); setError(null);
    try { const result = await initializeChannelAuthority(pseudonym, value); if (generation.current === ticket) { setValue(result); setConfirm(false); } }
    catch (reason) { if (generation.current === ticket) setError(reason instanceof Error ? reason.message : 'The channel authority was not initialized. Retry.'); }
    finally { if (generation.current === ticket) setBusy(false); }
  };
  const reconciled = async () => {
    // Refresh the actual authority and history. Existing encryption trust
    // anchors remain intact; adopting ordinary history cannot waive them.
    const context = getCredentialContext();
    const current = () => context.pseudonymId === pseudonym && isCredentialContextCurrent(context);
    if (!current()) return;
    await load();
    if (!current()) return;
    await useChannelsStore.getState().loadChannels(pseudonym);
    if (!current()) return;
    const channels = useChannelsStore.getState();
    if (channels.activeChannelId === channelId) await channels.selectChannel(pseudonym, channelId);
  };
  return <Modal label="Channel authority" onClose={() => { if (!busy && !reconciliationBusy) onClose(); }}>
    <h2>Channel authority</h2>
    {loading && <p role="status">Reading channel authority…</p>}
    {value && <>
      <dl className="encryption-fingerprints"><dt>Channel</dt><dd><code>{value.channel_id}</code></dd><dt>Authority fingerprint</dt><dd><code>{value.authority_identity}</code></dd><dt>Server</dt><dd>{value.authority_server}</dd></dl>
      {value.initialized ? <p role="status">{value.local_authority ? 'This server accepts changes for this federated channel.' : 'This channel follows the selected partner’s accepted changes.'} Its signed history contains {value.head_seq} events.</p>
        : <><p>This existing channel has not initialized its signed federation history. Initialization preserves retained messages and edit history, and lets agreed partners explicitly join this channel.</p>
          {moderator && value.local_authority ? <button type="button" disabled={blocked || loading} onClick={() => setConfirm(true)}>Initialize channel authority</button>
            : <p>A moderator on the authority server must initialize this channel.</p>}</>}
      {confirm && <section aria-label="Confirm channel initialization">
        <h3>Initialize this channel’s authority?</h3>
        <p>This server will become the fixed authority for the existing channel. Its retained history and encryption key namespaces are preserved. Joining a different authority cannot overwrite this channel afterward.</p>
        <button type="button" disabled={busy} onClick={() => setConfirm(false)}>Cancel initialization</button>
        <button type="button" disabled={blocked || !moderator} onClick={() => void initialize()}>{busy ? 'Initializing authority…' : 'Confirm initialization'}</button>
      </section>}
      {moderator && member && value.local_authority && !value.initialized && !confirm && <AuthorityReconciliation
        pseudonym={pseudonym} channelId={channelId} disabled={blocked || loading}
        onBusyChange={setReconciliationBusy} onComplete={reconciled} />}
    </>}
    {error && <p role="alert">{error}</p>}
    <div className="dialog-actions"><button type="button" disabled={loading || blocked} onClick={() => void load()}>Refresh authority</button><button type="button" disabled={busy || reconciliationBusy} onClick={onClose}>Done</button></div>
  </Modal>;
}
