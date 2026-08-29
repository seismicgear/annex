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
import { InfoTip } from '@/components/InfoTip';
import * as personas from '@/lib/personas';
import * as api from '@/lib/api';
import type { Persona } from '@/types';
import type { MemberInfo } from '@/lib/api';
import { Modal } from '@/components/Modal';
import { useDialogTitleId } from '@/lib/use-dialog-title-id';

interface Props {
  onClose: () => void;
}

export function IdentitySettings({ onClose }: Props) {
  const titleId = useDialogTitleId();
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
  const currentUsername = useUsernameStore((s) => pseudonymId ? s.getDisplayName(pseudonymId) : null);
  const [username, setUsername] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Persona failures need their own slot: the shared `error` above renders
  // inside the username section further down the dialog, so a persona failure
  // reported through it would appear next to unrelated controls.
  const [personaError, setPersonaError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);

  // Pre-populate the username field from the visible usernames cache
  useEffect(() => {
    if (currentUsername && !username) {
      setUsername(currentUsername);
    }
  }, [currentUsername]); // eslint-disable-line react-hooks/exhaustive-deps

  // ── Grants state ──
  const [grantees, setGrantees] = useState<string[]>([]);
  const [members, setMembers] = useState<MemberInfo[]>([]);
  const [loadingGrants, setLoadingGrants] = useState(true);
  // Each list gets its own failure slot. They fail independently, and the
  // consequence of each is different: an unknown member list means the
  // roster is incomplete, while an unknown grant list means every row's
  // "Hidden"/"Granted" label is a guess — and a guess here is a privacy
  // claim, so the roster is withheld entirely rather than shown wrong.
  const [grantsError, setGrantsError] = useState<string | null>(null);
  const [membersError, setMembersError] = useState<string | null>(null);
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
    setLoadingGrants(true);
    setGrantsError(null);
    try {
      const resp = await api.listUsernameGrants(pseudonymId);
      setGrantees(resp.grantees);
    } catch (err) {
      setGrantees([]);
      setGrantsError(err instanceof Error ? err.message : 'the server did not answer');
    } finally {
      setLoadingGrants(false);
    }
  }, [pseudonymId]);

  const loadMembers = useCallback(async () => {
    if (!pseudonymId) return;
    setMembersError(null);
    try {
      const list = await api.listMembers(pseudonymId);
      setMembers(list.filter((m) => m.pseudonym_id !== pseudonymId));
    } catch (err) {
      setMembers([]);
      setMembersError(err instanceof Error ? err.message : 'the server did not answer');
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
    setPersonaError(null);
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
    } catch (err) {
      // Was `catch {}` with a comment reading "form stays open for retry".
      // The form did stay open — with no indication anything had gone wrong,
      // so retrying produced the same silent nothing. Personas are stored in
      // IndexedDB, which fails for real reasons a user can act on: private
      // browsing, a full quota, a blocked origin.
      setPersonaError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleEditPersona = async (e: FormEvent) => {
    e.preventDefault();
    if (!editing) return;
    setPersonaError(null);
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
    } catch (err) {
      setPersonaError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleDeletePersona = async (id: string) => {
    setPersonaError(null);
    try {
      await personas.deletePersona(id);
      await loadPersonas();
    } catch (err) {
      // "List remains unchanged" is indistinguishable from "nothing happened
      // because you did not really click it".
      setPersonaError(err instanceof Error ? err.message : String(err));
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

  const activeServer = useServersStore((s) =>
    s.servers.find((srv) => srv.id === s.activeServerId) ?? null,
  );

  if (!pseudonymId) return null;

  return (
    <Modal onClose={onClose} className="profile-switcher" titleId={titleId}>
      <h2 id={titleId}>Identity</h2>

      {/* Current pseudonym reference */}
      {identity && (
        <div className="current-identity-ref">
          <span className="label">Cryptographic ID:<InfoTip text="A unique anonymous identifier generated on your device. This is how the server knows you without ever learning your real name." /></span>
          <span className="pseudonym">{identity.pseudonymId ? `${identity.pseudonymId.slice(0, 16)}...` : 'pending'}</span>
          <span className="server-badge">{identity.serverSlug}</span>
        </div>
      )}

      {/* ── Persona section ── */}
      <div className="policy-section">
        <h3>Persona<InfoTip text="Your persona is just for you — it sets your display name and color on your device. Other users and the server never see it." /></h3>

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

        {personaError && (
          <div className="error-message" role="alert">
            {personaError}
          </div>
        )}
      </div>

      {/* ── Username section ── */}
      <div className="policy-section" style={{ marginTop: '1rem' }}>
        <h3>Server Username<InfoTip text="Unlike your persona, your username is stored (encrypted) on the server. Only people you explicitly grant access to can see it." /></h3>
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
        <h3>Username Visibility<InfoTip text="Control exactly who can see your username. Everyone else only sees your anonymous cryptographic ID." /></h3>
        <p className="field-hint" style={{ marginTop: 0 }}>
          Choose who can see your username. Others will only see your pseudonym.
        </p>

        {grantsError && (
          <div className="member-list-error" role="alert">
            <span>Could not load who can see your username: {grantsError}</span>
            <button onClick={loadGrants}>Retry</button>
          </div>
        )}
        {membersError && (
          <div className="member-list-error" role="alert">
            <span>Could not load the member list: {membersError}</span>
            <button onClick={loadMembers}>Retry</button>
          </div>
        )}

        {loadingGrants ? (
          <p>Loading...</p>
        ) : grantsError ? null : (
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
            {members.length === 0 && !membersError && (
              <p className="no-personas">No other members on this server yet.</p>
            )}
          </div>
        )}
      </div>

      <div className="dialog-actions" style={{ marginTop: '1rem' }}>
        <button onClick={onClose}>Close</button>
      </div>
    </Modal>
  );
}
