import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { parsePrivateAttachment, type PrivateAttachment } from '@/lib/encrypted-attachments';
import { readEncryptedAttachment, retryEncryptedAttachment } from '@/api/encrypted-uploads';
import { getAttachmentTransfer, subscribeAttachmentTransfers } from '@/lib/attachment-transfers';
import { getCredentialContext } from '@/api/core';
import { useChannelsStore } from '@/stores/channels';
import { Modal } from './Modal';

const failureMessage = (reason: unknown, fallback: string) => reason instanceof Error && reason.message.trim() ? reason.message : fallback;

export function EncryptedAttachment({ attachment, channelId, pseudonym }: { attachment: PrivateAttachment; channelId: string; pseudonym: string }) {
  const encoded = JSON.stringify(attachment);
  const descriptor = useMemo(() => parsePrivateAttachment(JSON.parse(encoded)), [encoded]);
  const [url, setUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [retrying, setRetrying] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const generation = useRef(0);
  useEffect(() => { const counter = generation; return () => { counter.current++; }; }, []);
  const session = useChannelsStore(state => state.wsSessionState);
  const contextSerial = getCredentialContext().serial;
  const transfer = useSyncExternalStore(subscribeAttachmentTransfers,
    () => getAttachmentTransfer(channelId, descriptor.id));
  const status = transfer?.status;
  useEffect(() => {
    const current = ++generation.current, abort = new AbortController();
    let objectUrl: string | undefined;
    setUrl(null); setError(null); setExpanded(false); setRetrying(false);
    if (status === 'removed') { setError('This attachment was removed.'); return; }
    readEncryptedAttachment(pseudonym, channelId, descriptor, abort.signal).then(blob => {
      if (abort.signal.aborted || current !== generation.current) return;
      objectUrl = URL.createObjectURL(blob); setUrl(objectUrl);
    }).catch(reason => { if (!abort.signal.aborted && current === generation.current) setError(failureMessage(reason, 'The encrypted attachment could not be loaded.')); });
    return () => { abort.abort(); if (objectUrl) URL.revokeObjectURL(objectUrl); };
  }, [descriptor, channelId, pseudonym, contextSerial, status, attempt]);
  const retry = async () => {
    const current = generation.current;
    setRetrying(true); setError(null);
    try { await retryEncryptedAttachment(pseudonym, channelId, descriptor.id); if (current === generation.current) setAttempt(value => value + 1); }
    catch (reason) { if (current === generation.current) setError(failureMessage(reason, 'The attachment retry failed.')); }
    finally { if (current === generation.current) setRetrying(false); }
  };
  return <div className="encrypted-attachment" data-attachment-id={descriptor.id}>
    {url ? <>
      {descriptor.category === 'image' && <button type="button" aria-label={`View ${descriptor.name} full size`} className="encrypted-image-button" onClick={() => setExpanded(true)}>
        <img src={url} alt={descriptor.name} className="message-inline-image" />
      </button>}
      {descriptor.category === 'video' && <video src={url} aria-label={descriptor.name} className="message-inline-video" controls playsInline preload="metadata" />}
      <a href={url} download={descriptor.name}>Download {descriptor.name}</a>
    </> : <>
      {error && <p role="alert">{error}</p>}
      {transfer?.status === 'pending' ? <p role="status">Attachment transfer pending ({transfer.transferredBytes} of {transfer.totalBytes} bytes).</p>
        : !error && <p role="status">Decrypting attachment…</p>}
      {error && status !== 'removed' && <button disabled={retrying || session === 'taken_over' || session === 'revoked'} onClick={() => void retry()}>{retrying ? 'Retrying…' : 'Retry attachment'}</button>}
    </>}
    {expanded && url && <Modal label={descriptor.name} onClose={() => setExpanded(false)}>
      <img src={url} alt={descriptor.name} style={{ maxWidth: '100%' }} />
      <button type="button" onClick={() => setExpanded(false)}>Close image</button>
    </Modal>}
  </div>;
}
