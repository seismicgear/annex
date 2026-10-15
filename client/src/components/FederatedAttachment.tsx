import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { getCredentialContext } from '@/api/core';
import { parseOrdinaryAttachment, readOrdinaryFederatedAttachment, retryOrdinaryFederatedAttachment, type OrdinaryAttachmentCategory } from '@/api/federated-attachments';
import { getAttachmentTransfer, subscribeAttachmentTransfers } from '@/lib/attachment-transfers';
import { useChannelsStore } from '@/stores/channels';
import { Modal } from './Modal';

const failureMessage = (reason: unknown, fallback: string) => reason instanceof Error && reason.message.trim() ? reason.message : fallback;

export function FederatedAttachment({ path, category, channelId, pseudonym }: {
  path: string; category: OrdinaryAttachmentCategory; channelId: string; pseudonym: string;
}) {
  let attachment;
  try { attachment = parseOrdinaryAttachment(path, category); } catch { /* Invalid text is displayed as an attachment error below. */ }
  const id = attachment?.id ?? '', filename = attachment?.filename ?? 'attachment';
  const [url, setUrl] = useState<string | null>(null), [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0), [retrying, setRetrying] = useState(false), [expanded, setExpanded] = useState(false);
  const generation = useRef(0);
  useEffect(() => { const counter = generation; return () => { counter.current++; }; }, []);
  const session = useChannelsStore(state => state.wsSessionState);
  const contextSerial = getCredentialContext().serial;
  const transfer = useSyncExternalStore(subscribeAttachmentTransfers, () => getAttachmentTransfer(channelId, id));
  const status = transfer?.status;
  useEffect(() => {
    const current = ++generation.current, abort = new AbortController();
    let objectUrl: string | undefined;
    setUrl(null); setError(null); setExpanded(false); setRetrying(false);
    if (status === 'removed') { setError('This attachment was removed.'); return; }
    readOrdinaryFederatedAttachment(pseudonym, path, category, abort.signal).then(blob => {
      if (abort.signal.aborted || current !== generation.current) return;
      objectUrl = URL.createObjectURL(blob); setUrl(objectUrl);
    }).catch(reason => { if (!abort.signal.aborted && current === generation.current) setError(failureMessage(reason, 'The attachment could not be loaded.')); });
    return () => { abort.abort(); if (objectUrl) URL.revokeObjectURL(objectUrl); };
  }, [path, category, channelId, pseudonym, contextSerial, status, attempt]);
  const retry = async () => {
    const current = generation.current;
    setRetrying(true); setError(null);
    try { await retryOrdinaryFederatedAttachment(pseudonym, channelId, id); if (current === generation.current) setAttempt(value => value + 1); }
    catch (reason) { if (current === generation.current) setError(failureMessage(reason, 'The attachment retry failed.')); }
    finally { if (current === generation.current) setRetrying(false); }
  };
  return <div className="federated-attachment" data-attachment-id={id}>
    {url ? <>
      {category === 'image' && <button type="button" className="encrypted-image-button" aria-label="View image full size" onClick={() => setExpanded(true)}>
        <img src={url} alt="Uploaded image" className="message-inline-image" />
      </button>}
      {category === 'video' && <video src={url} aria-label={filename} className="message-inline-video" controls playsInline preload="metadata" />}
      <a href={url} download={filename} className="message-file-link">Download {filename}</a>
    </> : <>
      {error && <p role="alert">{error}</p>}
      {transfer?.status === 'pending' ? <p role="status">Attachment transfer pending ({transfer.transferredBytes} of {transfer.totalBytes} bytes).</p>
        : !error && <p role="status">Loading attachment…</p>}
      {error && id && status !== 'removed' && <button type="button" disabled={retrying || session === 'taken_over' || session === 'revoked'} onClick={() => void retry()}>{retrying ? 'Retrying…' : 'Retry attachment'}</button>}
    </>}
    {expanded && url && <Modal label="Full size attachment" onClose={() => setExpanded(false)}>
      <img src={url} alt="Full size" style={{ maxWidth: '100%' }} />
      <button type="button" onClick={() => setExpanded(false)}>Close image</button>
    </Modal>}
  </div>;
}
