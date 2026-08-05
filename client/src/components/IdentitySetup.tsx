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
    await generateLocalKeys(1); // roleCode 1 = Human
  };

  const handleImport = async () => {
    const file = fileInputRef.current?.files?.[0];
    if (!file) return;
    const text = await file.text();
    // Reset input so the same file can be re-selected
    if (fileInputRef.current) fileInputRef.current.value = '';
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
          {/* Purely decorative: it separates two alternatives that already
              read as alternatives. Announcing "or" on its own is noise. */}
          <span aria-hidden="true">or</span>
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
          <h3 id="import-backup-heading">Import Backup</h3>
          <p className="identity-description">
            Restore an identity from a backup file you exported earlier.
          </p>
          <input
            type="file"
            ref={fileInputRef}
            accept=".json"
            aria-labelledby="import-backup-heading"
          />
          <button onClick={handleImport}>Import</button>
        </div>
      )}

      {showDeviceLink && (
        <DeviceLinkDialog onClose={() => setShowDeviceLink(false)} />
      )}
    </div>
  );
}
