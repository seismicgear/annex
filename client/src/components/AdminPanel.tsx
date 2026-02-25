/**
 * Admin panel — server settings, policy editor, member management, and channel management.
 *
 * Only accessible to users with can_moderate permission.
 */

import { useEffect, useState, useCallback, useRef } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';
import { useServersStore } from '@/stores/servers';
import { InfoTip } from '@/components/InfoTip';
import * as api from '@/lib/api';
import type { ServerPolicy, AccessMode } from '@/types';
import type { MemberInfo } from '@/lib/api';

// ── Server Settings ──

function ServerSettings({ pseudonymId }: { pseudonymId: string }) {
  const [label, setLabel] = useState('');
  const [slug, setSlug] = useState('');
  const [publicUrl, setPublicUrl] = useState('');
  const [publicUrlInput, setPublicUrlInput] = useState('');
  const [savingUrl, setSavingUrl] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);

  // Server image state — shared via servers store so sidebar icon updates too
  const serverImageUrl = useServersStore((s) => s.serverImageUrl);
  const setServerImageUrl = useServersStore((s) => s.setServerImageUrl);
  const [uploadingImage, setUploadingImage] = useState(false);
  const imageInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    api
      .getServer(pseudonymId)
      .then((s) => {
        setLabel(s.label);
        setSlug(s.slug);
        setPublicUrl(s.public_url);
        setPublicUrlInput(s.public_url);
      })
      .catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setLoading(false));
  }, [pseudonymId]);

  const handleRename = async () => {
    if (!label.trim()) return;
    setSaving(true);
    setError(null);
    setSuccess(null);
    try {
      await api.renameServer(pseudonymId, label.trim());
      setSuccess('Server renamed.');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleSavePublicUrl = async () => {
    const trimmed = publicUrlInput.trim().replace(/\/+$/, '');
    if (trimmed && !/^https?:\/\//i.test(trimmed)) {
      setError('Public URL must start with http:// or https://');
      return;
    }
    setSavingUrl(true);
    setError(null);
    setSuccess(null);
    try {
      const resp = await api.setPublicUrl(pseudonymId, trimmed);
      setPublicUrl(resp.public_url);
      setPublicUrlInput(resp.public_url);
      setSuccess('Public URL saved.');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSavingUrl(false);
    }
  };

  const handleImageUpload = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    const allowed = ['image/jpeg', 'image/png', 'image/gif', 'image/webp'];
    if (!allowed.includes(file.type)) return;
    if (file.size > 10 * 1024 * 1024) return;

    setUploadingImage(true);
    setError(null);
    try {
      const resp = await api.uploadServerImage(pseudonymId, file);
      setServerImageUrl(api.resolveUrl(resp.url));
      setSuccess('Server image updated.');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setUploadingImage(false);
      e.target.value = '';
    }
  };

  const hasPublicUrl = !!publicUrl;
  const shareUrl = hasPublicUrl
    ? `${publicUrl}/#/invite?server=${encodeURIComponent(slug)}&label=${encodeURIComponent(label)}`
    : '';

  const handleCopyLink = async () => {
    if (!shareUrl) return;
    try {
      await navigator.clipboard.writeText(shareUrl);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch { /* clipboard denied */ }
  };

  if (loading) return <p>Loading server settings...</p>;

  return (
    <div className="policy-editor">
      <h3>Server Settings</h3>

      <div className="policy-section">
        <h4>Server Image</h4>
        <p className="field-hint" style={{ marginTop: 0 }}>Upload a logo or icon for your server. Metadata (EXIF, GPS) is stripped automatically.</p>
        <div className="server-image-section">
          {serverImageUrl ? (
            <img src={api.resolveUrl(serverImageUrl)} alt="Server" className="server-image-preview" />
          ) : (
            <div className="server-image-placeholder">No image set</div>
          )}
          <input
            ref={imageInputRef}
            type="file"
            accept="image/jpeg,image/png,image/gif,image/webp"
            onChange={handleImageUpload}
            style={{ display: 'none' }}
          />
          <button
            className="primary-btn"
            onClick={() => imageInputRef.current?.click()}
            disabled={uploadingImage}
          >
            {uploadingImage ? 'Uploading...' : serverImageUrl ? 'Change Image' : 'Upload Image'}
          </button>
        </div>
      </div>

      <label title="The display name of your server visible to members and federation peers.">
        Server Name
        <input
          type="text"
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          maxLength={128}
        />
        <span className="field-hint">The public display name of this server.</span>
      </label>

      <label>
        Server Slug
        <input type="text" value={slug} disabled />
        <span className="field-hint">Unique identifier — cannot be changed after creation.</span>
      </label>

      <div className="policy-section">
        <h4>Public URL</h4>
        <p className="field-hint">
          The publicly-reachable address of this server (e.g. your domain or tunnel URL).
          Invite links and federation use this so anyone in the world can connect.
        </p>
        <div className="share-link-row">
          <input
            type="text"
            value={publicUrlInput}
            onChange={(e) => setPublicUrlInput(e.target.value)}
            placeholder="https://your-server.example.com"
          />
          <button
            className="primary-btn"
            onClick={handleSavePublicUrl}
            disabled={savingUrl || publicUrlInput === publicUrl}
          >
            {savingUrl ? 'Saving...' : 'Save'}
          </button>
        </div>
      </div>

      <div className="policy-section">
        <h4>Share Server</h4>
        {hasPublicUrl ? (
          <p className="field-hint">Send this link to invite people to your server.</p>
        ) : (
          <p className="field-hint" style={{ color: 'var(--warning-color, #f0ad4e)' }}>
            Set a Public URL above to generate a shareable invite link.
          </p>
        )}
        {hasPublicUrl && (
          <div className="share-link-row">
            <input type="text" value={shareUrl} readOnly className="share-link-input" />
            <button className="primary-btn" onClick={handleCopyLink}>
              {copied ? 'Copied!' : 'Copy Link'}
            </button>
          </div>
        )}
      </div>

      {error && <div className="error-message">{error}</div>}
      {success && <div className="success-message">{success}</div>}

      <button className="primary-btn save-policy-btn" onClick={handleRename} disabled={saving || !label.trim()}>
        {saving ? 'Saving...' : 'Save Name'}
      </button>
    </div>
  );
}

// ── Policy Editor ──

const ACCESS_MODES: { value: AccessMode; label: string; description: string }[] = [
  { value: 'public', label: 'Public', description: 'Anyone can register and join freely.' },
  { value: 'invite_only', label: 'Invite Only', description: 'Only users with a share link can join. Registration is blocked otherwise.' },
  { value: 'password', label: 'Password Protected', description: 'Users must enter a server password to register.' },
];

function PolicyEditor({ pseudonymId }: { pseudonymId: string }) {
  const [policy, setPolicy] = useState<ServerPolicy | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);

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

      <div className="policy-section">
        <h4>Access Control<InfoTip text="Controls who can sign up for your server. Public means anyone, invite-only requires a link, and password-protected requires a shared secret." /></h4>
        <p className="field-hint" style={{ marginTop: 0 }}>Determines who can register on this server.</p>
        <label title="Controls how new users can join this server.">
          Access Mode
          <select
            value={policy.access_mode}
            onChange={(e) => setPolicy({ ...policy, access_mode: e.target.value as AccessMode })}
          >
            {ACCESS_MODES.map((m) => (
              <option key={m.value} value={m.value} title={m.description}>
                {m.label}
              </option>
            ))}
          </select>
          <span className="field-hint">
            {ACCESS_MODES.find((m) => m.value === policy.access_mode)?.description}
          </span>
        </label>

        {policy.access_mode === 'password' && (
          <label title="Users must enter this password when joining the server.">
            Server Password
            <input
              type="text"
              value={policy.access_password}
              onChange={(e) => setPolicy({ ...policy, access_password: e.target.value })}
              placeholder="Enter server password..."
            />
            <span className="field-hint">Required for users to register when access mode is password-protected.</span>
          </label>
        )}
      </div>

      <div className="policy-grid">
        <label title="Minimum VRP alignment score (0.0–1.0) required for AI agents to join this server. Higher values require stronger value alignment.">
          Min Alignment Score<InfoTip text="An AI safety score from 0 to 1. Higher values mean AI agents must be more closely aligned with human values to participate on this server. Most servers use 0.5 or above." />
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
          Federation Enabled<InfoTip text="Federation lets your server connect to other Annex servers so users can discover and chat across communities — like email servers that can message each other." />
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

        <label className="checkbox-label" title="When enabled, users can set a display name visible to specific people they grant access to. Usernames are encrypted at rest and never shared with federation peers.">
          <input
            type="checkbox"
            checked={policy.usernames_enabled}
            onChange={(e) => setPolicy({ ...policy, usernames_enabled: e.target.checked })}
          />
          Usernames Enabled<InfoTip text="When on, users can pick a display name that's encrypted on the server. They control exactly who sees it — everyone else sees only an anonymous ID." />
          <span className="field-hint">Allow users to set encrypted display names with per-user visibility grants.</span>
        </label>
      </div>

      <div className="policy-section">
        <h4>Media & File Uploads</h4>
        <p className="field-hint" style={{ marginTop: 0 }}>Control which upload types are allowed and their size limits. MIME types are verified server-side.</p>
        <div className="policy-grid">
          <label className="checkbox-label" title="Allow image uploads (JPEG, PNG, GIF, WebP). EXIF metadata is automatically stripped.">
            <input
              type="checkbox"
              checked={policy.images_enabled}
              onChange={(e) => setPolicy({ ...policy, images_enabled: e.target.checked })}
            />
            Images Enabled
          </label>
          <label title="Maximum image upload size in megabytes.">
            Max Image Size (MB)
            <input
              type="number"
              min="1"
              max="100"
              value={policy.max_image_size_mb}
              onChange={(e) =>
                setPolicy({ ...policy, max_image_size_mb: parseInt(e.target.value) || 1 })
              }
              disabled={!policy.images_enabled}
            />
          </label>

          <label className="checkbox-label" title="Allow video uploads (MP4, WebM, MOV). MIME types are verified via magic bytes.">
            <input
              type="checkbox"
              checked={policy.videos_enabled}
              onChange={(e) => setPolicy({ ...policy, videos_enabled: e.target.checked })}
            />
            Videos Enabled
          </label>
          <label title="Maximum video upload size in megabytes.">
            Max Video Size (MB)
            <input
              type="number"
              min="1"
              max="100"
              value={policy.max_video_size_mb}
              onChange={(e) =>
                setPolicy({ ...policy, max_video_size_mb: parseInt(e.target.value) || 1 })
              }
              disabled={!policy.videos_enabled}
            />
          </label>

          <label className="checkbox-label" title="Allow generic file uploads (PDF, ZIP, TXT, etc.). MIME types are verified to block executables.">
            <input
              type="checkbox"
              checked={policy.files_enabled}
              onChange={(e) => setPolicy({ ...policy, files_enabled: e.target.checked })}
            />
            Files Enabled
          </label>
          <label title="Maximum file upload size in megabytes.">
            Max File Size (MB)
            <input
              type="number"
              min="1"
              max="100"
              value={policy.max_file_size_mb}
              onChange={(e) =>
                setPolicy({ ...policy, max_file_size_mb: parseInt(e.target.value) || 1 })
              }
              disabled={!policy.files_enabled}
            />
          </label>
        </div>
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
        <h4>Required Agent Capabilities<InfoTip text="AI agents must declare specific abilities (like 'summarize' or 'translate') to join this server. List the capabilities you require." /></h4>
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
        <h4>Principles<InfoTip text="Guidelines that AI agents on this server must follow — for example, 'Be helpful and honest' or 'Respect user privacy'." /></h4>
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
        <h4>Prohibited Actions<InfoTip text="Things AI agents are explicitly forbidden from doing — for example, 'Share private messages' or 'Impersonate users'." /></h4>
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

// ── Member Manager ──

const CAP_LABELS: { key: keyof Omit<MemberInfo, 'pseudonym_id' | 'participant_type' | 'active' | 'created_at'>; label: string; hint: string }[] = [
  { key: 'can_moderate', label: 'Moderator', hint: 'Can edit server policy, manage channels, and promote/demote members.' },
  { key: 'can_voice', label: 'Voice', hint: 'Can join voice and video channels.' },
  { key: 'can_invite', label: 'Invite', hint: 'Can generate invite links.' },
  { key: 'can_federate', label: 'Federate', hint: 'Can initiate federation handshakes with other servers.' },
  { key: 'can_bridge', label: 'Bridge', hint: 'Can operate as a protocol bridge.' },
];

function MemberManager({ pseudonymId }: { pseudonymId: string }) {
  const [members, setMembers] = useState<MemberInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [updating, setUpdating] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const list = await api.listMembers(pseudonymId);
      setMembers(list);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [pseudonymId]);

  useEffect(() => { load(); }, [load]);

  const toggleCap = async (member: MemberInfo, cap: string) => {
    setUpdating(member.pseudonym_id);
    const updated = {
      can_voice: member.can_voice,
      can_moderate: member.can_moderate,
      can_invite: member.can_invite,
      can_federate: member.can_federate,
      can_bridge: member.can_bridge,
      [cap]: !(member as unknown as Record<string, boolean>)[cap],
    };
    try {
      await api.updateMemberCapabilities(pseudonymId, member.pseudonym_id, updated);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setUpdating(null);
    }
  };

  if (loading) return <p>Loading members...</p>;

  return (
    <div className="policy-editor">
      <h3>Member Management</h3>
      <p className="field-hint" style={{ marginBottom: '0.75rem' }}>
        Toggle capabilities for each member. The first member (founder) has all permissions by default.
      </p>

      {error && <div className="error-message">{error}</div>}

      <div className="member-list">
        {members.map((m) => (
          <div key={m.pseudonym_id} className="member-row">
            <div className="member-identity">
              <span className="member-pseudonym" title={m.pseudonym_id}>
                {m.pseudonym_id.slice(0, 16)}...
              </span>
              <span className="member-meta">
                {m.participant_type} | {m.active ? 'Active' : 'Inactive'}
              </span>
            </div>
            <div className="member-caps">
              {CAP_LABELS.map(({ key, label, hint }) => (
                <label
                  key={key}
                  className="cap-toggle"
                  title={hint}
                >
                  <input
                    type="checkbox"
                    checked={(m as unknown as Record<string, boolean>)[key]}
                    onChange={() => toggleCap(m, key)}
                    disabled={updating === m.pseudonym_id}
                  />
                  {label}
                </label>
              ))}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

// ── Channel Manager ──

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

// ── Main AdminPanel ──

export function AdminPanel({ section }: { section?: 'policy' | 'channels' | 'members' | 'server' }) {
  const identity = useIdentityStore((s) => s.identity);

  if (!identity?.pseudonymId) return null;

  const titles: Record<string, string> = {
    server: 'Server Settings',
    policy: 'Server Policy',
    members: 'Member Management',
    channels: 'Channel Management',
  };

  const active = section ?? 'policy';

  return (
    <div className="admin-panel">
      <h2>{titles[active] ?? 'Server Policy'}</h2>
      {active === 'server' && <ServerSettings pseudonymId={identity.pseudonymId} />}
      {active === 'policy' && <PolicyEditor pseudonymId={identity.pseudonymId} />}
      {active === 'members' && <MemberManager pseudonymId={identity.pseudonymId} />}
      {active === 'channels' && <ChannelManager pseudonymId={identity.pseudonymId} />}
    </div>
  );
}
