import { useState, useSyncExternalStore } from 'react';
import { getCredentialContext } from '@/api/core';
import { getUploadCleanups, removeUnpublishedUpload, subscribeUploadCleanup, type UploadCleanup } from '@/lib/upload-cleanup';

export function UploadCleanupNotice() {
  const all = useSyncExternalStore(subscribeUploadCleanup, getUploadCleanups);
  const context = getCredentialContext();
  const entries = all.filter(entry => entry.pending && entry.baseUrl === context.baseUrl && entry.pseudonym === context.pseudonymId);
  return <>{entries.map(entry => <PendingRemoval key={JSON.stringify([entry.channelId, entry.uploadId])} entry={entry} />)}</>;
}
function PendingRemoval({ entry }: { entry: UploadCleanup }) {
  const [busy, setBusy] = useState(false);
  const retry = async () => {
    setBusy(true);
    try { await removeUnpublishedUpload(entry); }
    catch { /* The durable entry retains the actual rejection. */ }
    finally { setBusy(false); }
  };
  return <div className="upload-error-bar" role="alert">
    <span>Attachment removal is not confirmed. {entry.error}</span>
    <button type="button" disabled={busy} onClick={() => void retry()}>{busy ? 'Removing attachment…' : 'Retry attachment removal'}</button>
  </div>;
}
