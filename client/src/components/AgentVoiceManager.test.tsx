// Component contract tests. Live provider/receiver tests qualify actual speech.
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AgentVoiceManager } from './AgentVoiceManager';
import type { AgentVoiceAssignment } from '@/api/agent-voices';

const calls = vi.hoisted(() => ({ list: vi.fn(), get: vi.fn(), assign: vi.fn(), agents: vi.fn() }));
vi.mock('@/api/agent-voices', () => ({ listVoiceProfiles: calls.list, getAgentVoice: calls.get, assignAgentVoice: calls.assign }));
vi.mock('@/api/rtx', () => ({ getPublicAgents: calls.agents }));

const assignment = (id = 'agent-a', voice: string | null = null): AgentVoiceAssignment => ({
  pseudonym_id: id, voice_profile_id: voice, voice_backend: voice === 'bark-live' ? 'bark' : 'piper',
});

async function open() {
  render(<AgentVoiceManager pseudonymId="operator" />);
  const user = userEvent.setup();
  await user.selectOptions(await screen.findByRole('combobox', { name: 'Agent' }), 'agent-a');
  await screen.findByRole('combobox', { name: 'Voice profile' });
  return user;
}

describe('operator agent voice selection', () => {
  beforeEach(() => {
    vi.resetAllMocks();
    calls.agents.mockResolvedValue({ agents: [{ pseudonym_id: 'agent-a', active: true }, { pseudonym_id: 'agent-b', active: true }, { pseudonym_id: 'inactive', active: false }] });
    calls.list.mockResolvedValue({ profiles: [
      { id: 'default', name: 'Default', model: 'piper', speed: 1, pitch: 1, speaker_id: null, available: true },
      { id: 'bark-live', name: 'Bark voice', model: 'bark', speed: 1.2, pitch: 0.9, speaker_id: 2, available: true },
      { id: 'missing', name: 'Missing voice', model: null, speed: null, pitch: null, speaker_id: null, available: false },
    ] });
    calls.get.mockImplementation(async (_operator, id) => assignment(id));
    calls.assign.mockResolvedValue({ status: 'updated' });
  });
  afterEach(cleanup);

  it('excludes inactive agents and prevents assigning an unavailable profile', async () => {
    await open();
    expect(screen.queryByRole('option', { name: 'inactive' })).not.toBeInTheDocument();
    expect(screen.getByRole('option', { name: /Missing voice/ })).toBeDisabled();
    expect(calls.assign).not.toHaveBeenCalled();
  });

  it('confirms assignment by readback and explicitly restores the default with null', async () => {
    const user = await open();
    await user.selectOptions(screen.getByRole('combobox', { name: 'Voice profile' }), 'bark-live');
    expect(screen.getByText(/Speed: 1.2×. Pitch: 0.9×./)).toHaveTextContent('Speaker: 2');
    calls.get.mockResolvedValueOnce(assignment('agent-a', 'bark-live'));
    await user.click(screen.getByRole('button', { name: 'Save voice' }));
    expect(await screen.findByText('Voice assignment saved.')).toBeVisible();
    expect(calls.assign).toHaveBeenCalledWith('operator', 'agent-a', 'bark-live');
    expect(screen.getByText('Current voice: Bark voice (bark)')).toBeVisible();
    await user.selectOptions(screen.getByRole('combobox', { name: 'Voice profile' }), 'default');
    await user.click(screen.getByRole('button', { name: 'Save voice' }));
    await waitFor(() => expect(calls.assign).toHaveBeenLastCalledWith('operator', 'agent-a', null));
    expect(await screen.findByText('Current voice: Default (piper)')).toBeVisible();
  });

  it('retains a rejected selection without claiming it was saved, and permits retry', async () => {
    const user = await open();
    await user.selectOptions(screen.getByRole('combobox', { name: 'Voice profile' }), 'bark-live');
    calls.assign.mockRejectedValueOnce(new Error('Operator permission revoked'));
    await user.click(screen.getByRole('button', { name: 'Save voice' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Operator permission revoked');
    expect(screen.queryByText('Voice assignment saved.')).not.toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Voice profile' })).toHaveValue('bark-live');
    expect(screen.getByText('Current voice: Default (piper)')).toBeVisible();
    calls.get.mockResolvedValueOnce(assignment('agent-a', 'bark-live'));
    await user.click(screen.getByRole('button', { name: 'Save voice' }));
    expect(await screen.findByText('Voice assignment saved.')).toBeVisible();
  });

  it('does not announce success when readback contradicts an accepted update', async () => {
    const user = await open();
    await user.selectOptions(screen.getByRole('combobox', { name: 'Voice profile' }), 'bark-live');
    await user.click(screen.getByRole('button', { name: 'Save voice' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('server did not confirm');
    expect(screen.queryByText('Voice assignment saved.')).not.toBeInTheDocument();
  });

  it('recovers a failed catalog request through the visible refresh control', async () => {
    calls.list.mockRejectedValueOnce(new Error('Connection refused'));
    render(<AgentVoiceManager pseudonymId="operator" />);
    expect(await screen.findByRole('alert')).toHaveTextContent('Connection refused');
    await userEvent.click(screen.getByRole('button', { name: 'Refresh agent voices' }));
    expect(await screen.findByRole('combobox', { name: 'Agent' })).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('does not apply a previous agent save after switching selection', async () => {
    const user = await open();
    let finish!: (value: { status: string }) => void;
    calls.assign.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    await user.selectOptions(screen.getByRole('combobox', { name: 'Voice profile' }), 'bark-live');
    await user.click(screen.getByRole('button', { name: 'Save voice' }));
    await user.selectOptions(screen.getByRole('combobox', { name: 'Agent' }), 'agent-b');
    await screen.findByRole('combobox', { name: 'Voice profile' });
    await act(async () => { finish({ status: 'updated' }); });
    expect(calls.get).toHaveBeenCalledTimes(2);
    expect(screen.queryByText('Voice assignment saved.')).not.toBeInTheDocument();
    expect(screen.getByText('Current voice: Default (piper)')).toBeVisible();
  });
});
