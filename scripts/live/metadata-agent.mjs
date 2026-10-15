// Real role-2 participant for profile-sharing acceptance. Communicates only
// through its parent's private pipes. Never writes credentials or keys to disk.
import { createInterface } from 'node:readline';
import { authenticate, handshake, hex, register, request } from './protocol.mjs';

let identity, policy, serverSlug;
async function update(marker) {
  const payload = handshake(policy);
  payload.capability_contract = {
    ...payload.capability_contract, version: 2,
    offered_capabilities: [...new Set([...policy.agent_required_capabilities, marker])],
    knowledge_domains_allowed: policy.knowledge_domains_allowed ?? null,
    retention_policy: policy.rtx_retention_seconds == null ? { kind: 'unlimited' } : { kind: 'max_age', seconds: policy.rtx_retention_seconds },
    max_exchange_size: policy.max_exchange_size ?? 1_048_576,
  };
  await request(identity.url, '/api/vrp/agent-handshake', { method: 'POST', token: identity.token,
    body: { pseudonymId: identity.pseudonym, handshake: payload } });
  // A real membership verification refreshes the graph node from the actual
  // registered capability contract. There is no metadata editor or DB injection.
  await authenticate(identity);
  const snapshot = await request(identity.url, '/api/graph/metadata-grants', { token: identity.token });
  if (snapshot.owner_pseudonym !== identity.pseudonym || typeof snapshot.metadata_json !== 'string') throw new Error('Metadata not populated');
  const metadata = JSON.parse(snapshot.metadata_json);
  if (!metadata.capability_contract?.offered_capabilities?.includes(marker)) throw new Error('Actual metadata update not observed');
  return { pseudonym: identity.pseudonym, metadata };
}

const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of lines) {
  let command;
  try {
    command = JSON.parse(line);
    if (command.command === 'close') process.exit(0);
    if (command.command === 'create') {
      if (identity) throw new Error('Already created');
      policy = command.policy; serverSlug = command.serverSlug;
      if (!serverSlug || !command.url || !command.marker) throw new Error('Missing input');
      identity = await register(command.url, 2, `annex:server:${serverSlug}:v2`);
      const actual = await update(command.marker);
      const path = await request(identity.url, `/api/registry/path/${identity.commitment}`);
      const backup = JSON.stringify({
        sk: hex(identity.sk), roleCode: 2, nodeId: identity.nodeId,
        commitmentHex: identity.commitment, pseudonymId: identity.pseudonym,
        sessionToken: null, serverSlug, leafIndex: path.leafIndex,
      });
      process.stdout.write(JSON.stringify({ ok: true, ...actual, backup }) + '\n');
    } else if (command.command === 'update' && identity && command.marker) {
      process.stdout.write(JSON.stringify({ ok: true, ...await update(command.marker) }) + '\n');
    } else throw new Error('Unknown command');
  } catch {
    // Protocol error bodies can include application state; keep this pipe's
    // failure result bounded and never echo private input or a proof.
    process.stdout.write(JSON.stringify({ ok: false, error: `Genuine agent ${command?.command === 'update' ? 'update' : 'registration'} failed` }) + '\n');
  }
}
process.exit(0);
