/**
 * Create channel dialog — modal form for creating new channels.
 *
 * Only accessible to moderators. Supports TEXT, VOICE, AGENT,
 * and BROADCAST channel types with optional topic and federation scope.
 */

import { useState, type FormEvent } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import { InfoTip } from '@/components/InfoTip';
import type { ChannelType } from '@/types';

const CHANNEL_TYPES: { value: ChannelType; label: string; description: string }[] = [
  { value: 'Text', label: 'Text', description: 'A text-only chat channel for messages' },
  { value: 'Voice', label: 'Voice', description: 'A voice/video channel with built-in text chat' },
  { value: 'Agent', label: 'Agent', description: 'A channel where AI agents can participate and respond' },
  { value: 'Broadcast', label: 'Broadcast', description: 'One-to-many announcements — only moderators can post' },
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
              title={CHANNEL_TYPES.find((t) => t.value === channelType)?.description}
            >
              {CHANNEL_TYPES.map((t) => (
                <option key={t.value} value={t.value} title={t.description}>
                  {t.label}
                </option>
              ))}
            </select>
            <span className="field-hint">
              {CHANNEL_TYPES.find((t) => t.value === channelType)?.description}
            </span>
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
          <label className="checkbox-label" title="When enabled, messages in this channel are shared with connected partner servers. Leave off to keep conversations local to this server only.">
            <input
              type="checkbox"
              checked={federated}
              onChange={(e) => setFederated(e.target.checked)}
            />
            Federated<InfoTip text="When on, messages in this channel are shared with partner servers your admin has connected to. Turn off to keep conversations private to this server." />
            <span className="field-hint">
              {federated
                ? 'Messages will be shared with connected partner servers.'
                : 'Messages stay on this server only.'}
            </span>
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
