/**
 * Identity setup component — Screen 1 of the startup flow.
 *
 * Displayed when no identity keys exist locally. Generates ZK keys
 * OFFLINE — makes ZERO network requests. The user creates or imports
 * identity keys here; server interaction happens on the next screen.
 */

import { useState, useRef, type FormEvent } from 'react';
import { useIdentityStore, type IdentityPhase } from '@/stores/identity';
import { DeviceLinkDialog } from '@/components/DeviceLinkDialog';
import { isTauri } from '@/lib/tauri';
import { clearWebStartupMode } from '@/lib/startup-prefs';

const PHASE_LABELS: Partial<Record<IdentityPhase, string>> = {
  uninitialized: 'Ready to create identity',
  generating: 'Generating cryptographic keys...',
  keys_ready: 'Keys ready',
  error: 'Error',
};

export function IdentitySetup() {
  const {
    phase,
    error,
    storedIdentities,
    generateLocalKeys,
    selectIdentity,
    importBackup,
  } = useIdentityStore();

  const fileInputRef = useRef<HTMLInputElement>(null);
  const isWorking = phase === 'generating';
  const [showDeviceLink, setShowDeviceLink] = useState(false);

  const handleCreate = async (e: FormEvent) => {
    e.preventDefault();

    // Clear any existing startup prefs to ensure the user is asked to choose a server
    // for this new identity (discarding choices made for previous identities).
    if (isTauri()) {
      const { clearStartupMode } = await import('@/lib/tauri');
      await clearStartupMode();
    } else {
      clearWebStartupMode();
    }

    await generateLocalKeys(1); // roleCode 1 = Human
  };

  const handleImport = async () => {
    const file = fileInputRef.current?.files?.[0];
    if (!file) return;

    // Clear existing prefs so imported identity must choose server connection
    if (isTauri()) {
      const { clearStartupMode } = await import('@/lib/tauri');
      await clearStartupMode();
    } else {
      clearWebStartupMode();
    }

    const text = await file.text();
    await importBackup(text);
  };

  // Show identities that have keys (regardless of registration status).
  const existingIdentities = storedIdentities.filter((i) => i.sk);

  return (
    <div className="identity-setup">
      <h2>Create Your Identity</h2>

      {/* Status */}
      <div className={`phase-status phase-${phase}`}>
        {PHASE_LABELS[phase] ?? ''}
      </div>
      {error && <div className="error-message">{error}</div>}

      {/* Create new identity */}
      {!isWorking && (
        <form onSubmit={handleCreate} className="create-form">
          <p className="identity-description">
            Generate a new cryptographic identity. Your keys are created
            and stored locally on this device.
          </p>
          <button type="submit" disabled={isWorking}>
            Create New Identity
          </button>
        </form>
      )}

      {/* Device linking — transfer identity from another device */}
      {!isWorking && (
        <div className="setup-divider">
          <span>or</span>
        </div>
      )}
      {!isWorking && (
        <button
          className="device-link-setup-btn"
          onClick={() => setShowDeviceLink(true)}
        >
          Link from Another Device
        </button>
      )}

      {/* Select existing identity */}
      {existingIdentities.length > 0 && !isWorking && (
        <div className="existing-identities">
          <h3>Existing Identities</h3>
          {existingIdentities.map((id) => (
            <button
              key={id.id}
              onClick={() => selectIdentity(id.id)}
              className="identity-option"
            >
              <span className="pseudonym">
                {id.pseudonymId
                  ? `${id.pseudonymId.slice(0, 16)}...`
                  : `${id.commitmentHex.slice(0, 16)}...`}
              </span>
            </button>
          ))}
        </div>
      )}

      {/* Import backup */}
      {!isWorking && (
        <div className="import-section">
          <h3>Import Backup</h3>
          <input type="file" ref={fileInputRef} accept=".json" />
          <button onClick={handleImport}>Import</button>
        </div>
      )}

      {showDeviceLink && (
        <DeviceLinkDialog onClose={() => setShowDeviceLink(false)} />
      )}
    </div>
  );
}
