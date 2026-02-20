/**
 * Profile switcher — manage persona identities across different server contexts.
 *
 * Users define local personas (display name, avatar, bio) that are automatically
 * mapped to the correct derived pseudonym when crossing server boundaries.
 * The cryptographic identity mapping is invisible to the user.
 */

import { useState, useEffect, useCallback, type FormEvent } from 'react';
import { useIdentityStore } from '@/stores/identity';
import type { Persona } from '@/types';
import * as personas from '@/lib/personas';

interface Props {
  onClose: () => void;
}

export function ProfileSwitcher({ onClose }: Props) {
  const identity = useIdentityStore((s) => s.identity);

  const [personaList, setPersonaList] = useState<Persona[]>([]);
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<Persona | null>(null);

  // Form state
  const [displayName, setDisplayName] = useState('');
  const [bio, setBio] = useState('');
  const [accentColor, setAccentColor] = useState(personas.randomAccentColor());

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

  const resetForm = () => {
    setDisplayName('');
    setBio('');
    setAccentColor(personas.randomAccentColor());
    setCreating(false);
    setEditing(null);
  };

  const handleCreate = async (e: FormEvent) => {
    e.preventDefault();
    if (!identity || !displayName.trim()) return;

    await personas.createPersona(
      displayName.trim(),
      identity.id,
      identity.serverSlug,
      bio.trim(),
    );
    resetForm();
    await loadPersonas();
  };

  const handleEdit = async (e: FormEvent) => {
    e.preventDefault();
    if (!editing) return;

    await personas.updatePersona({
      ...editing,
      displayName: displayName.trim() || editing.displayName,
      bio: bio.trim(),
      accentColor,
    });
    resetForm();
    await loadPersonas();
  };

  const handleDelete = async (id: string) => {
    await personas.deletePersona(id);
    await loadPersonas();
  };

  const startEdit = (persona: Persona) => {
    setEditing(persona);
    setDisplayName(persona.displayName);
    setBio(persona.bio);
    setAccentColor(persona.accentColor);
  };

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog profile-switcher" onClick={(e) => e.stopPropagation()}>
        <h3>Persona Profiles</h3>
        <p className="profile-description">
          Define how you appear across different server contexts. Your cryptographic
          identity stays private — only the display name and avatar are visible.
        </p>

        {/* Current pseudonym reference */}
        {identity && (
          <div className="current-identity-ref">
            <span className="label">Cryptographic ID:</span>
            <span className="pseudonym">{identity.pseudonymId?.slice(0, 16)}...</span>
            <span className="server-badge">{identity.serverSlug}</span>
          </div>
        )}

        {/* Persona list */}
        <div className="persona-list">
          {personaList.length === 0 && !creating && (
            <p className="no-personas">
              No personas defined. Create one to customize your display name.
            </p>
          )}
          {personaList.map((p) => (
            <div key={p.id} className="persona-item">
              <div
                className="persona-avatar"
                style={{ background: p.accentColor }}
              >
                {p.displayName.charAt(0).toUpperCase()}
              </div>
              <div className="persona-info">
                <span className="persona-name">{p.displayName}</span>
                <span className="persona-meta">
                  {p.serverSlug} {p.bio && `— ${p.bio}`}
                </span>
              </div>
              <div className="persona-actions">
                <button
                  className="persona-edit-btn"
                  onClick={() => startEdit(p)}
                  title="Edit"
                >
                  Edit
                </button>
                <button
                  className="persona-delete-btn"
                  onClick={() => handleDelete(p.id)}
                  title="Delete"
                >
                  Del
                </button>
              </div>
            </div>
          ))}
        </div>

        {/* Create / Edit form */}
        {(creating || editing) && (
          <form
            className="persona-form"
            onSubmit={editing ? handleEdit : handleCreate}
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
                {[
                  '#646cff', '#4ade80', '#f87171', '#fbbf24', '#7eb8da',
                  '#b87eda', '#ff6b9d', '#10b981', '#6366f1', '#ec4899',
                ].map((color) => (
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

        {/* Bottom actions */}
        {!creating && !editing && (
          <div className="dialog-actions">
            <button onClick={() => setCreating(true)} className="primary-btn">
              New Persona
            </button>
            <button onClick={onClose}>Close</button>
          </div>
        )}
      </div>
    </div>
  );
}
