/**
 * Identity settings — unified panel for persona, color, username, and visibility.
 *
 * Combines persona management (display name, bio, accent color) with
 * server-scoped username and visibility grants in a single dialog.
 */

import { useState, useEffect, useCallback, type FormEvent } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useServersStore } from '@/stores/servers';
import { useUsernameStore } from '@/stores/usernames';
import * as personas from '@/lib/personas';
import * as api from '@/lib/api';
import type { Persona } from '@/types';
import type { MemberInfo } from '@/lib/api';

interface Props {
  onClose: () => void;
}

export function IdentitySettings({ onClose }: Props) {
  const identity = useIdentityStore((s) => s.identity);
  const loadVisibleUsernames = useUsernameStore((s) => s.loadVisibleUsernames);
  const pseudonymId = identity?.pseudonymId ?? '';

  // ── Persona state ──
  const [personaList, setPersonaList] = useState<Persona[]>([]);
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<Persona | null>(null);
  const [displayName, setDisplayName] = useState('');
  const [bio, setBio] = useState('');
  const [accentColor, setAccentColor] = useState(personas.randomAccentColor());

  // ── Username state ──
  const [username, setUsername] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);

  // ── Grants state ──
  const [grantees, setGrantees] = useState<string[]>([]);
  const [members, setMembers] = useState<MemberInfo[]>([]);
  const [loadingGrants, setLoadingGrants] = useState(true);
  const [granting, setGranting] = useState<string | null>(null);

  // ── Load persona list ──
  const loadPersonas = useCallback(async () => {
    if (!identity) return;
    const list = await personas.getPersonasForIdentity(identity.id);
    setPersonaList(list);
  }, [identity]);

  useEffect(() => {
    let cancelled = false;
    if (identity) {
      personas.getPersonasForIdentity(identity.id).then((list) => {
        if (!cancelled) setPersonaList(list);
      });
    }
    return () => { cancelled = true; };
  }, [identity]);

  // ── Load grants & members ──
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

  // ── Persona handlers ──
  const resetForm = () => {
    setDisplayName('');
    setBio('');
    setAccentColor(personas.randomAccentColor());
    setCreating(false);
    setEditing(null);
  };

  const handleCreatePersona = async (e: FormEvent) => {
    e.preventDefault();
    if (!identity || !displayName.trim()) return;
    try {
      const created = await personas.createPersona(
        displayName.trim(),
        identity.id,
        identity.serverSlug,
        bio.trim(),
        null,
        accentColor,
      );
      const server = useServersStore.getState().getActiveServer();
      if (server) {
        await useServersStore.getState().setServerPersona(server.id, created.id, created.accentColor);
      }
      resetForm();
      await loadPersonas();
    } catch {
      // Creation failed — form stays open for retry
    }
  };

  const handleEditPersona = async (e: FormEvent) => {
    e.preventDefault();
    if (!editing) return;
    try {
      await personas.updatePersona({
        ...editing,
        displayName: displayName.trim() || editing.displayName,
        bio: bio.trim(),
        accentColor,
      });
      const server = useServersStore.getState().getActiveServer();
      if (server && server.personaId === editing.id) {
        await useServersStore.getState().setServerPersona(server.id, editing.id, accentColor);
      }
      resetForm();
      await loadPersonas();
    } catch {
      // Update failed — form stays open for retry
    }
  };

  const handleDeletePersona = async (id: string) => {
    try {
      await personas.deletePersona(id);
      await loadPersonas();
    } catch {
      // Delete failed — list remains unchanged
    }
  };

  const startEdit = (persona: Persona) => {
    setEditing(persona);
    setDisplayName(persona.displayName);
    setBio(persona.bio);
    setAccentColor(persona.accentColor);
  };

  const handleQuickColorChange = async (persona: Persona, color: string) => {
    const updated = { ...persona, accentColor: color };
    await personas.updatePersona(updated);
    const server = useServersStore.getState().getActiveServer();
    if (server && server.personaId === persona.id) {
      await useServersStore.getState().setServerPersona(server.id, persona.id, color);
    }
    await loadPersonas();
  };

  // ── Username handlers ──
  const handleSetUsername = async () => {
    if (!pseudonymId || !username.trim()) return;
    setSaving(true);
    setError(null);
    setSuccess(null);
    try {
      await api.setUsername(pseudonymId, username.trim());
      await loadVisibleUsernames(pseudonymId);
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
      await loadVisibleUsernames(pseudonymId);
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

  const activeServer = useServersStore.getState().getActiveServer();

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog profile-switcher" onClick={(e) => e.stopPropagation()}>
        <h3>Identity</h3>

        {/* Current pseudonym reference */}
        {identity && (
          <div className="current-identity-ref">
            <span className="label">Cryptographic ID:</span>
            <span className="pseudonym">{identity.pseudonymId ? `${identity.pseudonymId.slice(0, 16)}...` : 'pending'}</span>
            <span className="server-badge">{identity.serverSlug}</span>
          </div>
        )}

        {/* ── Persona section ── */}
        <div className="policy-section">
          <h4>Persona</h4>

          {/* Persona list */}
          <div className="persona-list">
            {personaList.length === 0 && !creating && (
              <p className="no-personas">
                No personas defined. Create one to customize your display name and color.
              </p>
            )}
            {personaList.map((p) => {
              const isActive = activeServer?.personaId === p.id;
              return (
                <div
                  key={p.id}
                  className={`persona-item ${isActive ? 'active' : ''}`}
                  onClick={async () => {
                    if (activeServer && !isActive) {
                      await useServersStore.getState().setServerPersona(activeServer.id, p.id, p.accentColor);
                      await loadPersonas();
                    }
                  }}
                  style={{ cursor: isActive ? 'default' : 'pointer' }}
                >
                  <div className="persona-avatar" style={{ background: p.accentColor }}>
                    {p.displayName.charAt(0).toUpperCase()}
                  </div>
                  <div className="persona-info">
                    <span className="persona-name">{p.displayName}{isActive ? ' (active)' : ''}</span>
                    <span className="persona-meta">
                      {p.serverSlug} {p.bio && `— ${p.bio}`}
                    </span>
                    {/* Inline color swatches for active persona */}
                    {isActive && (
                      <div className="color-picker" style={{ marginTop: '0.35rem' }}>
                        {personas.ACCENT_COLORS.map((color) => (
                          <button
                            key={color}
                            type="button"
                            className={`color-swatch ${p.accentColor === color ? 'active' : ''}`}
                            style={{ background: color }}
                            onClick={(e) => {
                              e.stopPropagation();
                              handleQuickColorChange(p, color);
                            }}
                          />
                        ))}
                      </div>
                    )}
                  </div>
                  <div className="persona-actions">
                    <button
                      className="persona-edit-btn"
                      onClick={(e) => { e.stopPropagation(); startEdit(p); }}
                      title="Edit"
                    >
                      Edit
                    </button>
                    <button
                      className="persona-delete-btn"
                      onClick={(e) => { e.stopPropagation(); handleDeletePersona(p.id); }}
                      title="Delete"
                    >
                      Del
                    </button>
                  </div>
                </div>
              );
            })}
          </div>

          {/* Create / Edit form */}
          {(creating || editing) && (
            <form
              className="persona-form"
              onSubmit={editing ? handleEditPersona : handleCreatePersona}
            >
              <label>
                Display Name
                <input
                  type="text"
                  value={displayName}
                  onChange={(e) => setDisplayName(e.target.value)}
                  placeholder="e.g. seismicgear"
                  maxLength={32}
                  autoFocus
                />
              </label>
              <label>
                Bio / Status
                <input
                  type="text"
                  value={bio}
                  onChange={(e) => setBio(e.target.value)}
                  placeholder="Optional status or bio"
                  maxLength={120}
                />
              </label>
              <label>
                Accent Color
                <div className="color-picker">
                  {personas.ACCENT_COLORS.map((color) => (
                    <button
                      key={color}
                      type="button"
                      className={`color-swatch ${accentColor === color ? 'active' : ''}`}
                      style={{ background: color }}
                      onClick={() => setAccentColor(color)}
                    />
                  ))}
                </div>
              </label>
              <div className="dialog-actions">
                <button type="button" onClick={resetForm}>
                  Cancel
                </button>
                <button
                  type="submit"
                  className="primary-btn"
                  disabled={!displayName.trim()}
                >
                  {editing ? 'Save Changes' : 'Create Persona'}
                </button>
              </div>
            </form>
          )}

          {!creating && !editing && (
            <button onClick={() => setCreating(true)} className="primary-btn" style={{ marginTop: '0.5rem' }}>
              New Persona
            </button>
          )}
        </div>

        {/* ── Username section ── */}
        <div className="policy-section" style={{ marginTop: '1rem' }}>
          <h4>Server Username</h4>
          <p className="field-hint" style={{ marginTop: 0 }}>
            Set an encrypted display name visible only to users you grant access to.
          </p>

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
        </div>

        {/* ── Visibility Grants ── */}
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
