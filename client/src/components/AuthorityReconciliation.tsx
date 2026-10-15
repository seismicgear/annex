import { useEffect, useRef, useState } from 'react';
import { listAuthorityChannels, listAuthorityPeers, type AuthorityChannel, type AuthorityPeer } from '@/api/channel-authority';
import { cancelReconciliation, getReconciliationPreview, listReconciliations, prepareReconciliation, reconcileChannel,
  type ReconciliationOperation, type ReconciliationPreview, type ReconciliationRequest, type ReconciliationResult } from '@/api/authority-reconciliation';

const errorMessage = (value: unknown) => value instanceof Error && value.message.trim() ? value.message : 'The reconciliation could not be completed. Review the saved operation before retrying.';
export function AuthorityReconciliation({ pseudonym, channelId, disabled, onBusyChange, onComplete }: {
  pseudonym: string; channelId: string; disabled: boolean; onBusyChange: (value: boolean) => void; onComplete: () => Promise<void>;
}) {
  const [open, setOpen] = useState(false), [loading, setLoading] = useState(false), [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null), [verified, setVerified] = useState(false), [cancelPrompt, setCancelPrompt] = useState(false);
  const [peers, setPeers] = useState<AuthorityPeer[]>([]), [peerId, setPeerId] = useState('');
  const [partnerChannel, setPartnerChannel] = useState<AuthorityChannel | null>(null);
  const [after, setAfter] = useState<string | null>(null), [catalogRead, setCatalogRead] = useState(false);
  const [preview, setPreview] = useState<ReconciliationPreview | null>(null);
  const [operations, setOperations] = useState<ReconciliationOperation[]>([]);
  const [intent, setIntent] = useState<ReconciliationRequest | null>(null), [result, setResult] = useState<ReconciliationResult | null>(null);
  const generation = useRef(0);
  useEffect(() => { const counter = generation; return () => { counter.current++; }; }, []);
  const partner = peers.find(value => String(value.peer_id) === peerId);
  const blocked = disabled || busy || loading;
  const resetReview = () => { setPreview(null); setIntent(null); setResult(null); setVerified(false); setCancelPrompt(false); setError(null); };
  const load = async () => {
    const ticket = ++generation.current; setOpen(true); setLoading(true); setError(null);
    try {
      const [nextPeers, saved] = await Promise.all([listAuthorityPeers(pseudonym), listReconciliations(pseudonym, channelId)]);
      if (generation.current !== ticket) return;
      setPeers(nextPeers); setOperations(saved);
      if (intent) { const current = saved.find(value => value.operation_id === intent.operation_id); if (current) setResult(current); }
    } catch (reason) { if (generation.current === ticket) setError(errorMessage(reason)); }
    finally { if (generation.current === ticket) setLoading(false); }
  };
  const inspectPartner = async (cursor?: string) => {
    if (!partner || blocked) return;
    const ticket = ++generation.current; setLoading(true); setError(null); setPreview(null); setVerified(false);
    if (!cursor) { setPartnerChannel(null); setCatalogRead(false); setAfter(null); }
    try {
      const catalog = await listAuthorityChannels(pseudonym, partner, cursor);
      if (generation.current !== ticket) return;
      setPartnerChannel(catalog.channels.find(value => value.channel_id === channelId) ?? null);
      setCatalogRead(true); setAfter(catalog.next_after_channel_id);
    } catch (reason) { if (generation.current === ticket) setError(errorMessage(reason)); }
    finally { if (generation.current === ticket) setLoading(false); }
  };
  const inspectLocal = async () => {
    if (!partnerChannel?.initialized || blocked) return;
    const ticket = ++generation.current; setLoading(true); setError(null); setVerified(false);
    try { const value = await getReconciliationPreview(pseudonym, channelId); if (generation.current === ticket) setPreview(value); }
    catch (reason) { if (generation.current === ticket) setError(errorMessage(reason)); }
    finally { if (generation.current === ticket) setLoading(false); }
  };
  const storeResult = (request: ReconciliationRequest, outcome: ReconciliationResult) => {
    setResult(outcome);
    setOperations(values => [{ ...request, ...outcome, channel_id: channelId }, ...values.filter(value => value.operation_id !== request.operation_id)]);
  };
  const submit = async () => {
    if (blocked || !verified || (!intent && (!preview || !partner || !partnerChannel?.initialized))) return;
    const ticket = ++generation.current; setBusy(true); onBusyChange(true); setError(null); setCancelPrompt(false);
    try {
      const request = intent ?? await prepareReconciliation(pseudonym, preview!, partner!, partnerChannel!);
      if (generation.current !== ticket) return;
      // Preserve this exact request before the mutation. A lost acknowledgement
      // must not produce a new operation or silently adopt a different snapshot.
      setIntent(request);
      const outcome = await reconcileChannel(pseudonym, channelId, request);
      if (generation.current !== ticket) return;
      storeResult(request, outcome);
      if (outcome.status === 'complete') await onComplete();
    } catch (reason) { if (generation.current === ticket) setError(errorMessage(reason)); }
    finally { if (generation.current === ticket) setBusy(false); onBusyChange(false); }
  };
  const cancel = async () => {
    if (!intent || blocked || !cancelPrompt) return;
    const ticket = ++generation.current; setBusy(true); onBusyChange(true); setError(null);
    try {
      const outcome = await cancelReconciliation(pseudonym, channelId, intent.operation_id);
      if (generation.current !== ticket) return;
      storeResult(intent, outcome); setCancelPrompt(false); setVerified(false);
    } catch (reason) { if (generation.current === ticket) setError(errorMessage(reason)); }
    finally { if (generation.current === ticket) setBusy(false); onBusyChange(false); }
  };
  const reviewSaved = (value: ReconciliationOperation) => {
    ++generation.current; setIntent(value); setResult(value); setPreview(null); setVerified(false); setCancelPrompt(false); setError(null);
  };
  const currentFingerprint = intent?.authority_identity ?? partner?.authority_identity;
  const currentSnapshot = intent?.expected_snapshot_sha256 ?? preview?.snapshot_sha256;
  return <section aria-label="Partner history reconciliation">
    <h3>Reconcile retained channel history</h3>
    <p>If this channel already exists locally and on a partner, compare its retained history before adopting the partner’s signed authority. An initialized local authority cannot be retargeted.</p>
    {!open ? <button type="button" disabled={disabled} onClick={() => void load()}>Review partner reconciliation</button> : <>
      {loading && <p role="status">Reading reconciliation information…</p>}
      <button type="button" disabled={blocked} onClick={() => void load()}>Refresh saved reconciliations</button>
      {operations.length > 0 && <section aria-label="Saved reconciliations"><h4>Saved operations</h4><ul>
        {operations.map(value => <li key={value.operation_id} data-reconciliation-operation={value.operation_id}>
          <code>{value.operation_id}</code> — {value.status}
          <button type="button" disabled={blocked} onClick={() => reviewSaved(value)}>Review saved operation</button>
        </li>)}
      </ul></section>}
      {!intent && <>
        <label>Reconciliation partner<select value={peerId} disabled={blocked} onChange={event => {
          ++generation.current; setPeerId(event.target.value); setPartnerChannel(null); setCatalogRead(false); setAfter(null); resetReview();
        }}><option value="">Choose a partner</option>{peers.map(value => <option key={value.peer_id} value={String(value.peer_id)}>{value.label} — {value.base_url}</option>)}</select></label>
        {partner && <button type="button" disabled={blocked} onClick={() => void inspectPartner()}>Load matching partner channel</button>}
        {catalogRead && !partnerChannel && <p>{after ? 'This page does not contain this channel. Continue searching the partner’s catalog.' : 'The partner does not offer this channel ID.'}</p>}
        {!partnerChannel && after && <button type="button" disabled={blocked} onClick={() => void inspectPartner(after)}>Load more partner channels</button>}
        {partnerChannel && <>
          <p>Partner channel: {partnerChannel.name} (<code>{partnerChannel.channel_id}</code>).</p>
          {partnerChannel.initialized ? <><p>Your device will be verified before the local snapshot is taken. The review does not join the partner channel.</p><button type="button" disabled={blocked} onClick={() => void inspectLocal()}>Review retained local history</button></>
            : <p>The partner must initialize its channel authority before reconciliation.</p>}
        </>}
      </>}
      {(preview || intent) && <section aria-label="Confirm history reconciliation">
        <h4>Verify the partner and retained snapshot</h4>
        <dl className="encryption-fingerprints"><dt>Channel</dt><dd><code>{channelId}</code></dd>
          <dt>Partner authority fingerprint</dt><dd><code>{currentFingerprint}</code></dd>
          <dt>Local snapshot fingerprint</dt><dd><code>{currentSnapshot}</code></dd>
          {intent && <><dt>Operation</dt><dd><code>{intent.operation_id}</code></dd><dt>Signing device</dt><dd><code>{intent.device_id}</code></dd></>}
        </dl>
        {preview && <p>This snapshot retains {preview.messages} messages, {preview.edits} edits, {preview.attachments} attachments, and {preview.key_records} encryption records. Its fingerprint also covers channel settings, membership, and outstanding legacy deliveries.</p>}
        <p>The current local projection changes only after every retained record is accounted for in authenticated partner history or an explicit signed removal. A conflict preserves the current local state. Closing this dialog does not cancel a submitted operation.</p>
        {result && <>
          <p role="status">{result.status === 'complete' ? 'Reconciliation completed. The channel now follows the verified partner authority.'
            : result.status === 'cancelled' ? 'Remaining local reconciliation staging was cancelled. The retained channel history was not replaced.'
              : result.status === 'conflict' ? 'Reconciliation found conflicts. The current local channel state is preserved.'
                : `Reconciliation is pending. Verified through event ${result.after_seq} of ${result.head_seq}. Resume to continue.`}</p>
          <p>{result.remote_join_accepted ? 'The partner has accepted this identity’s channel Join. Cancelling staging does not reverse that Join.' : 'No partner Join has been confirmed by this operation.'}</p>
          {result.status === 'complete' && <p>{result.retained_messages} messages retained; {result.removed_messages} messages removed by authenticated authority history.</p>}
          {result.conflicts.length > 0 && <section aria-label="Reconciliation conflicts"><h4>Records requiring reconciliation</h4><ul>
            {result.conflicts.map((conflict, index) => <li key={`${conflict.kind}:${conflict.id}:${index}`}><span>{conflict.kind}</span>: <code>{conflict.id}</code></li>)}
          </ul><p>If the partner gains the missing authenticated history, resume this operation. If the local snapshot changes, review a new snapshot and confirm a new operation.</p></section>}
        </>}
        {(!result || result.status === 'pending' || result.status === 'conflict') && <>
          <label><input type="checkbox" checked={verified} disabled={blocked} onChange={event => setVerified(event.target.checked)} />I verified this partner fingerprint and local snapshot</label>
          <button type="button" disabled={blocked || !verified} onClick={() => void submit()}>{busy ? 'Reconciling…' : intent ? 'Resume reconciliation' : 'Confirm reconciliation'}</button>
          {!intent ? <button type="button" disabled={blocked} onClick={() => { setPreview(null); setVerified(false); }}>Cancel reconciliation review</button>
            : <button type="button" disabled={blocked} onClick={() => setCancelPrompt(true)}>Cancel remaining reconciliation</button>}
        </>}
        {intent && <button type="button" disabled={blocked} onClick={resetReview}>Review a new local snapshot</button>}
      </section>}
      {cancelPrompt && <section aria-label="Confirm reconciliation cancellation">
        <h4>Cancel remaining local staging?</h4>
        <p>This stops local reconciliation staging and preserves the retained channel history. The partner may already have accepted this identity’s Join; cancellation does not reverse that Join.</p>
        <button type="button" disabled={blocked} onClick={() => setCancelPrompt(false)}>Keep reconciliation</button>
        <button type="button" disabled={blocked} onClick={() => void cancel()}>Confirm cancellation</button>
      </section>}
      {error && <p role="alert">{error}</p>}
    </>}
  </section>;
}
