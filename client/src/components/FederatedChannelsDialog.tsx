import { useCallback, useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { attachAuthorityChannel, getChannelAuthority, listAuthorityChannels, listAuthorityPeers, type AuthorityChannel, type AuthorityPeer } from '@/api/channel-authority';
import { getCredentialContext, isCredentialContextCurrent } from '@/api/core';
import { useChannelsStore } from '@/stores/channels';

export function FederatedChannelsDialog({ pseudonym, onClose }: { pseudonym: string; onClose: () => void }) {
  const [peers, setPeers] = useState<AuthorityPeer[]>([]);
  const [peerId, setPeerId] = useState('');
  const [channels, setChannels] = useState<AuthorityChannel[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [loading, setLoading] = useState(false);
  const [joining, setJoining] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<AuthorityChannel | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const generation = useRef(0);
  const session = useChannelsStore(state => state.wsSessionState);
  const blocked = joining || session === 'taken_over' || session === 'revoked';
  const peer = peers.find(value => String(value.peer_id) === peerId);
  const loadPeers = useCallback(async () => {
    const ticket = ++generation.current; setLoading(true); setError(null); setSelected(null); setConfirmed(false);
    setPeers([]); setPeerId(''); setChannels([]); setNext(null); setLoaded(false);
    try { const result = await listAuthorityPeers(pseudonym); if (generation.current === ticket) setPeers(result); }
    catch (reason) { if (generation.current === ticket) setError(reason instanceof Error ? reason.message : 'Partners could not be loaded. Retry.'); }
    finally { if (generation.current === ticket) setLoading(false); }
  }, [pseudonym]);
  useEffect(() => { void loadPeers(); const counter = generation; return () => { counter.current++; }; }, [loadPeers]);
  const selectPeer = (id: string) => {
    generation.current++; setPeerId(id); setChannels([]); setNext(null); setLoaded(false); setLoading(false); setSelected(null); setConfirmed(false); setError(null);
  };
  const loadChannels = async (after?: string) => {
    if (!peer || blocked || loading) return;
    const ticket = ++generation.current; setLoading(true); setError(null); setSelected(null); setConfirmed(false);
    if (!after) { setChannels([]); setNext(null); setLoaded(false); }
    try {
      const page = await listAuthorityChannels(pseudonym, peer, after);
      if (generation.current !== ticket) return;
      setChannels(previous => after ? [...previous.filter(entry => !page.channels.some(current => current.channel_id === entry.channel_id)), ...page.channels] : page.channels);
      setNext(page.next_after_channel_id); setLoaded(true);
    } catch (reason) { if (generation.current === ticket) setError(reason instanceof Error ? reason.message : 'Channels could not be loaded. Retry.'); }
    finally { if (generation.current === ticket) setLoading(false); }
  };
  const join = async () => {
    if (!selected || !peer || blocked || !confirmed) return;
    const ticket = ++generation.current, context = getCredentialContext();
    const current = () => generation.current === ticket && isCredentialContextCurrent(context);
    const id = selected.channel_id;
    setJoining(true); setError(null);
    try {
      const local = useChannelsStore.getState().channels.find(channel => channel.channel_id === id);
      if (local) {
        const bound = await getChannelAuthority(pseudonym, id); if (!current()) return;
        if (bound.local_authority || bound.authority_identity !== peer.authority_identity) throw new Error('A different local channel already uses this channel ID. Its history cannot be overwritten by joining a partner.');
      }
      await attachAuthorityChannel(pseudonym, peer, selected); if (!current()) return;
      await useChannelsStore.getState().loadChannels(pseudonym); if (!current()) return;
      const observed = useChannelsStore.getState();
      if (observed.error || !observed.joinedChannelIds.has(id)) throw new Error(observed.error ?? 'The accepted channel membership has not been loaded. Retry joining to reconcile it.');
      await observed.selectChannel(pseudonym, id); if (current()) onClose();
    } catch (reason) { if (current()) setError(reason instanceof Error ? reason.message : 'The partner did not confirm channel membership. Retry.'); }
    finally { if (current()) setJoining(false); }
  };
  return <Modal label="Federated channels" onClose={() => { if (!joining) onClose(); }}>
    <h2>Federated channels</h2>
    <p>Choose a partner already connected to this server. Joining keeps your current identity and follows that channel’s membership and encryption policy.</p>
    <button type="button" disabled={loading || blocked} onClick={() => void loadPeers()}>Refresh partners</button>
    <label>Partner server <select aria-label="Partner server" value={peerId} disabled={blocked || !peers.length} onChange={event => selectPeer(event.target.value)}>
      <option value="">Choose a partner</option>
      {peers.map(value => <option key={value.peer_id} value={value.peer_id}>{value.label} — {value.base_url}</option>)}
    </select></label>
    {!loading && !error && !peers.length && <p>No active federation partners are available.</p>}
    {peer && <>
      <dl className="encryption-fingerprints"><dt>Partner fingerprint</dt><dd><code>{peer.authority_identity}</code></dd></dl>
      <button type="button" disabled={loading || blocked} onClick={() => void loadChannels()}>Load partner channels</button>
      {loaded && !channels.length && <p>This partner has no federated channels available.</p>}
      <ul>{channels.map(channel => <li key={channel.channel_id} data-federated-channel={channel.channel_id}>
        <strong>{channel.name}</strong><p>{channel.channel_type} · {channel.e2e_enabled ? 'Encrypted' : 'Stored on the authority server'}</p>
        <code className="encryption-fingerprint">{channel.channel_id}</code>
        {channel.initialized ? <button type="button" disabled={blocked || loading} onClick={() => { setSelected(channel); setConfirmed(false); setError(null); }}>Join channel</button>
          : <p>The partner must initialize this channel’s authority before it can be joined.</p>}
      </li>)}</ul>
      {next && <button type="button" disabled={loading || blocked} onClick={() => void loadChannels(next)}>Load more channels</button>}
    </>}
    {loading && <p role="status">Loading federated channels…</p>}
    {selected && peer && <section aria-label="Confirm federated channel">
      <h3>Join {selected.name}?</h3>
      <p>This channel’s changes are accepted by {peer.label}. Existing local history will not be replaced. Encrypted channels require a separate verification of their signed membership history before new keys are accepted.</p>
      <dl className="encryption-fingerprints"><dt>Authority</dt><dd><code>{peer.authority_identity}</code></dd><dt>Channel</dt><dd><code>{selected.channel_id}</code></dd></dl>
      <label><input type="checkbox" checked={confirmed} disabled={blocked} onChange={event => setConfirmed(event.target.checked)} />I verified this partner fingerprint</label>
      <button type="button" disabled={joining} onClick={() => { setSelected(null); setConfirmed(false); }}>Cancel joining</button>
      <button type="button" disabled={blocked || !confirmed} onClick={() => void join()}>{joining ? 'Joining channel…' : 'Confirm joining'}</button>
    </section>}
    {error && <p role="alert">{error}</p>}
    <div className="dialog-actions"><button type="button" disabled={joining} onClick={onClose}>Done</button></div>
  </Modal>;
}
