import { useCallback, useEffect, useRef, useState } from 'react';
import { disconnectArrayWallet, getArrayWalletStatus, isTauri, type ArrayWalletStatus } from '@/lib/tauri';
import { arrayWalletMessage } from '@/lib/array-wallet';

const standalone: ArrayWalletStatus = {
  managedBy: null, available: false, paired: false, accountLabel: null, message: null,
};

export function useArrayWallet() {
  const [status, setStatus] = useState<ArrayWalletStatus | null>(isTauri() ? null : standalone);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(true);
  const activeRead = useRef<Promise<ArrayWalletStatus> | null>(null);
  const version = useRef(0);

  const refresh = useCallback(async () => {
    if (!isTauri()) return standalone;
    if (activeRead.current) return activeRead.current;
    const requestVersion = version.current;
    const pending = getArrayWalletStatus().then(next => {
      if (mounted.current && requestVersion === version.current) { setStatus(next); setError(null); }
      return next;
    }).catch(cause => {
      if (mounted.current && requestVersion === version.current) {
        setError(arrayWalletMessage(cause, 'Could not check the Array wallet. Try again or reopen Annex from Array.'));
      }
      throw cause;
    }).finally(() => { if (activeRead.current === pending) activeRead.current = null; });
    activeRead.current = pending;
    return pending;
  }, []);

  useEffect(() => {
    mounted.current = true;
    const check = () => { void refresh().catch(() => {}); };
    const changed = () => {
      version.current += 1;
      activeRead.current = null;
      check();
    };
    check();
    if (!isTauri()) return () => { mounted.current = false; };
    window.addEventListener('focus', check);
    window.addEventListener('annex-array-wallet-changed', changed);
    const timer = window.setInterval(check, 15_000);
    return () => {
      mounted.current = false;
      window.removeEventListener('focus', check);
      window.removeEventListener('annex-array-wallet-changed', changed);
      window.clearInterval(timer);
    };
  }, [refresh]);

  const disconnect = useCallback(async () => {
    setBusy(true); setError(null); version.current += 1;
    activeRead.current = null;
    try {
      const next = await disconnectArrayWallet();
      if (mounted.current) setStatus(next);
      window.dispatchEvent(new Event('annex-array-wallet-changed'));
    } catch (cause) {
      if (mounted.current) setError(arrayWalletMessage(cause, 'Could not disconnect Array. The pairing remains active; try again.'));
    } finally { if (mounted.current) setBusy(false); }
  }, []);

  return { status, error, busy, refresh, disconnect };
}
