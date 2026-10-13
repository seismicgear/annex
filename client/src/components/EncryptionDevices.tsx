import { useEffect, useState } from 'react';
import { Modal } from './Modal';
import { getE2eControls } from '@/lib/e2e-store';
import { useChannelsStore } from '@/stores/channels';
import type { E2eControls } from '@/api/e2e-v2';

type Snapshot = Awaited<ReturnType<E2eControls['channelDevices']>>;
export function EncryptionDevices({ channelId, pseudonym, onClose }: { channelId: string; pseudonym: string; onClose(): void }) {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refresh, setRefresh] = useState(0);
  const [remove, setRemove] = useState<string | null>(null);
  const session = useChannelsStore(state => state.wsSessionState);
  const blocked = busy || session === 'taken_over' || session === 'revoked';
  useEffect(() => {
    let current = true;
    getE2eControls(pseudonym).channelDevices(channelId).then(value => { if (current) setSnapshot(value); })
      .catch(reason => { if (current) setError(reason instanceof Error ? reason.message : 'Devices could not be loaded.'); });
    return () => { current = false; };
  }, [channelId, pseudonym, refresh]);
  const act = async (operation: (controls: E2eControls) => Promise<void>) => {
    if (blocked) return;
    setBusy(true); setError(null);
    try {
      await operation(getE2eControls(pseudonym));
      await useChannelsStore.getState().retryChannelKey(pseudonym);
      setRemove(null); setRefresh(value => value + 1);
    } catch (reason) { setError(reason instanceof Error ? reason.message : 'The device change was not confirmed.'); }
    finally { setBusy(false); }
  };
  return <Modal label="Encryption devices" onClose={() => { if (!busy) onClose(); }}>
    <h2>Encryption devices</h2>
    <p>Compare a new device’s fingerprint before approving it. Approval shares this channel’s current and available history keys. Removing a device rotates future keys; copies of past messages cannot be recalled.</p>
    {error && <p role="alert">{error}</p>}
    {!snapshot && !error && <p role="status">Loading devices…</p>}
    {snapshot?.devices.map(device => <div key={device.device_id} className="encryption-device">
      <p>{device.device_id === snapshot.currentDeviceId ? 'This device' : 'Another device'} — {device.admitted ? 'Approved' : device.revoked ? 'Removed' : 'Awaiting approval'}</p>
      <p className="encryption-fingerprint"><code>{device.device_id}</code></p>
      {!device.admitted && !device.revoked && device.device_certificate && <button disabled={blocked} onClick={() => void act(c => c.approveChannelDevice(channelId, device.device_certificate!))}>Approve device</button>}
      {device.admitted && device.device_id !== snapshot.currentDeviceId && <button disabled={blocked} onClick={() => setRemove(device.device_id)}>Remove from channel</button>}
      {remove === device.device_id && <div>
        <p>Remove this device from this channel and rotate future encryption keys?</p>
        <button disabled={blocked} onClick={() => setRemove(null)}>Cancel removal</button>
        <button disabled={blocked} onClick={() => void act(c => c.revokeChannelDevice(channelId, device.device_id))}>Confirm removal</button>
      </div>}
    </div>)}
    <div className="dialog-actions">
      <button disabled={blocked} onClick={() => { setError(null); setRefresh(value => value + 1); }}>Refresh devices</button>
      <button disabled={busy} onClick={onClose}>Done</button>
    </div>
  </Modal>;
}
