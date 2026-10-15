// Local live agent acceptance runtime. It proves identity, negotiates trust,
// consumes real Annex frames, runs Ollama inference, and publishes text + speech.
// Only isolated test servers and explicitly addressed test prompts are accepted.
import assert from 'node:assert/strict';
import { writeFileSync } from 'node:fs';
import { createInterface } from 'node:readline';
import { register, authenticate, request, handshake, socket, createChannel, joinChannel, digest, until } from './protocol.mjs';

const args = process.argv.slice(2);
const arg = (name, fallback) => args.includes(name) ? args[args.indexOf(name) + 1] : fallback;
const url = arg('--url', 'http://127.0.0.1:3000');
const model = arg('--model', 'gpt-oss:20b');
const modelUrl = arg('--model-url', 'http://127.0.0.1:11434');
const evidencePath = arg('--evidence', undefined);
const requestedVoiceProfiles = arg('--voice-profiles', undefined)?.split(',');
if (requestedVoiceProfiles) assert(requestedVoiceProfiles.length === 2
  && requestedVoiceProfiles.every(id => typeof id === 'string' && /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/.test(id)),
  '--voice-profiles requires exactly two comma-separated operator profile IDs');
const receipts = [];
const agents = [];
let founder, policy, channelId;
let queue = Promise.resolve();
function emit(record) {
  const dated = { at: new Date().toISOString(), ...record };
  receipts.push(dated); console.log(JSON.stringify(dated));
  if (evidencePath) writeFileSync(evidencePath, JSON.stringify(receipts, null, 2));
}
async function connect(agent) {
  const report = await request(url, '/api/vrp/agent-handshake', {
    method: 'POST', token: agent.identity.token,
    body: { pseudonymId: agent.identity.pseudonym, handshake: handshake(policy) },
  });
  assert.equal(report.alignment_status, 'Aligned');
  if (requestedVoiceProfiles) {
    const profileId = requestedVoiceProfiles[agent.index];
    const assigned = await request(url, `/api/agents/${agent.identity.pseudonym}/voice-profile`, {
      method: 'PUT', token: founder.token, body: { voice_profile_id: profileId },
    });
    assert.equal(assigned.status, 'updated');
    const actual = await request(url, `/api/agents/${agent.identity.pseudonym}`, { token: founder.token });
    assert.equal(actual.voice_profile_id, profileId, 'GET must confirm the actual stored agent assignment');
    assert(['piper', 'bark', 'system'].includes(actual.voice_backend), 'The assigned voice profile must have an actual loaded backend');
    agent.voiceProfile = { id: actual.voice_profile_id, backend: actual.voice_backend };
    emit({ type: 'voice-profile', agent: agent.index, pseudonym: agent.identity.pseudonym, ...agent.voiceProfile });
  }
  await joinChannel(agent.identity, channelId);
  agent.connection = await socket(agent.identity);
  agent.connection.send({ type: 'subscribe', channelId });
  agent.connection.ws.addEventListener('message', event => {
    const frame = JSON.parse(String(event.data));
    if (frame.type === 'error') emit({ type: 'error', agent: agent.index, message: frame.message });
    if (frame.type === 'transcription' && frame.channelId === channelId) {
      emit({ type: 'transcription', agent: agent.index, channelId, speakerPseudonym: frame.speakerPseudonym, text: frame.text });
    }
    if (frame.type !== 'message' || frame.channelId !== channelId || !frame.content?.startsWith(agent.mention)) return;
    if (agents.some(item => item.identity.pseudonym === frame.senderPseudonym)) return;
    const prompt = frame.content.slice(agent.mention.length).trim();
    queue = queue.then(() => answer(agent, prompt, frame.messageId)).catch(error => {
      emit({ type: 'error', agent: agent.index, message: error.message });
    });
  });
}
async function answer(agent, prompt, messageId) {
  const start = Date.now();
  const response = await fetch(new URL('/api/chat', modelUrl), {
    method: 'POST', headers: { 'content-type': 'application/json' }, signal: AbortSignal.timeout(180_000),
    body: JSON.stringify({
      model, stream: false, think: 'low', keep_alive: '10m',
      messages: [
        { role: 'system', content: 'You are participating in an isolated Annex acceptance session. Answer the addressed participant in one brief sentence. Preserve any requested test marker exactly. Give numeric arithmetic results as digits. Do not use markdown.' },
        { role: 'user', content: prompt },
      ], options: { num_ctx: 4096, num_predict: 512 },
    }),
  });
  assert.equal(response.status, 200, 'Local inference must succeed');
  const inference = await response.json();
  const text = inference.message?.content?.trim();
  assert.equal(inference.done, true, 'Model response must complete');
  assert(text && inference.eval_count > 0, 'Model must generate a non-empty answer');
  const requestId = `agent-${agent.index}-${Date.now()}`;
  agent.connection.send({ type: 'message', channelId, content: text, replyTo: messageId, clientRequestId: requestId });
  const published = await agent.connection.wait(frame => frame.type === 'message' && frame.clientRequestId === requestId, 'acknowledged generated agent response');
  assert.equal(published.content, text);
  agent.connection.send({ type: 'voice_intent', channelId, text });
  emit({ type: 'inference', agent: agent.index, model: inference.model, promptMessageId: messageId,
    responseMessageId: published.messageId, response: text, response_sha256: digest(text),
    eval_count: inference.eval_count, model_duration_ns: inference.total_duration, elapsed_ms: Date.now() - start });
}
async function control(command) {
  if (command.action === 'grant-operator') {
    assert(typeof command.pseudonym === 'string' && /^[a-zA-Z0-9_-]{1,128}$/.test(command.pseudonym));
    const directory = await request(url, '/api/admin/members', { token: founder.token });
    const member = directory.members.find(item => item.pseudonym_id === command.pseudonym);
    assert(member?.active && member.participant_type === 'HUMAN', 'Only the separately registered live human may become the test operator');
    await request(url, `/api/admin/members/${command.pseudonym}/capabilities`, {
      method: 'PATCH', token: founder.token, body: {
        can_voice: member.can_voice, can_moderate: true, can_invite: member.can_invite,
        can_federate: member.can_federate, can_bridge: member.can_bridge,
      },
    });
    const current = await request(url, `/api/identity/${command.pseudonym}`);
    assert.equal(current.capabilities.can_moderate, true);
    emit({ type: 'control', action: command.action, pseudonym: command.pseudonym, ok: true });
    return;
  }
  assert(['disconnect', 'reconnect', 'revoke', 'revoke-voice', 'restore-voice'].includes(command.action));
  assert(Number.isInteger(command.agent) && command.agent >= 0 && command.agent < agents.length);
  const agent = agents[command.agent];
  if (command.action === 'disconnect') {
    agent.connection.ws.close();
    await until(() => agent.connection.ws.readyState === WebSocket.CLOSED, 'agent socket shutdown');
  } else if (command.action === 'reconnect') {
    if (agent.connection.ws.readyState !== WebSocket.CLOSED) {
      agent.connection.ws.close();
      await until(() => agent.connection.ws.readyState === WebSocket.CLOSED, 'prior agent socket shutdown');
    }
    await authenticate(agent.identity);
    await connect(agent);
  } else if (command.action === 'revoke-voice' || command.action === 'restore-voice') {
    const canVoice = command.action === 'restore-voice';
    await request(url, `/api/admin/members/${agent.identity.pseudonym}/capabilities`, {
      method: 'PATCH', token: founder.token,
      body: { can_voice: canVoice, can_moderate: false, can_invite: false, can_federate: false, can_bridge: false },
    });
    const current = await request(url, `/api/identity/${agent.identity.pseudonym}`);
    assert.equal(current.capabilities.can_voice, canVoice);
    // Do not close or replace this socket: the receiver must prove that the
    // server stops active media itself while retaining authorized text access.
  } else {
    await request(url, `/api/admin/members/${agent.identity.pseudonym}/revoke-sessions`, { method: 'POST', token: founder.token, body: {} });
    await request(url, '/api/channels', { token: agent.identity.token, expected: 401 });
    await until(() => agent.connection.ws.readyState === WebSocket.CLOSED, 'revoked agent socket shutdown');
  }
  emit({ type: 'control', action: command.action, agent: command.agent, ok: true });
}
async function main() {
  await request(url, '/health');
  const tags = await fetch(new URL('/api/tags', modelUrl), { signal: AbortSignal.timeout(10_000) }).then(response => response.json());
  assert(tags.models?.some(item => item.name === model), `The requested real model ${model} must already be installed`);
  founder = await register(url);
  policy = await request(url, '/api/admin/policy', { token: founder.token });
  channelId = `agent-live-${Date.now()}`;
  await createChannel(founder, channelId, 'Hybrid', 'Local');
  for (let index = 0; index < 2; index++) {
    const identity = await register(url, 2);
    // Operator grants an actual capability through the shipped moderator API.
    await request(url, `/api/admin/members/${identity.pseudonym}/capabilities`, {
      method: 'PATCH', token: founder.token,
      body: { can_voice: true, can_moderate: false, can_invite: false, can_federate: false, can_bridge: false },
    });
    const agent = { identity, index, mention: `@annex-agent-${index + 1}`, connection: null };
    agents.push(agent);
    await connect(agent);
  }
  emit({ type: 'ready', channelId, channelName: channelId, agentPseudonym: agents[0].identity.pseudonym,
    agents: agents.map(agent => ({ pseudonym: agent.identity.pseudonym, mention: agent.mention,
      ...(agent.voiceProfile ? { voiceProfile: agent.voiceProfile } : {}) })) });
  const input = createInterface({ input: process.stdin });
  input.on('line', line => {
    queue = queue.then(() => control(JSON.parse(line))).catch(error => emit({ type: 'error', message: error.message }));
  });
}
function stop() { for (const agent of agents) agent.connection?.ws.close(); process.exit(0); }
process.on('SIGTERM', stop); process.on('SIGINT', stop);
try { await main(); } catch (error) { emit({ type: 'fatal', message: error.message }); process.exit(1); }
