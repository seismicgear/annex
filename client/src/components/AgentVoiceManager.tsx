import { useEffect, useId, useState } from 'react';
import { getCredentialContext, isCredentialContextCurrent } from '@/api/core';
import { getPublicAgents } from '@/api/rtx';
import { assignAgentVoice, getAgentVoice, listVoiceProfiles, type AgentVoiceAssignment, type AvailableVoiceProfile } from '@/api/agent-voices';
import { useUsernameStore } from '@/stores/usernames';
import type { AgentInfo } from '@/types';

const message = (error: unknown) => error instanceof Error ? error.message : String(error);

function Assignment({ pseudonymId, agentId, profiles }: {
  pseudonymId: string; agentId: string; profiles: AvailableVoiceProfile[];
}) {
  const selectId = useId();
  const [assignment, setAssignment] = useState<AgentVoiceAssignment | null>(null);
  const [selected, setSelected] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const [saving, setSaving] = useState(false);
  const [reload, setReload] = useState(0);
  const [lifetime] = useState(() => ({ active: true }));

  useEffect(() => {
    lifetime.active = true;
    return () => { lifetime.active = false; };
  }, [lifetime]);

  useEffect(() => {
    let active = true;
    const context = getCredentialContext();
    getAgentVoice(pseudonymId, agentId).then(value => {
      if (!active || !isCredentialContextCurrent(context)) return;
      setAssignment(value); setSelected(value.voice_profile_id ?? 'default'); setError(null);
    }).catch(reason => { if (active && isCredentialContextCurrent(context)) setError(message(reason)); });
    return () => { active = false; };
  }, [agentId, pseudonymId, reload]);

  const save = async () => {
    const context = getCredentialContext();
    const current = () => lifetime.active && isCredentialContextCurrent(context);
    setSaving(true); setError(null); setSaved(false);
    try {
      await assignAgentVoice(pseudonymId, agentId, selected === 'default' ? null : selected);
      if (!current()) return;
      const next = await getAgentVoice(pseudonymId, agentId);
      if (!current()) return;
      setAssignment(next);
      if ((next.voice_profile_id ?? 'default') !== selected || !next.voice_backend) {
        throw new Error('The server did not confirm the selected voice. Refresh and try again.');
      }
      setSaved(true);
    } catch (reason) { if (current()) setError(message(reason)); }
    finally { if (current()) setSaving(false); }
  };

  const chosen = profiles.find(profile => profile.id === selected);
  return <div className="policy-section">
    {error && <p className="error-message" role="alert">{error}</p>}
    {!assignment ? <>
      <p role="status">{error ? 'Voice assignment unavailable.' : 'Loading voice assignment...'}</p>
      {error && <button type="button" onClick={() => setReload(value => value + 1)}>Retry voice assignment</button>}
    </> : <>
      <p>Current voice: {profiles.find(profile => profile.id === (assignment.voice_profile_id ?? 'default'))?.name
        ?? assignment.voice_profile_id ?? 'Default'} ({assignment.voice_backend ?? 'unavailable'})</p>
      <label htmlFor={selectId}>Voice profile</label>
      <select id={selectId} value={selected} disabled={saving} onChange={event => {
        setSelected(event.target.value); setSaved(false); setError(null);
      }}>
        {!chosen && <option value={selected} disabled>{selected || 'No voice configured'} — unavailable</option>}
        {profiles.map(profile => <option key={profile.id} value={profile.id} disabled={!profile.available}>
          {profile.name}{profile.model ? ` (${profile.model})` : ''}{profile.available ? '' : ' — unavailable'}
        </option>)}
      </select>
      {chosen?.available && <p className="field-hint">Speed: {chosen.speed}×. Pitch: {chosen.pitch}×.
        {chosen.speaker_id !== null ? ` Speaker: ${chosen.speaker_id}.` : ''} Applies to new speech requests.</p>}
      <button type="button" disabled={saving || !chosen?.available} onClick={() => { void save(); }}>
        {saving ? 'Saving voice...' : 'Save voice'}
      </button>
      {saved && <p className="success-message" role="status">Voice assignment saved.</p>}
    </>}
  </div>;
}

export function AgentVoiceManager({ pseudonymId }: { pseudonymId: string }) {
  const selectId = useId();
  const getDisplayName = useUsernameStore(state => state.getDisplayName);
  const [data, setData] = useState<{ agents: AgentInfo[]; profiles: AvailableVoiceProfile[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState('');
  const [loading, setLoading] = useState(true);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    let active = true;
    const context = getCredentialContext();
    Promise.all([getPublicAgents(), listVoiceProfiles(pseudonymId)]).then(([agents, voices]) => {
      if (!active || !isCredentialContextCurrent(context)) return;
      setData({ agents: agents.agents.filter(agent => agent.active), profiles: voices.profiles });
      setError(null);
    }).catch(reason => {
      if (active && isCredentialContextCurrent(context)) { setError(message(reason)); setData(null); }
    }).finally(() => { if (active && isCredentialContextCurrent(context)) setLoading(false); });
    return () => { active = false; };
  }, [pseudonymId, reload]);

  return <section className="policy-section" aria-label="Agent voices">
    <h3>Agent voices</h3>
    <p className="field-hint">Choose a configured voice for an active agent. Unavailable profiles cannot be assigned.</p>
    <button type="button" disabled={loading} onClick={() => {
      setLoading(true); setSelected(''); setReload(value => value + 1);
    }}>Refresh agent voices</button>
    {loading && <p role="status">Loading agent voices...</p>}
    {error && <p className="error-message" role="alert">Could not load agent voices: {error}</p>}
    {data && !loading && (data.agents.length ? <>
      <label htmlFor={selectId}>Agent</label>
      <select id={selectId} value={selected} onChange={event => setSelected(event.target.value)}>
        <option value="">Choose an agent</option>
        {data.agents.map(agent => <option key={agent.pseudonym_id} value={agent.pseudonym_id}>
          {getDisplayName(agent.pseudonym_id) ?? agent.pseudonym_id}
        </option>)}
      </select>
      {selected && <Assignment key={`${pseudonymId}:${selected}:${reload}`} pseudonymId={pseudonymId}
        agentId={selected} profiles={data.profiles} />}
    </> : <p>No active agents are registered on this server.</p>)}
  </section>;
}
