/**
 * Identity setup component — handles new identity creation and existing identity selection.
 *
 * Displayed when no active identity is available. Guides the user through
 * key generation, registration, proof generation, and verification.
 */

import { useState, useRef, type FormEvent } from 'react';
import { useIdentityStore, type IdentityPhase } from '@/stores/identity';

const PHASE_LABELS: Record<IdentityPhase, string> = {
  uninitialized: 'Ready to create identity',
  generating: 'Generating cryptographic keys...',
  registering: 'Registering with server...',
  proving: 'Generating zero-knowledge proof...',
  verifying: 'Verifying membership...',
  ready: 'Identity ready',
  error: 'Error',
};

export function IdentitySetup() {
  const {
    phase,
    error,
    storedIdentities,
    createIdentity,
    selectIdentity,
    importBackup,
  } = useIdentityStore();

  const [serverSlug, setServerSlug] = useState('default');
  const fileInputRef = useRef<HTMLInputElement>(null);
  const isWorking = ['generating', 'registering', 'proving', 'verifying'].includes(phase);

  const handleCreate = async (e: FormEvent) => {
    e.preventDefault();
    await createIdentity(1, serverSlug); // roleCode 1 = Human
  };

  const handleImport = async () => {
    const file = fileInputRef.current?.files?.[0];
    if (!file) return;
    const text = await file.text();
    await importBackup(text);
  };

  const readyIdentities = storedIdentities.filter((i) => i.pseudonymId !== null);

  return (
    <div className="identity-setup">
      <h2>Annex Identity</h2>

      {/* Status */}
      <div className={`phase-status phase-${phase}`}>
        {PHASE_LABELS[phase]}
      </div>
      {error && <div className="error-message">{error}</div>}

      {/* Create new identity */}
      {!isWorking && (
        <form onSubmit={handleCreate} className="create-form">
          <label>
            Server:
            <input
              type="text"
              value={serverSlug}
              onChange={(e) => setServerSlug(e.target.value)}
              disabled={isWorking}
            />
          </label>
          <button type="submit" disabled={isWorking}>
            Create New Identity
          </button>
        </form>
      )}

      {/* Select existing identity */}
      {readyIdentities.length > 0 && !isWorking && (
        <div className="existing-identities">
          <h3>Existing Identities</h3>
          {readyIdentities.map((id) => (
            <button
              key={id.id}
              onClick={() => selectIdentity(id.id)}
              className="identity-option"
            >
              <span className="pseudonym">{id.pseudonymId?.slice(0, 16)}...</span>
              <span className="server">{id.serverSlug}</span>
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
    </div>
  );
}
