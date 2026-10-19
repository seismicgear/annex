import { useArrayWallet } from '@/lib/use-array-wallet';
import { isTauri } from '@/lib/tauri';
import './array-wallet.css';

/** Lives inside the existing Identity settings dialog. */
export function ArrayWalletControls() {
  const wallet = useArrayWallet();
  if (!isTauri()) return null;
  return (
    <section className="policy-section array-wallet-panel" aria-label="Array credential wallet">
      <h3>Credential wallet</h3>
      {!wallet.status && !wallet.error && <p role="status">Checking Array…</p>}
      {wallet.status?.managedBy === 'array' ? <>
        <p><strong>Managed by Array</strong>{wallet.status.accountLabel ? ` · ${wallet.status.accountLabel}` : ''}</p>
        <p className="field-hint">Identity backups use your Array credential wallet when it is available. Manage saved credentials and app access in Array → Settings → Credential wallet. If Array is unavailable, Backup offers an explicit file export.</p>
        {wallet.status.message && <p role="status">{wallet.status.message}</p>}
        <button type="button" disabled={wallet.busy} onClick={() => void wallet.disconnect()}>
          {wallet.busy ? 'Disconnecting…' : 'Disconnect Array pairing'}
        </button>
        <p className="field-hint">Disconnecting keeps this Annex identity and existing backups. To use the wallet again, pair Annex in Array and reopen Annex from Array.</p>
      </> : wallet.status && <p className="field-hint">Close Annex, then open it from Array to use its credential wallet. You can also export and import identity backup files independently.</p>}
      {wallet.error && <p className="error-message" role="alert">{wallet.error}</p>}
      {(wallet.error || wallet.status?.message) && <button type="button" onClick={() => void wallet.refresh().catch(() => {})}>Check Array again</button>}
    </section>
  );
}
