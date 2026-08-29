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
import {
  looksLikeShardJson,
  parseShardPayload,
  serializeShardPayload,
  SHARD_FORMAT_VERSION,
  type ShardPayload,
} from '@/lib/recovery-shard';
import * as zk from '@/lib/zk';
import type { RecoveryConfig, RecoveryShard } from '@/types';
import { Modal } from '@/components/Modal';
import { useDialogTitleId } from '@/lib/use-dialog-title-id';

interface Props {
  onClose: () => void;
}

type Mode = 'choose' | 'setup' | 'setup-complete' | 'recover';

/** One row of the recover form: what was typed, plus what it parsed to. */
interface RecoveryShardEntry {
  index: string;
  data: string;
  payload: ShardPayload | null;
}

export function SocialRecoveryDialog({ onClose }: Props) {
  const titleId = useDialogTitleId();
  const identity = useIdentityStore((s) => s.identity);
  const importBackup = useIdentityStore((s) => s.importBackup);

  const [mode, setMode] = useState<Mode>('choose');
  const [error, setError] = useState<string | null>(null);

  // Setup state
  const [totalShards, setTotalShards] = useState(5);
  const [threshold, setThreshold] = useState(3);
  /**
   * Named guardians, one per shard.
   *
   * This used to be seeded with three entries while `totalShards` started at
   * five, and only grew when the user TOUCHED the Total Guardians field. So on
   * first open the dialog offered three name boxes, asked for five guardians,
   * and refused to submit — with no visible way to add the missing two. The
   * rendered rows are derived from `totalShards` below so the two cannot
   * disagree again.
   */
  const [guardians, setGuardians] = useState<Array<{ pseudonymId: string; label: string }>>(
    () => Array.from({ length: 5 }, () => ({ pseudonymId: '', label: '' })),
  );
  const [recoveryConfig, setRecoveryConfig] = useState<RecoveryConfig | null>(null);
  const [generatedShards, setGeneratedShards] = useState<RecoveryShard[]>([]);
  const [copiedShard, setCopiedShard] = useState<number | null>(null);
  /** Shard JSON shown in a read-only fallback field when clipboard write fails. */
  const [fallbackShardText, setFallbackShardText] = useState<string | null>(null);

  // Recovery state
  const [recoveryShards, setRecoveryShards] = useState<RecoveryShardEntry[]>([
    { index: '', data: '', payload: null },
    { index: '', data: '', payload: null },
    { index: '', data: '', payload: null },
  ]);
  const [recoveredSk, setRecoveredSk] = useState<string | null>(null);
  /** The verified parameters the recovered key belongs to. */
  const [recoveredMeta, setRecoveredMeta] = useState<ShardPayload | null>(null);
  const [importSuccess, setImportSuccess] = useState(false);

  /**
   * Exactly `totalShards` rows, whatever the backing array happens to hold.
   * Deriving them is what keeps "Total Guardians: 5" from rendering three
   * boxes.
   */
  const guardianSlots = Array.from(
    { length: totalShards },
    (_, i) => guardians[i] ?? { pseudonymId: '', label: '' },
  );

  const updateGuardian = (idx: number, field: 'pseudonymId' | 'label', value: string) => {
    setGuardians((g) => {
      // The rendered rows come from `totalShards`, which can exceed what the
      // array holds; pad rather than dropping the edit on the floor.
      const next = g.length > idx ? [...g] : [
        ...g,
        ...Array.from({ length: idx + 1 - g.length }, () => ({ pseudonymId: '', label: '' })),
      ];
      next[idx] = { ...next[idx], [field]: value };
      return next;
    });
  };

  const handleSetup = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    if (!identity?.sk) {
      setError('No active identity to protect');
      return;
    }

    // Validate the rows the user can actually see and fill.
    const validGuardians = guardianSlots.filter((g) => g.label.trim());
    if (validGuardians.length < totalShards) {
      const missing = totalShards - validGuardians.length;
      setError(
        `Name all ${totalShards} guardians — ${missing} still ` +
          `${missing === 1 ? 'needs' : 'need'} a name.`,
      );
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
    if (!identity) return;
    // Everything a recovery needs: the share itself, how many shares it takes,
    // and the public identity parameters to check the result against. The old
    // payload carried only `index`/`data`, which the recover screen could not
    // read and which left the reconstruction unverifiable.
    const shardData = serializeShardPayload({
      v: SHARD_FORMAT_VERSION,
      index: shard.index,
      data: shard.data,
      threshold,
      totalShards,
      roleCode: identity.roleCode,
      nodeId: identity.nodeId,
      commitment: identity.commitmentHex,
      for: identity.pseudonymId?.slice(0, 12),
    });
    setFallbackShardText(null);
    try {
      await navigator.clipboard.writeText(shardData);
      setCopiedShard(shard.index);
      setTimeout(() => setCopiedShard(null), 2000);
    } catch {
      // Clipboard API denied (e.g. Tauri webview) — show inline fallback
      setFallbackShardText(shardData);
      setError('Clipboard access denied. Copy the shard data manually from the field below.');
    }
  };

  const updateRecoveryShard = (idx: number, field: 'index' | 'data', value: string) => {
    setRecoveryShards((s) =>
      s.map((item, i) => {
        if (i !== idx) return item;
        if (field === 'data') {
          // What a guardian was given is the JSON blob from the setup screen,
          // so that is what they will paste. Unpack it into the row instead of
          // rejecting it as "not hex".
          const payload = parseShardPayload(value);
          if (payload) {
            return { index: String(payload.index), data: payload.data, payload };
          }
          return { ...item, data: value, payload: null };
        }
        return { ...item, index: value };
      }),
    );
  };

  const addRecoveryShardSlot = () => {
    setRecoveryShards((s) => [...s, { index: '', data: '', payload: null }]);
  };

  /**
   * Reconstruct, then CHECK.
   *
   * Shamir cannot tell you that you supplied too few shares — interpolating
   * k < threshold points returns a wrong 32-byte key, indistinguishable from a
   * right one, and `reconstruct` only refuses fewer than two. This screen used
   * to hand that straight to "Key reconstructed successfully!" and offer to
   * import it, on the one path a user reaches after losing everything else.
   *
   * So the shards carry the identity's public parameters, and the result is
   * only accepted once recomputing the commitment from it reproduces the one
   * the shards agree on.
   */
  const handleRecover = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);

    const filled = recoveryShards.filter((s) => s.index && s.data);
    const payloads = filled.map((s) => s.payload).filter((p): p is ShardPayload => p !== null);

    if (payloads.length !== filled.length || payloads.length === 0) {
      const legacy = filled.some((s) => s.payload === null && looksLikeShardJson(s.data));
      setError(
        legacy
          ? 'These shards were generated by an older version and do not carry the ' +
            'information needed to check the recovered key. Generate a new set from ' +
            'a device that still has your identity.'
          : 'Paste the whole shard your guardian sent you — the block starting with "{". ' +
            'A bare hex string carries nothing to check the recovered key against.',
      );
      return;
    }

    const commitment = payloads[0].commitment;
    if (payloads.some((p) => p.commitment !== commitment)) {
      setError('These shards belong to different identities. Use shards from a single set.');
      return;
    }

    const indices = new Set(payloads.map((p) => p.index));
    if (indices.size !== payloads.length) {
      setError('The same shard was entered twice. Each guardian holds a different one.');
      return;
    }

    const needed = payloads[0].threshold;
    if (payloads.length < needed) {
      setError(
        `${payloads.length} of the ${needed} shards needed. ` +
          'Collect the rest before reconstructing — fewer will not produce your key.',
      );
      return;
    }

    try {
      const sk = reconstructSecretKey(
        payloads.map((p) => ({ index: p.index, data: p.data })),
      );
      await zk.initPoseidon();
      const recomputed = await zk.computeCommitment(
        BigInt('0x' + sk),
        payloads[0].roleCode,
        payloads[0].nodeId,
      );
      if (recomputed.toLowerCase() !== commitment.toLowerCase()) {
        setError(
          'The shards did not reconstruct your identity. Check that each one was ' +
            'pasted whole and unmodified, and that they all come from the same set.',
        );
        return;
      }
      setRecoveredSk(sk);
      setRecoveredMeta(payloads[0]);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Reconstruction failed — check your shards');
    }
  };

  const handleImportRecovered = async () => {
    if (!recoveredSk || !recoveredMeta) return;
    setError(null);

    try {
      // Restore the identity the shards describe.
      //
      // This used to call `generateNodeId()` — a fresh RANDOM value — and
      // hardcode `roleCode: 1`, then derive a commitment from them. A
      // commitment over a random node id is a different Merkle leaf, so even a
      // perfectly reconstructed secret key produced a NEW identity rather than
      // the one being recovered. The parameters travel with the shards now,
      // and `handleRecover` has already verified they reproduce the
      // commitment.
      const { roleCode, nodeId, commitment: commitmentHex } = recoveredMeta;

      const backup = JSON.stringify({
        id: crypto.randomUUID(),
        sk: recoveredSk,
        roleCode,
        nodeId,
        commitmentHex,
        pseudonymId: null,
        sessionToken: null,
        serverSlug: '',
        leafIndex: null,
        createdAt: new Date().toISOString(),
      });

      await importBackup(backup);
      setImportSuccess(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to import recovered key');
    }
  };

  return (
    <Modal onClose={onClose} className="social-recovery-dialog" titleId={titleId} focusKey={mode}>
      <h2 id={titleId}>Social Recovery</h2>

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
                  // Clamp threshold if it now exceeds the new total.
                  // Guardian rows follow `totalShards` at render time, so
                  // there is no slot bookkeeping to do here.
                  if (threshold > val) setThreshold(val);
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
            <h3>Guardians</h3>
            {guardianSlots.map((g, i) => (
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

          {error && <div className="error-message" role="alert">{error}</div>}
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

          {error && <div className="error-message" role="alert">{error}</div>}
          {fallbackShardText && (
            <div className="shard-fallback" onClick={(e) => e.stopPropagation()}>
              <input
                type="text"
                readOnly
                value={fallbackShardText}
                className="share-link-input"
                autoFocus
                onFocus={(e) => e.target.select()}
              />
              <button
                onClick={(e) => { e.stopPropagation(); setFallbackShardText(null); setError(null); }}
                className="channel-error-dismiss"
                aria-label="Dismiss"
              >
                &times;
              </button>
            </div>
          )}

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
            Paste the shards your guardians sent you — each one whole, exactly as
            they received it. The shard number fills itself in.
          </p>

          <div className="recovery-shard-inputs">
            {recoveryShards.map((s, i) => (
              <div key={i} className="recovery-shard-entry">
                {/* The number box is 60px wide, so its "Shard #" placeholder
                    rendered as "Shar" with the spinner arrows on top of it —
                    the only thing naming the field was clipped. It is labelled
                    properly now, and normally filled by the paste below rather
                    than typed. */}
                <label className="visually-hidden" htmlFor={`shard-index-${i}`}>
                  Shard number for entry {i + 1}
                </label>
                <input
                  id={`shard-index-${i}`}
                  type="number"
                  placeholder="#"
                  value={s.index}
                  onChange={(e) => updateRecoveryShard(i, 'index', e.target.value)}
                  min={1}
                  max={255}
                  className="recovery-shard-index"
                />
                <label className="visually-hidden" htmlFor={`shard-data-${i}`}>
                  Shard {i + 1}, as sent by your guardian
                </label>
                <input
                  id={`shard-data-${i}`}
                  type="text"
                  placeholder="Paste the shard your guardian sent"
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

          {error && <div className="error-message" role="alert">{error}</div>}
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

      {mode === 'recover' && recoveredSk && !importSuccess && (
        <div className="recovery-result">
          <div className="success-message">Key reconstructed successfully!</div>
          <p className="recovery-hint">
            Your secret key has been recovered. Import it to regain access to your identity.
            You will need to re-register with the server to generate a new membership proof.
          </p>
          {error && <div className="error-message" role="alert">{error}</div>}
          <div className="dialog-actions">
            <button onClick={() => setMode('choose')}>Back</button>
            <button className="primary-btn" onClick={handleImportRecovered}>
              Import Recovered Key
            </button>
          </div>
        </div>
      )}

      {mode === 'recover' && importSuccess && (
        <div className="recovery-result">
          <div className="success-message">Identity restored locally!</div>
          <p className="recovery-hint">
            Your identity keys have been recovered and saved to this device.
            To use this identity on a server, you still need to register it
            — go to the server connection screen and complete registration.
          </p>
          <div className="dialog-actions">
            <button className="primary-btn" onClick={onClose}>
              Done
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}
