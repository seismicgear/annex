import { useEffect, useRef, useState } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useArrayWallet } from '@/lib/use-array-wallet';
import { arrayWalletMessage, verifiedArrayIdentity } from '@/lib/array-wallet';
import { getArrayIdentityBackup, isTauri, listArrayWalletBackups, type ArrayWalletBackup } from '@/lib/tauri';
import './array-wallet.css';

/** Restore enters Annex's existing re-prove flow; no source session is imported. */
export function ArrayWalletRestore() {
  const wallet = useArrayWallet();
  const [backups, setBackups] = useState<ArrayWalletBackup[]>([]);
  const [selected, setSelected] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const generation = useRef(0);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => { alive.current = false; generation.current += 1; };
  }, []);
  useEffect(() => {
    if (!wallet.status?.paired) {
      generation.current += 1;
      setBackups([]); setSelected(''); setLoaded(false);
    }
  }, [wallet.status?.paired]);

  const load = async () => {
    const current = ++generation.current;
    setBusy(true); setError(null);
    try {
      const records = await listArrayWalletBackups();
      if (!alive.current || current !== generation.current) return;
      setBackups(records); setSelected(records[0]?.id ?? ''); setLoaded(true);
    } catch (cause) {
      if (alive.current && current === generation.current) setError(arrayWalletMessage(cause, 'Could not load backups from Array. Try again.'));
    } finally { if (alive.current) setBusy(false); }
  };

  const restore = async () => {
    if (!selected) return;
    const current = ++generation.current;
    setBusy(true); setError(null);
    try {
      const backup = await getArrayIdentityBackup(selected);
      if (backup.format !== 'annex.identity.v1' || backup.managedBy !== 'array') throw new Error('This is not a usable Annex identity backup.');
      const identity = await verifiedArrayIdentity(backup.value);
      if (!alive.current || current !== generation.current) return;
      await useIdentityStore.getState().importBackup(JSON.stringify(identity));
    } catch (cause) {
      if (alive.current && current === generation.current) setError(arrayWalletMessage(cause, 'Could not restore the identity from Array. Your current identities have been kept.'));
    } finally { if (alive.current) setBusy(false); }
  };

  if (!isTauri()) return null;
  return (
    <section className="import-section array-wallet-panel" aria-label="Restore from Array">
      <h3>Restore from Array</h3>
      {!wallet.status && !wallet.error ? <p role="status">Checking Array…</p> : wallet.status?.managedBy !== 'array' ? (
        <p>Close Annex, then open it from Array to restore an identity from your credential wallet.</p>
      ) : <>
        <p><strong>Managed by Array</strong>{wallet.status.accountLabel ? ` · ${wallet.status.accountLabel}` : ''}</p>
        {wallet.status.message && <p role="status">{wallet.status.message}</p>}
        {wallet.status.paired && <>
          <p>Choose a saved Annex identity. Annex will establish a fresh session with its server.</p>
          <button type="button" disabled={busy} onClick={() => void load()}>{busy ? 'Working…' : loaded ? 'Refresh Array backups' : 'Choose an Array backup'}</button>
          {loaded && backups.length === 0 && <p role="status">No Annex backups are saved in this Array wallet yet.</p>}
          {backups.length > 0 && <>
            <label>Annex identity backup
              <select value={selected} onChange={event => setSelected(event.target.value)} disabled={busy}>
                {backups.map(backup => <option key={backup.id} value={backup.id}>{backup.label} · {backup.id.slice(-8)}</option>)}
              </select>
            </label>
            <button type="button" disabled={busy || !selected} onClick={() => void restore()}>Restore selected identity</button>
          </>}
        </>}
      </>}
      {(error || wallet.error) && <p className="error-message" role="alert">{error ?? wallet.error}</p>}
      {(wallet.error || wallet.status?.message) && <button type="button" onClick={() => void wallet.refresh().catch(() => {})}>Check Array again</button>}
    </section>
  );
}
