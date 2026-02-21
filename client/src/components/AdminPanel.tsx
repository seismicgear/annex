/**
 * Admin panel — server policy editor and channel management.
 *
 * Only accessible to users with can_moderate permission.
 */

import { useEffect, useState } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';
import * as api from '@/lib/api';
import type { ServerPolicy } from '@/types';

function PolicyEditor({ pseudonymId }: { pseudonymId: string }) {
  const [policy, setPolicy] = useState<ServerPolicy | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);

  // Editable fields for list items
  const [newPrinciple, setNewPrinciple] = useState('');
  const [newProhibited, setNewProhibited] = useState('');
  const [newCapability, setNewCapability] = useState('');

  useEffect(() => {
    api
      .getPolicy(pseudonymId)
      .then(setPolicy)
      .catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setLoading(false));
  }, [pseudonymId]);

  const handleSave = async () => {
    if (!policy) return;
    setSaving(true);
    setError(null);
    setSuccess(null);
    try {
      const result = await api.updatePolicy(pseudonymId, policy);
      setSuccess(`Policy updated (version: ${result.version_id.slice(0, 8)}...)`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  if (loading) return <p>Loading policy...</p>;
  if (!policy) return <p className="error-message">Failed to load policy</p>;

  return (
    <div className="policy-editor">
      <h3>Server Policy</h3>

      <div className="policy-grid">
        <label title="Minimum VRP alignment score (0.0–1.0) required for AI agents to join this server. Higher values require stronger value alignment.">
          Min Alignment Score
          <input
            type="number"
            step="0.1"
            min="0"
            max="1"
            value={policy.agent_min_alignment_score}
            onChange={(e) => {
              const val = parseFloat(e.target.value);
              if (!Number.isNaN(val)) {
                setPolicy({ ...policy, agent_min_alignment_score: Math.min(1, Math.max(0, val)) });
              }
            }}
          />
          <span className="field-hint">AI agents must meet this alignment threshold to participate.</span>
        </label>

        <label title="Maximum number of members allowed on this server.">
          Max Members
          <input
            type="number"
            min="1"
            value={policy.max_members}
            onChange={(e) =>
              setPolicy({ ...policy, max_members: parseInt(e.target.value) || 1 })
            }
          />
          <span className="field-hint">Limits how many users can register on this server.</span>
        </label>

        <label title="How many days messages are kept before automatic deletion. Older messages are purged to save storage.">
          Retention (days)
          <input
            type="number"
            min="1"
            value={policy.default_retention_days}
            onChange={(e) =>
              setPolicy({ ...policy, default_retention_days: parseInt(e.target.value) || 1 })
            }
          />
          <span className="field-hint">Messages older than this are automatically deleted.</span>
        </label>

        <label className="checkbox-label" title="When enabled, this server can connect to and exchange messages with other Annex servers. Disable to keep this server completely isolated.">
          <input
            type="checkbox"
            checked={policy.federation_enabled}
            onChange={(e) => setPolicy({ ...policy, federation_enabled: e.target.checked })}
          />
          Federation Enabled
          <span className="field-hint">Allow connecting to other Annex servers to share channels and messages.</span>
        </label>

        <label className="checkbox-label" title="When enabled, users can create voice/video channels and make real-time calls. Disable to restrict the server to text-only communication.">
          <input
            type="checkbox"
            checked={policy.voice_enabled}
            onChange={(e) => setPolicy({ ...policy, voice_enabled: e.target.checked })}
          />
          Voice Enabled
          <span className="field-hint">Allow voice and video calls on this server.</span>
        </label>
      </div>

      <div className="policy-section">
        <h4>Rate Limits (per minute)</h4>
        <p className="field-hint" style={{ marginTop: 0 }}>Controls how many requests a single user can make per minute. Lower values protect against abuse but may slow down legitimate usage.</p>
        <div className="policy-grid">
          <label title="Maximum identity registrations allowed per minute from a single source.">
            Registration
            <input
              type="number"
              min="1"
              value={policy.rate_limit.registration_limit}
              onChange={(e) =>
                setPolicy({
                  ...policy,
                  rate_limit: {
                    ...policy.rate_limit,
                    registration_limit: parseInt(e.target.value) || 1,
                  },
                })
              }
            />
          </label>
          <label>
            Verification
            <input
              type="number"
              min="1"
              value={policy.rate_limit.verification_limit}
              onChange={(e) =>
                setPolicy({
                  ...policy,
                  rate_limit: {
                    ...policy.rate_limit,
                    verification_limit: parseInt(e.target.value) || 1,
                  },
                })
              }
            />
          </label>
          <label>
            Default
            <input
              type="number"
              min="1"
              value={policy.rate_limit.default_limit}
              onChange={(e) =>
                setPolicy({
                  ...policy,
                  rate_limit: {
                    ...policy.rate_limit,
                    default_limit: parseInt(e.target.value) || 1,
                  },
                })
              }
            />
          </label>
        </div>
      </div>

      <div className="policy-section">
        <h4>Required Agent Capabilities</h4>
        <ul className="tag-list">
          {policy.agent_required_capabilities.map((cap, i) => (
            <li key={i} className="tag-item">
              {cap}
              <button
                onClick={() =>
                  setPolicy({
                    ...policy,
                    agent_required_capabilities: policy.agent_required_capabilities.filter(
                      (_, j) => j !== i,
                    ),
                  })
                }
              >
                x
              </button>
            </li>
          ))}
        </ul>
        <div className="tag-input">
          <input
            type="text"
            value={newCapability}
            onChange={(e) => setNewCapability(e.target.value)}
            placeholder="Add capability..."
            onKeyDown={(e) => {
              if (e.key === 'Enter' && newCapability.trim()) {
                e.preventDefault();
                setPolicy({
                  ...policy,
                  agent_required_capabilities: [
                    ...policy.agent_required_capabilities,
                    newCapability.trim(),
                  ],
                });
                setNewCapability('');
              }
            }}
          />
        </div>
      </div>

      <div className="policy-section">
        <h4>Principles</h4>
        <ul className="tag-list">
          {policy.principles.map((p, i) => (
            <li key={i} className="tag-item">
              {p}
              <button
                onClick={() =>
                  setPolicy({
                    ...policy,
                    principles: policy.principles.filter((_, j) => j !== i),
                  })
                }
              >
                x
              </button>
            </li>
          ))}
        </ul>
        <div className="tag-input">
          <input
            type="text"
            value={newPrinciple}
            onChange={(e) => setNewPrinciple(e.target.value)}
            placeholder="Add principle..."
            onKeyDown={(e) => {
              if (e.key === 'Enter' && newPrinciple.trim()) {
                e.preventDefault();
                setPolicy({
                  ...policy,
                  principles: [...policy.principles, newPrinciple.trim()],
                });
                setNewPrinciple('');
              }
            }}
          />
        </div>
      </div>

      <div className="policy-section">
        <h4>Prohibited Actions</h4>
        <ul className="tag-list">
          {policy.prohibited_actions.map((p, i) => (
            <li key={i} className="tag-item">
              {p}
              <button
                onClick={() =>
                  setPolicy({
                    ...policy,
                    prohibited_actions: policy.prohibited_actions.filter((_, j) => j !== i),
                  })
                }
              >
                x
              </button>
            </li>
          ))}
        </ul>
        <div className="tag-input">
          <input
            type="text"
            value={newProhibited}
            onChange={(e) => setNewProhibited(e.target.value)}
            placeholder="Add prohibited action..."
            onKeyDown={(e) => {
              if (e.key === 'Enter' && newProhibited.trim()) {
                e.preventDefault();
                setPolicy({
                  ...policy,
                  prohibited_actions: [...policy.prohibited_actions, newProhibited.trim()],
                });
                setNewProhibited('');
              }
            }}
          />
        </div>
      </div>

      {error && <div className="error-message">{error}</div>}
      {success && <div className="success-message">{success}</div>}

      <button className="primary-btn save-policy-btn" onClick={handleSave} disabled={saving}>
        {saving ? 'Saving...' : 'Save Policy'}
      </button>
    </div>
  );
}

function ChannelManager({ pseudonymId }: { pseudonymId: string }) {
  const { channels, loadChannels } = useChannelsStore();
  const [deleting, setDeleting] = useState<string | null>(null);

  useEffect(() => {
    loadChannels(pseudonymId);
  }, [pseudonymId, loadChannels]);

  const handleDelete = async (channelId: string) => {
    if (!confirm('Delete this channel? This cannot be undone.')) return;
    setDeleting(channelId);
    try {
      await api.deleteChannel(pseudonymId, channelId);
      await loadChannels(pseudonymId);
    } catch (err) {
      alert(err instanceof Error ? err.message : String(err));
    } finally {
      setDeleting(null);
    }
  };

  return (
    <div className="channel-manager">
      <h3>Channel Management</h3>
      {channels.length === 0 && <p className="no-channels">No channels</p>}
      <div className="channel-manager-list">
        {channels.map((ch) => (
          <div key={ch.channel_id} className="channel-manager-item">
            <div className="channel-manager-info">
              <span className="channel-manager-name">{ch.name}</span>
              <span className="channel-manager-meta">
                {ch.channel_type} | {ch.federation_scope}
              </span>
            </div>
            <button
              className="delete-btn"
              onClick={() => handleDelete(ch.channel_id)}
              disabled={deleting === ch.channel_id}
            >
              {deleting === ch.channel_id ? '...' : 'Delete'}
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}

export function AdminPanel({ section }: { section?: 'policy' | 'channels' }) {
  const identity = useIdentityStore((s) => s.identity);

  if (!identity?.pseudonymId) return null;

  return (
    <div className="admin-panel">
      <h2>{section === 'channels' ? 'Channel Management' : 'Server Policy'}</h2>
      {section === 'channels' ? (
        <ChannelManager pseudonymId={identity.pseudonymId} />
      ) : (
        <PolicyEditor pseudonymId={identity.pseudonymId} />
      )}
    </div>
  );
}
