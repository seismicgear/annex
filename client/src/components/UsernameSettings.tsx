/**
 * Username settings — set your server-scoped encrypted username and manage
 * visibility grants to other users.
 *
 * Only shown when `usernames_enabled` is true in server policy.
 * The username is encrypted at rest on the server and only visible to
 * users you explicitly grant access to.
 */

import { useState, useEffect, useCallback } from 'react';
import { useIdentityStore } from '@/stores/identity';
import * as api from '@/lib/api';
import type { MemberInfo } from '@/lib/api';

interface Props {
  onClose: () => void;
}

export function UsernameSettings({ onClose }: Props) {
  const identity = useIdentityStore((s) => s.identity);
  const pseudonymId = identity?.pseudonymId ?? '';

  const [username, setUsername] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);

  // Grants state
  const [grantees, setGrantees] = useState<string[]>([]);
  const [members, setMembers] = useState<MemberInfo[]>([]);
  const [loadingGrants, setLoadingGrants] = useState(true);
  const [granting, setGranting] = useState<string | null>(null);

  const loadGrants = useCallback(async () => {
    if (!pseudonymId) return;
    try {
      const resp = await api.listUsernameGrants(pseudonymId);
      setGrantees(resp.grantees);
    } catch {
      // Ignore grant load errors (usernames may be disabled)
    } finally {
      setLoadingGrants(false);
    }
  }, [pseudonymId]);

  const loadMembers = useCallback(async () => {
    if (!pseudonymId) return;
    try {
      const list = await api.listMembers(pseudonymId);
      setMembers(list.filter((m) => m.pseudonym_id !== pseudonymId));
    } catch {
      // May not have permission to list members
    }
  }, [pseudonymId]);

  useEffect(() => {
    loadGrants();
    loadMembers();
  }, [loadGrants, loadMembers]);

  const handleSetUsername = async () => {
    if (!pseudonymId || !username.trim()) return;
    setSaving(true);
    setError(null);
    setSuccess(null);
    try {
      await api.setUsername(pseudonymId, username.trim());
      setSuccess('Username saved.');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleDeleteUsername = async () => {
    if (!pseudonymId) return;
    setSaving(true);
    setError(null);
    setSuccess(null);
    try {
      await api.deleteUsername(pseudonymId);
      setUsername('');
      setGrantees([]);
      setSuccess('Username removed.');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleGrant = async (targetPseudonym: string) => {
    if (!pseudonymId) return;
    setGranting(targetPseudonym);
    setError(null);
    try {
      await api.grantUsername(pseudonymId, targetPseudonym);
      await loadGrants();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setGranting(null);
    }
  };

  const handleRevoke = async (targetPseudonym: string) => {
    if (!pseudonymId) return;
    setGranting(targetPseudonym);
    setError(null);
    try {
      await api.revokeUsernameGrant(pseudonymId, targetPseudonym);
      await loadGrants();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setGranting(null);
    }
  };

  if (!pseudonymId) return null;

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog profile-switcher" onClick={(e) => e.stopPropagation()}>
        <h3>Username Settings</h3>
        <p className="profile-description">
          Set an encrypted display name visible only to users you grant access to.
          Your username is encrypted on the server and never shared outside this server.
        </p>

        {/* Set Username */}
        <div className="persona-form">
          <label>
            Your Username
            <input
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              placeholder="Enter your display name..."
              maxLength={32}
            />
            <span className="field-hint">Max 32 characters. Encrypted at rest.</span>
          </label>

          {error && <div className="error-message">{error}</div>}
          {success && <div className="success-message">{success}</div>}

          <div className="dialog-actions">
            <button
              className="primary-btn"
              onClick={handleSetUsername}
              disabled={saving || !username.trim()}
            >
              {saving ? 'Saving...' : 'Save Username'}
            </button>
            <button onClick={handleDeleteUsername} disabled={saving}>
              Remove Username
            </button>
          </div>
        </div>

        {/* Visibility Grants */}
        <div className="policy-section" style={{ marginTop: '1rem' }}>
          <h4>Username Visibility</h4>
          <p className="field-hint" style={{ marginTop: 0 }}>
            Choose who can see your username. Others will only see your pseudonym.
          </p>

          {loadingGrants ? (
            <p>Loading...</p>
          ) : (
            <div className="member-list">
              {members.map((m) => {
                const isGranted = grantees.includes(m.pseudonym_id);
                return (
                  <div key={m.pseudonym_id} className="member-row">
                    <div className="member-identity">
                      <span className="member-pseudonym" title={m.pseudonym_id}>
                        {m.pseudonym_id.slice(0, 16)}...
                      </span>
                      <span className="member-meta">
                        {m.participant_type} | {isGranted ? 'Granted' : 'Hidden'}
                      </span>
                    </div>
                    <div className="member-caps">
                      <button
                        className={isGranted ? 'delete-btn' : 'primary-btn'}
                        onClick={() =>
                          isGranted ? handleRevoke(m.pseudonym_id) : handleGrant(m.pseudonym_id)
                        }
                        disabled={granting === m.pseudonym_id}
                        style={{ fontSize: '0.8rem', padding: '0.25rem 0.5rem' }}
                      >
                        {granting === m.pseudonym_id
                          ? '...'
                          : isGranted
                            ? 'Revoke'
                            : 'Grant'}
                      </button>
                    </div>
                  </div>
                );
              })}
              {members.length === 0 && (
                <p className="no-personas">No other members on this server yet.</p>
              )}
            </div>
          )}
        </div>

        <div className="dialog-actions" style={{ marginTop: '1rem' }}>
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
