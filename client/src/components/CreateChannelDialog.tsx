/**
 * Create channel dialog — modal form for creating new channels.
 *
 * Only accessible to moderators. Supports TEXT, VOICE, HYBRID, AGENT,
 * and BROADCAST channel types with optional topic and federation scope.
 */

import { useState, type FormEvent } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import type { ChannelType } from '@/types';

const CHANNEL_TYPES: { value: ChannelType; label: string }[] = [
  { value: 'Text', label: 'Text' },
  { value: 'Voice', label: 'Voice' },
  { value: 'Hybrid', label: 'Hybrid (Text + Voice)' },
  { value: 'Agent', label: 'Agent' },
  { value: 'Broadcast', label: 'Broadcast' },
];

export function CreateChannelDialog({ onClose }: { onClose: () => void }) {
  const identity = useIdentityStore((s) => s.identity);
  const { createChannel, loadChannels } = useChannelsStore();

  const [name, setName] = useState('');
  const [channelType, setChannelType] = useState<ChannelType>('Text');
  const [topic, setTopic] = useState('');
  const [federated, setFederated] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!identity?.pseudonymId || !name.trim()) return;

    setSubmitting(true);
    setError(null);
    try {
      await createChannel(identity.pseudonymId, name.trim(), channelType, topic || undefined, federated);
      await loadChannels(identity.pseudonymId);
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog" onClick={(e) => e.stopPropagation()}>
        <h3>Create Channel</h3>
        <form onSubmit={handleSubmit}>
          <label>
            Name
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="general"
              required
              autoFocus
            />
          </label>
          <label>
            Type
            <select
              value={channelType}
              onChange={(e) => setChannelType(e.target.value as ChannelType)}
            >
              {CHANNEL_TYPES.map((t) => (
                <option key={t.value} value={t.value}>
                  {t.label}
                </option>
              ))}
            </select>
          </label>
          <label>
            Topic (optional)
            <input
              type="text"
              value={topic}
              onChange={(e) => setTopic(e.target.value)}
              placeholder="What this channel is about"
            />
          </label>
          <label className="checkbox-label">
            <input
              type="checkbox"
              checked={federated}
              onChange={(e) => setFederated(e.target.checked)}
            />
            Federated
          </label>
          {error && <div className="error-message">{error}</div>}
          <div className="dialog-actions">
            <button type="button" onClick={onClose} disabled={submitting}>
              Cancel
            </button>
            <button type="submit" disabled={submitting || !name.trim()} className="primary-btn">
              {submitting ? 'Creating...' : 'Create'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
