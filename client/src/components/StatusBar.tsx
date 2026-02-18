/**
 * Status bar — shows current identity, connection status, and controls.
 */

import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';

export function StatusBar() {
  const identity = useIdentityStore((s) => s.identity);
  const logout = useIdentityStore((s) => s.logout);
  const exportCurrent = useIdentityStore((s) => s.exportCurrent);
  const wsConnected = useChannelsStore((s) => s.wsConnected);

  const handleExport = () => {
    const json = exportCurrent();
    if (!json) return;
    const blob = new Blob([json], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `annex-identity-${identity?.pseudonymId?.slice(0, 8)}.json`;
    a.click();
    URL.revokeObjectURL(url);
  };

  if (!identity) return null;

  return (
    <footer className="status-bar">
      <div className="identity-info">
        <span className={`ws-indicator ${wsConnected ? 'connected' : 'disconnected'}`} />
        <span className="pseudonym" title={identity.pseudonymId ?? undefined}>
          {identity.pseudonymId?.slice(0, 16) ?? 'No pseudonym'}...
        </span>
      </div>
      <div className="status-actions">
        <button onClick={handleExport} title="Export identity backup">
          Export
        </button>
        <button onClick={logout} title="Switch identity">
          Logout
        </button>
      </div>
    </footer>
  );
}
