// Controlled component/request tests, not live backend or speech evidence.
// Keep real API helpers, credential scoping and takeover cancellation in use;
// only fetch responses are fixtures. The mount key matches MainLayout (server)
// and MemberManager (identity), so old responses must not update the new view.
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AgentVoiceManager } from './AgentVoiceManager';
import { getSessionToken, setApiBaseUrl, setSessionToken } from '@/api/core';
import { markSessionTakenOver, reclaimSession, TAKEOVER_MESSAGE } from '@/lib/session-ownership';

interface Context { server: string; operator: string; token: string; agent: string }
interface Call { url: URL; method: string; headers: Headers; signal?: AbortSignal | null; body?: BodyInit | null }
const first: Context = { server: 'https://voice-a.test', operator: 'operator-a', token: 'fixture-a', agent: 'agent-a' };
const serverSwitch: Context = { server: 'https://voice-b.test', operator: 'operator-b', token: 'fixture-b', agent: 'agent-b' };
const identitySwitch: Context = { ...serverSwitch, server: first.server };
const response = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
const assignment = (agent: string, profile: string | null = null) => ({
  pseudonym_id: agent, voice_profile_id: profile, voice_backend: profile ? 'bark' : 'piper',
});
const profiles = { profiles: [
  { id: 'default', name: 'Default', model: 'piper', speed: 1, pitch: 1, speaker_id: null, available: true },
  { id: 'bark-profile', name: 'Bark', model: 'bark', speed: 1, pitch: 1, speaker_id: null, available: true },
] };
function deferred() {
  let resolve!: (value: Response) => void;
  const promise = new Promise<Response>(done => { resolve = done; });
  return { promise, resolve };
}

let current: Context;
let calls: Call[];
let intercept: ((call: Call) => Promise<Response> | Response | undefined) | undefined;
function activate(context: Context) {
  current = context;
  setApiBaseUrl(context.server);
  setSessionToken(context.token, context.operator);
}
function manager(context: Context) {
  return <AgentVoiceManager key={`${context.server}:${context.operator}`} pseudonymId={context.operator} />;
}
async function selectAgent(context: Context) {
  await userEvent.selectOptions(await screen.findByRole('combobox', { name: 'Agent' }), context.agent);
  await screen.findByRole('combobox', { name: 'Voice profile' });
}
async function chooseAndSave() {
  await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Voice profile' }), 'bark-profile');
  await userEvent.click(screen.getByRole('button', { name: 'Save voice' }));
}
function assignmentReads(agent: string) { return calls.filter(call => call.method === 'GET' && call.url.pathname === `/api/agents/${agent}`); }
function expectScopedCalls(second: Context) {
  for (const call of calls) {
    const bearer = call.headers.get('authorization');
    if (!bearer) { expect(call.url.pathname).toBe('/api/public/agents'); continue; }
    if (call.url.origin === serverSwitch.server) expect(bearer).toBe(`Bearer ${serverSwitch.token}`);
    if (call.url.pathname === `/api/agents/${second.agent}`) expect(bearer).toBe(`Bearer ${second.token}`);
    if (call.url.pathname.startsWith(`/api/agents/${first.agent}`)) expect(bearer).toBe(`Bearer ${first.token}`);
  }
  expect(getSessionToken()).toBe(second.token);
}

