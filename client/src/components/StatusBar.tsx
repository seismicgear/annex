/**
 * Status bar — shows current identity, persona, connection status, and controls.
 *
 * Provides quick access to: device linking, profile switching,
 * social recovery setup, identity export, and logout.
 */

import { useState, useEffect } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';
import { DeviceLinkDialog } from '@/components/DeviceLinkDialog';
import { ProfileSwitcher } from '@/components/ProfileSwitcher';
import { SocialRecoveryDialog } from '@/components/SocialRecoveryDialog';
import { getPersonasForIdentity } from '@/lib/personas';
import type { Persona } from '@/types';

export function StatusBar() {
  const identity = useIdentityStore((s) => s.identity);
  const logout = useIdentityStore((s) => s.logout);
  const exportCurrent = useIdentityStore((s) => s.exportCurrent);
  const wsConnected = useChannelsStore((s) => s.wsConnected);

  const [showDeviceLink, setShowDeviceLink] = useState(false);
  const [showProfile, setShowProfile] = useState(false);
  const [showRecovery, setShowRecovery] = useState(false);
  const [activePersona, setActivePersona] = useState<Persona | null>(null);

  // Load active persona for display
  useEffect(() => {
    if (!identity) return;
    getPersonasForIdentity(identity.id).then((list) => {
      setActivePersona(list[0] ?? null);
    });
  }, [identity, showProfile]);

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

  const displayName = activePersona?.displayName
    ?? (identity.pseudonymId ? identity.pseudonymId.slice(0, 16) + '...' : 'No pseudonym');

  return (
    <>
      <footer className="status-bar">
        <div className="identity-info">
          <span className={`ws-indicator ${wsConnected ? 'connected' : 'disconnected'}`} />
          {activePersona && (
            <span
              className="persona-indicator"
              style={{ background: activePersona.accentColor }}
            >
              {activePersona.displayName.charAt(0).toUpperCase()}
            </span>
          )}
          <button
            className="pseudonym-btn"
            onClick={() => setShowProfile(true)}
            title="Manage personas"
          >
            <span className="pseudonym">{displayName}</span>
            {identity.serverSlug && (
              <span className="server-slug">{identity.serverSlug}</span>
            )}
          </button>
        </div>
        <div className="status-actions">
          <button onClick={() => setShowDeviceLink(true)} title="Link another device">
            Link
          </button>
          <button onClick={() => setShowRecovery(true)} title="Social recovery">
            Recovery
          </button>
          <button onClick={handleExport} title="Export identity backup">
            Export
          </button>
          <button onClick={logout} title="Switch identity">
            Logout
          </button>
        </div>
      </footer>

      {showDeviceLink && (
        <DeviceLinkDialog onClose={() => setShowDeviceLink(false)} />
      )}
      {showProfile && (
        <ProfileSwitcher onClose={() => setShowProfile(false)} />
      )}
      {showRecovery && (
        <SocialRecoveryDialog onClose={() => setShowRecovery(false)} />
      )}
    </>
  );
}
