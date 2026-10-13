import { useEffect, useRef, useState } from 'react';
import { Modal } from './Modal';
import { useDialogTitleId } from '@/lib/use-dialog-title-id';
import { useVoiceStore } from '@/stores/voice';
import { checkDesktopUpdates, downloadDesktopUpdate, getDesktopUpdateStatus, installDesktopUpdate, onDesktopUpdateStatus, type DesktopUpdateStatus } from '@/lib/tauri';

const busyPhases = new Set(['checking', 'downloading', 'verifying', 'installing']);

export function DesktopUpdates({ onClose }: { onClose: () => void }) {
  const titleId = useDialogTitleId();
  const [status, setStatus] = useState<DesktopUpdateStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [working, setWorking] = useState(false);
  const [confirmInstall, setConfirmInstall] = useState(false);
  const revision = useRef(0);

  useEffect(() => {
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    void (async () => {
      unsubscribe = await onDesktopUpdateStatus(value => {
        if (!disposed) { revision.current++; setStatus(value); }
      });
      if (disposed) { unsubscribe(); return; }
      const before = revision.current;
      const value = await getDesktopUpdateStatus();
      if (!disposed && revision.current === before) setStatus(value);
    })().catch(reason => { if (!disposed) setError(reason instanceof Error ? reason.message : String(reason)); });
    return () => { disposed = true; unsubscribe?.(); };
  }, []);

  const run = async (action: () => Promise<DesktopUpdateStatus>) => {
    if (working) return;
    setError(null);
    setWorking(true);
    try { setStatus(await action()); }
    catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)); }
    finally { setWorking(false); }
  };
  const busy = working || (status !== null && busyPhases.has(status.phase));
  const supported = status !== null && status.phase !== 'unavailable';

  return <Modal onClose={onClose} titleId={titleId} className="settings-dialog">
    <h2 id={titleId}>Annex Updates</h2>
    {error && <p className="error-message" role="alert">{error}</p>}
    {!status && !error && <p role="status">Loading update status…</p>}
    {status && <>
      <p>Installed version: {status.currentVersion}</p>
      {status.message && <p role={status.phase === 'failed' ? 'alert' : 'status'}>{status.message}</p>}
      {status.phase === 'up_to_date' && <p role="status">This version is up to date.</p>}
      {status.phase === 'checking' && <p role="status">Checking for updates…</p>}
      {status.availableVersion && <p>Available version: {status.availableVersion}</p>}
      {status.notes && <p style={{ whiteSpace: 'pre-wrap' }}>{status.notes}</p>}
      {status.phase === 'downloading' && <div role="status">
        <p>Downloading update… {status.totalBytes ? `${Math.min(100, Math.floor(status.downloadedBytes / status.totalBytes * 100))}%` : `${Math.floor(status.downloadedBytes / 1024)} KB`}</p>
        <progress aria-label="Update download" value={status.totalBytes ? status.downloadedBytes : undefined} max={status.totalBytes ?? 1} />
      </div>}
      {status.phase === 'verifying' && <p role="status">Verifying the update signature…</p>}
      {status.phase === 'verified' && <p role="status">Signature verified. Ready to install.</p>}
      {status.phase === 'installing' && <p role="status">Stopping active sessions and installing the update…</p>}
      {status.endpoints.length > 0 && <details><summary>Configured update source</summary>{status.endpoints.map(endpoint => <p key={endpoint} style={{ overflowWrap: 'anywhere' }}>{endpoint}</p>)}</details>}
    </>}
    {confirmInstall && <p role="alert">Installing closes active calls and stops your local server. Annex will restart after installation.</p>}
    <div className="dialog-actions">
      {supported && <button disabled={busy} onClick={() => { setConfirmInstall(false); void run(checkDesktopUpdates); }}>Check for updates</button>}
      {supported && status.availableVersion && (status.phase === 'available' || status.phase === 'failed') && <button disabled={busy} onClick={() => void run(downloadDesktopUpdate)}>Download update</button>}
      {status?.phase === 'verified' && <button disabled={busy} className="primary-btn" onClick={() => {
        if (!confirmInstall) { setConfirmInstall(true); return; }
        setConfirmInstall(false);
        useVoiceStore.getState().forceReset();
        void run(installDesktopUpdate);
      }}>{confirmInstall ? 'Confirm install and restart' : 'Install and restart'}</button>}
      <button onClick={onClose}>Close</button>
    </div>
  </Modal>;
}