describe('agent voice context and ownership boundaries', () => {
  beforeEach(() => {
    calls = []; intercept = undefined;
    for (const context of [first, serverSwitch, identitySwitch]) reclaimSession(context.server, context.operator);
    vi.stubGlobal('fetch', vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const call: Call = { url: new URL(String(input)), method: init?.method ?? 'GET', headers: new Headers(init?.headers), signal: init?.signal, body: init?.body };
      calls.push(call);
      const held = intercept?.(call); if (held) return held;
      if (call.url.pathname === '/api/uploads/grant') return response({ token: 'fixture-grant', expiresInSecs: 600 });
      if (call.url.pathname === '/api/public/agents') return response({ agents: [{
        pseudonym_id: current.agent, alignment_status: 'Aligned', transfer_scope: 'ReflectionSummariesOnly',
        reputation_score: 0, capability_contract: { offered_capabilities: ['text'] },
      }] });
      if (call.url.pathname === '/api/voice-profiles') return response(profiles);
      if (call.url.pathname === `/api/agents/${current.agent}`) return response(assignment(current.agent));
      throw new Error(`Unexpected fixture request ${call.method} ${call.url.pathname}`);
    }));
    activate(first);
  });
  afterEach(() => {
    cleanup(); setSessionToken(null, null); setApiBaseUrl('');
    for (const context of [first, serverSwitch, identitySwitch]) reclaimSession(context.server, context.operator);
    vi.unstubAllGlobals();
  });

  for (const [kind, second] of [['server', serverSwitch], ['identity', identitySwitch]] as const) {
    it(`discards an old catalog after a ${kind} switch without crossing credentials`, async () => {
      const gate = deferred();
      intercept = call => call.url.pathname === '/api/voice-profiles' && call.headers.get('authorization') === `Bearer ${first.token}` ? gate.promise : undefined;
      const view = render(manager(first));
      expect(await screen.findByText('Loading agent voices...')).toBeVisible();
      activate(second); view.rerender(manager(second)); await selectAgent(second);
      await act(async () => { gate.resolve(response({ error: 'Old server is unavailable' }, 503)); });
      expect(screen.getByRole('combobox', { name: 'Agent' })).toHaveValue(second.agent);
      expect(screen.queryByRole('option', { name: first.agent })).not.toBeInTheDocument();
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expect(screen.getByText('Current voice: Default (piper)')).toBeVisible();
      expectScopedCalls(second);
    });

    for (const stage of ['write', 'readback'] as const) {
      it(`discards a pending ${stage} after a ${kind} switch`, async () => {
        const gate = deferred(); let submitted = false;
        intercept = call => {
          if (call.method === 'PUT') { submitted = true; return stage === 'write' ? gate.promise : response({ status: 'updated' }); }
          if (stage === 'readback' && submitted && call.url.pathname === `/api/agents/${first.agent}`) return gate.promise;
        };
        const view = render(manager(first)); await selectAgent(first); await chooseAndSave();
        await waitFor(() => expect(assignmentReads(first.agent)).toHaveLength(stage === 'readback' ? 2 : 1));
        expect(screen.getByRole('button', { name: 'Saving voice...' })).toBeDisabled();
        activate(second); view.rerender(manager(second)); await selectAgent(second);
        await act(async () => { gate.resolve(response(stage === 'write' ? { status: 'updated' } : assignment(first.agent, 'bark-profile'))); });
        expect(assignmentReads(first.agent)).toHaveLength(stage === 'readback' ? 2 : 1);
        expect(assignmentReads(second.agent)).toHaveLength(1);
        expect(screen.getByText('Current voice: Default (piper)')).toBeVisible();
        expect(screen.getByRole('combobox', { name: 'Voice profile' })).toHaveValue('default');
        expect(screen.queryByText('Voice assignment saved.')).not.toBeInTheDocument();
        expect(screen.queryByRole('alert')).not.toBeInTheDocument();
        expectScopedCalls(second);
      });
    }
  }

  it('does not read back or announce success after an actual request helper receives revoked-session refusal', async () => {
    intercept = call => call.method === 'PUT' ? response({ error: 'Moderator session is no longer active' }, 401) : undefined;
    render(manager(first)); await selectAgent(first); await chooseAndSave();
    expect(await screen.findByRole('alert')).toHaveTextContent('Moderator session is no longer active');
    expect(assignmentReads(first.agent)).toHaveLength(1);
    expect(screen.queryByText('Voice assignment saved.')).not.toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'Voice profile' })).toHaveValue('bark-profile');
    expect(screen.getByText('Current voice: Default (piper)')).toBeVisible();
    expect(calls.filter(call => call.method === 'PUT')).toHaveLength(1);
  });

  it('aborts the pending write on takeover and rejects even a late fixture acknowledgement', async () => {
    const gate = deferred();
    intercept = call => call.method === 'PUT' ? gate.promise : undefined;
    render(manager(first)); await selectAgent(first); await chooseAndSave();
    const write = calls.find(call => call.method === 'PUT'); expect(write?.signal?.aborted).toBe(false);
    await act(async () => {
      markSessionTakenOver(first.server, first.operator);
      // Simulate a transport that cannot retract an already arriving response.
      // The real helper must still reject its aborted operation, not read back.
      gate.resolve(response({ status: 'updated' }));
    });
    expect(write?.signal?.aborted).toBe(true);
    expect(await screen.findByRole('alert')).toHaveTextContent(TAKEOVER_MESSAGE);
    expect(assignmentReads(first.agent)).toHaveLength(1);
    expect(screen.queryByText('Voice assignment saved.')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Save voice' }));
    expect(calls.filter(call => call.method === 'PUT')).toHaveLength(1);
  });
});
