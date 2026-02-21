/**
 * Status bar — shows current identity, persona, connection status, and controls.
 *
 * When a voice call is active, a persistent "Voice Connected" strip appears
 * above the main status row — similar to Discord's bottom-left voice panel.
 *
 * Provides quick access to: device linking, profile switching,
 * social recovery setup, identity export, audio settings, and logout.
 */

import { useState, useEffect, useCallback } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';
import { useVoiceStore } from '@/stores/voice';
import { DeviceLinkDialog } from '@/components/DeviceLinkDialog';
import { ProfileSwitcher } from '@/components/ProfileSwitcher';
import { SocialRecoveryDialog } from '@/components/SocialRecoveryDialog';
import { AudioSettings } from '@/components/AudioSettings';
import { getPersonasForIdentity } from '@/lib/personas';
import type { Persona } from '@/types';

export function StatusBar() {
  const identity = useIdentityStore((s) => s.identity);
  const logout = useIdentityStore((s) => s.logout);
  const exportCurrent = useIdentityStore((s) => s.exportCurrent);
  const wsConnected = useChannelsStore((s) => s.wsConnected);
  const channels = useChannelsStore((s) => s.channels);

  const {
    voiceToken,
    connectedChannelId,
    deafened,
    leaveCall,
    toggleDeafen,
  } = useVoiceStore();

  const [showDeviceLink, setShowDeviceLink] = useState(false);
  const [showProfile, setShowProfile] = useState(false);
  const [showRecovery, setShowRecovery] = useState(false);
  const [showAudioSettings, setShowAudioSettings] = useState(false);
  const [activePersona, setActivePersona] = useState<Persona | null>(null);
  // Local mic muted state — tracks whether the user toggled mute from the status bar.
  // The actual LiveKit mute is handled inside VoicePanel's MediaControls; this is
  // a secondary quick-toggle that mirrors the intent.
  const [micMuted, setMicMuted] = useState(false);

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

  const handleDisconnect = useCallback(async () => {
    if (identity?.pseudonymId) {
      await leaveCall(identity.pseudonymId);
    }
  }, [identity?.pseudonymId, leaveCall]);

  if (!identity) return null;

  const inCall = !!(voiceToken && connectedChannelId);
  const connectedChannel = channels.find((c) => c.channel_id === connectedChannelId);
  const channelLabel = connectedChannel?.name ?? connectedChannelId?.slice(0, 12) ?? '';

  const displayName = activePersona?.displayName
    ?? (identity.pseudonymId ? identity.pseudonymId.slice(0, 16) + '...' : 'No pseudonym');

  return (
    <>
      {/* ── Persistent voice status strip (Discord-style) ── */}
      {inCall && (
        <div className="voice-status-strip">
          <div className="voice-status-info">
            <span className="voice-status-dot" />
            <div className="voice-status-text">
              <span className="voice-status-label">Voice Connected</span>
              <span className="voice-status-channel">{channelLabel}</span>
            </div>
          </div>
          <div className="voice-status-controls">
            <button
              className={`voice-status-btn ${micMuted ? 'muted' : ''}`}
              onClick={() => setMicMuted((m) => !m)}
              title={micMuted ? 'Unmute microphone' : 'Mute microphone'}
            >
              <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
                {micMuted ? (
                  <>
                    <path d="M8 11a3 3 0 003-3V4a3 3 0 10-6 0v4a3 3 0 003 3zm5-3a5 5 0 01-4.5 4.975V15h-1v-2.025A5 5 0 013 8h1a4 4 0 108 0h1z" opacity="0.3"/>
                    <line x1="2" y1="2" x2="14" y2="14" stroke="currentColor" strokeWidth="1.5"/>
                  </>
                ) : (
                  <path d="M8 11a3 3 0 003-3V4a3 3 0 10-6 0v4a3 3 0 003 3zm5-3a5 5 0 01-4.5 4.975V15h-1v-2.025A5 5 0 013 8h1a4 4 0 108 0h1z"/>
                )}
              </svg>
            </button>
            <button
              className={`voice-status-btn ${deafened ? 'muted' : ''}`}
              onClick={toggleDeafen}
              title={deafened ? 'Undeafen — resume hearing others' : 'Deafen — mute all incoming audio'}
            >
              <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
                {deafened ? (
                  <>
                    <path d="M8 1C4.5 1 2 3.5 2 6v4a2 2 0 002 2h1V7H4V6c0-2.2 1.8-4 4-4s4 1.8 4 4v1h-1v5h1a2 2 0 002-2V6c0-2.5-2.5-5-6-5z" opacity="0.3"/>
                    <line x1="2" y1="2" x2="14" y2="14" stroke="currentColor" strokeWidth="1.5"/>
                  </>
                ) : (
                  <path d="M8 1C4.5 1 2 3.5 2 6v4a2 2 0 002 2h1V7H4V6c0-2.2 1.8-4 4-4s4 1.8 4 4v1h-1v5h1a2 2 0 002-2V6c0-2.5-2.5-5-6-5z"/>
                )}
              </svg>
            </button>
            <button
              className="voice-status-btn disconnect"
              onClick={handleDisconnect}
              title="Disconnect from voice channel"
            >
              <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
                <path d="M3.654 1.328a.678.678 0 00-1.015-.063L1.605 2.3c-.483.484-.661 1.169-.45 1.77a17.568 17.568 0 004.168 6.608 17.569 17.569 0 006.608 4.168c.601.211 1.286.033 1.77-.45l1.034-1.034a.678.678 0 00-.063-1.015l-2.307-1.794a.678.678 0 00-.58-.122l-2.19.547a1.745 1.745 0 01-1.657-.459L5.482 8.062a1.745 1.745 0 01-.46-1.657l.548-2.19a.678.678 0 00-.122-.58L3.654 1.328z"/>
              </svg>
            </button>
          </div>
        </div>
      )}

      {/* ── Main status bar ── */}
      <footer className="status-bar">
        <div className="identity-info">
          <span className={`ws-indicator ${wsConnected ? 'connected' : 'disconnected'}`}
                title={wsConnected ? 'Connected to server' : 'Disconnected from server'} />
          {activePersona && (
            <span
              className="persona-indicator"
              style={{ background: activePersona.accentColor }}
              title={`Persona: ${activePersona.displayName}`}
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
          <button
            onClick={() => setShowAudioSettings(true)}
            title="Audio & Video Settings — change your microphone, speakers, camera, and volume levels"
          >
            <svg width="14" height="14" viewBox="0 0 16 16" fill="currentColor">
              <path d="M8 4.754a3.246 3.246 0 1 0 0 6.492 3.246 3.246 0 0 0 0-6.492zM5.754 8a2.246 2.246 0 1 1 4.492 0 2.246 2.246 0 0 1-4.492 0z"/>
              <path d="M9.796 1.343c-.527-1.79-3.065-1.79-3.592 0l-.094.319a.873.873 0 0 1-1.255.52l-.292-.16c-1.64-.892-3.433.902-2.54 2.541l.159.292a.873.873 0 0 1-.52 1.255l-.319.094c-1.79.527-1.79 3.065 0 3.592l.319.094a.873.873 0 0 1 .52 1.255l-.16.292c-.892 1.64.901 3.434 2.541 2.54l.292-.159a.873.873 0 0 1 1.255.52l.094.319c.527 1.79 3.065 1.79 3.592 0l.094-.319a.873.873 0 0 1 1.255-.52l.292.16c1.64.893 3.434-.902 2.54-2.541l-.159-.292a.873.873 0 0 1 .52-1.255l.319-.094c1.79-.527 1.79-3.065 0-3.592l-.319-.094a.873.873 0 0 1-.52-1.255l.16-.292c.893-1.64-.902-3.433-2.541-2.54l-.292.159a.873.873 0 0 1-1.255-.52l-.094-.319zm-2.633.283c.246-.835 1.428-.835 1.674 0l.094.319a1.873 1.873 0 0 0 2.693 1.115l.291-.16c.764-.415 1.6.42 1.184 1.185l-.159.292a1.873 1.873 0 0 0 1.116 2.692l.318.094c.835.246.835 1.428 0 1.674l-.319.094a1.873 1.873 0 0 0-1.115 2.693l.16.291c.415.764-.421 1.6-1.185 1.184l-.291-.159a1.873 1.873 0 0 0-2.693 1.116l-.094.318c-.246.835-1.428.835-1.674 0l-.094-.319a1.873 1.873 0 0 0-2.692-1.115l-.292.16c-.764.415-1.6-.421-1.184-1.185l.159-.291A1.873 1.873 0 0 0 1.945 8.93l-.319-.094c-.835-.246-.835-1.428 0-1.674l.319-.094A1.873 1.873 0 0 0 3.06 4.377l-.16-.292c-.415-.764.42-1.6 1.185-1.184l.292.159a1.873 1.873 0 0 0 2.692-1.116l.094-.318z"/>
            </svg>
          </button>
          <button onClick={() => setShowDeviceLink(true)} title="Link another device — transfer your identity to a second device via QR code">
            Link
          </button>
          <button onClick={() => setShowRecovery(true)} title="Social recovery — split your secret key among trusted peers so you can recover if you lose access">
            Recovery
          </button>
          <button onClick={handleExport} title="Export identity backup — download a JSON file of your cryptographic identity for safekeeping">
            Export
          </button>
          <button onClick={logout} title="Switch identity — log out and choose or create a different identity">
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
      {showAudioSettings && (
        <AudioSettings onClose={() => setShowAudioSettings(false)} />
      )}
    </>
  );
}
