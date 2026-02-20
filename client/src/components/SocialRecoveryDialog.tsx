/**
 * Social recovery dialog — set up or execute Shamir's Secret Sharing
 * based recovery of a user's master identity key.
 *
 * Setup: Shard the secret key across trusted peers.
 * Recover: Collect shards from peers to reconstruct the key.
 */

import { useState, type FormEvent } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { splitSecretKey, reconstructSecretKey } from '@/lib/shamir';
import type { RecoveryConfig, RecoveryShard } from '@/types';

interface Props {
  onClose: () => void;
}

type Mode = 'choose' | 'setup' | 'setup-complete' | 'recover';

export function SocialRecoveryDialog({ onClose }: Props) {
  const identity = useIdentityStore((s) => s.identity);
  const importBackup = useIdentityStore((s) => s.importBackup);

  const [mode, setMode] = useState<Mode>('choose');
  const [error, setError] = useState<string | null>(null);

  // Setup state
  const [totalShards, setTotalShards] = useState(5);
  const [threshold, setThreshold] = useState(3);
  const [guardians, setGuardians] = useState<Array<{ pseudonymId: string; label: string }>>([
    { pseudonymId: '', label: '' },
    { pseudonymId: '', label: '' },
    { pseudonymId: '', label: '' },
  ]);
  const [recoveryConfig, setRecoveryConfig] = useState<RecoveryConfig | null>(null);
  const [generatedShards, setGeneratedShards] = useState<RecoveryShard[]>([]);
  const [copiedShard, setCopiedShard] = useState<number | null>(null);

  // Recovery state
  const [recoveryShards, setRecoveryShards] = useState<Array<{ index: string; data: string }>>([
    { index: '', data: '' },
    { index: '', data: '' },
    { index: '', data: '' },
  ]);
  const [recoveredSk, setRecoveredSk] = useState<string | null>(null);

  const updateGuardian = (idx: number, field: 'pseudonymId' | 'label', value: string) => {
    setGuardians((g) => g.map((item, i) => (i === idx ? { ...item, [field]: value } : item)));
  };

  const handleSetup = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    if (!identity?.sk) {
      setError('No active identity to protect');
      return;
    }

    // Validate guardians
    const validGuardians = guardians.filter((g) => g.label.trim());
    if (validGuardians.length < totalShards) {
      setError(`Need at least ${totalShards} guardians`);
      return;
    }

    try {
      const shards = splitSecretKey(identity.sk, totalShards, threshold);
      const recoveryShards: RecoveryShard[] = shards.map((s, i) => ({
        index: s.index,
        data: s.data,
        holderPseudonymId: validGuardians[i]?.pseudonymId ?? '',
        holderLabel: validGuardians[i]?.label ?? `Guardian ${i + 1}`,
      }));

      const config: RecoveryConfig = {
        identityId: identity.id,
        totalShards,
        threshold,
        shards: recoveryShards.map((s) => ({
          ...s,
          data: '***', // Don't store shard data in the config
        })),
        createdAt: new Date().toISOString(),
      };

      setRecoveryConfig(config);
      setGeneratedShards(recoveryShards);
      setMode('setup-complete');
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to generate shards');
    }
  };

  const copyShard = async (shard: RecoveryShard) => {
    const shardData = JSON.stringify({
      index: shard.index,
      data: shard.data,
      for: identity?.pseudonymId?.slice(0, 12),
    });
    await navigator.clipboard.writeText(shardData);
    setCopiedShard(shard.index);
    setTimeout(() => setCopiedShard(null), 2000);
  };

  const updateRecoveryShard = (idx: number, field: 'index' | 'data', value: string) => {
    setRecoveryShards((s) =>
      s.map((item, i) => (i === idx ? { ...item, [field]: value } : item)),
    );
  };

  const addRecoveryShardSlot = () => {
    setRecoveryShards((s) => [...s, { index: '', data: '' }]);
  };

  const handleRecover = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);

    const validShards = recoveryShards.filter((s) => s.index && s.data);
    if (validShards.length < 2) {
      setError('Need at least 2 shards to reconstruct');
      return;
    }

    try {
      const sk = reconstructSecretKey(
        validShards.map((s) => ({ index: parseInt(s.index, 10), data: s.data })),
      );
      setRecoveredSk(sk);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Reconstruction failed — check your shards');
    }
  };

  const handleImportRecovered = async () => {
    if (!recoveredSk) return;
    // Build a minimal identity backup that can be completed through re-registration
    const backup = JSON.stringify({
      id: crypto.randomUUID(),
      sk: recoveredSk,
      roleCode: 1,
      nodeId: 0,
      commitmentHex: '',
      pseudonymId: null,
      serverSlug: 'default',
      leafIndex: null,
      createdAt: new Date().toISOString(),
    });

    try {
      await importBackup(backup);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to import recovered key');
    }
  };

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog social-recovery-dialog" onClick={(e) => e.stopPropagation()}>
        <h3>Social Recovery</h3>

        {mode === 'choose' && (
          <div className="recovery-choose">
            <p className="recovery-description">
              Protect your identity by splitting your secret key across trusted peers.
              If you lose your devices, collect shards from your guardians to restore access.
            </p>
            <button
              className="device-link-option"
              onClick={() => setMode('setup')}
              disabled={!identity}
            >
              <span className="device-link-option-icon">&#x1F6E1;</span>
              <span className="device-link-option-text">
                <strong>Set Up Recovery</strong>
                <span>Split your key across trusted guardians</span>
              </span>
            </button>
            <button
              className="device-link-option"
              onClick={() => setMode('recover')}
            >
              <span className="device-link-option-icon">&#x1F504;</span>
              <span className="device-link-option-text">
                <strong>Recover Identity</strong>
                <span>Reconstruct your key from collected shards</span>
              </span>
            </button>
            <div className="dialog-actions">
              <button onClick={onClose}>Cancel</button>
            </div>
          </div>
        )}

        {mode === 'setup' && (
          <form className="recovery-setup" onSubmit={handleSetup}>
            <div className="recovery-params">
              <label>
                Total Guardians
                <input
                  type="number"
                  min={2}
                  max={10}
                  value={totalShards}
                  onChange={(e) => {
                    const val = parseInt(e.target.value, 10);
                    setTotalShards(val);
                    // Ensure enough guardian slots
                    while (guardians.length < val) {
                      guardians.push({ pseudonymId: '', label: '' });
                    }
                    setGuardians([...guardians]);
                  }}
                />
              </label>
              <label>
                Required to Recover
                <input
                  type="number"
                  min={2}
                  max={totalShards}
                  value={threshold}
                  onChange={(e) => setThreshold(parseInt(e.target.value, 10))}
                />
              </label>
            </div>

            <p className="recovery-hint">
              {threshold} of {totalShards} guardians must provide their shard to recover your identity.
            </p>

            <div className="guardian-list">
              <h4>Guardians</h4>
              {guardians.slice(0, totalShards).map((g, i) => (
                <div key={i} className="guardian-entry">
                  <input
                    type="text"
                    placeholder={`Guardian ${i + 1} name`}
                    value={g.label}
                    onChange={(e) => updateGuardian(i, 'label', e.target.value)}
                  />
                  <input
                    type="text"
                    placeholder="Pseudonym ID (optional)"
                    value={g.pseudonymId}
                    onChange={(e) => updateGuardian(i, 'pseudonymId', e.target.value)}
                    className="guardian-pseudo"
                  />
                </div>
              ))}
            </div>

            {error && <div className="error-message">{error}</div>}
            <div className="dialog-actions">
              <button type="button" onClick={() => setMode('choose')}>
                Back
              </button>
              <button type="submit" className="primary-btn">
                Generate Shards
              </button>
            </div>
          </form>
        )}

        {mode === 'setup-complete' && recoveryConfig && (
          <div className="recovery-complete">
            <div className="success-message">
              Recovery shards generated successfully!
            </div>
            <p className="recovery-hint">
              Send each shard to the designated guardian. They should store it securely.
              {recoveryConfig.threshold} of {recoveryConfig.totalShards} shards
              are needed to recover.
            </p>

            <div className="shard-list">
              {generatedShards.map((shard) => (
                <div key={shard.index} className="shard-item">
                  <div className="shard-header">
                    <span className="shard-label">
                      Shard #{shard.index} — {shard.holderLabel}
                    </span>
                    <button
                      className="shard-copy-btn"
                      onClick={() => copyShard(shard)}
                    >
                      {copiedShard === shard.index ? 'Copied!' : 'Copy'}
                    </button>
                  </div>
                  <code className="shard-data">{shard.data.slice(0, 32)}...</code>
                </div>
              ))}
            </div>

            <div className="dialog-actions">
              <button className="primary-btn" onClick={onClose}>
                Done
              </button>
            </div>
          </div>
        )}

        {mode === 'recover' && !recoveredSk && (
          <form className="recovery-reconstruct" onSubmit={handleRecover}>
            <p className="recovery-description">
              Enter the shards collected from your guardians to reconstruct your secret key.
            </p>

            <div className="recovery-shard-inputs">
              {recoveryShards.map((s, i) => (
                <div key={i} className="recovery-shard-entry">
                  <input
                    type="number"
                    placeholder="Shard #"
                    value={s.index}
                    onChange={(e) => updateRecoveryShard(i, 'index', e.target.value)}
                    min={1}
                    max={255}
                    className="recovery-shard-index"
                  />
                  <input
                    type="text"
                    placeholder="Shard data (hex)"
                    value={s.data}
                    onChange={(e) => updateRecoveryShard(i, 'data', e.target.value)}
                    className="recovery-shard-data"
                  />
                </div>
              ))}
              <button
                type="button"
                className="add-shard-btn"
                onClick={addRecoveryShardSlot}
              >
                + Add Another Shard
              </button>
            </div>

            {error && <div className="error-message">{error}</div>}
            <div className="dialog-actions">
              <button type="button" onClick={() => setMode('choose')}>
                Back
              </button>
              <button type="submit" className="primary-btn">
                Reconstruct Key
              </button>
            </div>
          </form>
        )}

        {mode === 'recover' && recoveredSk && (
          <div className="recovery-result">
            <div className="success-message">Key reconstructed successfully!</div>
            <p className="recovery-hint">
              Your secret key has been recovered. Import it to regain access to your identity.
              You will need to re-register with the server to generate a new membership proof.
            </p>
            {error && <div className="error-message">{error}</div>}
            <div className="dialog-actions">
              <button onClick={() => setMode('choose')}>Back</button>
              <button className="primary-btn" onClick={handleImportRecovered}>
                Import Recovered Key
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
