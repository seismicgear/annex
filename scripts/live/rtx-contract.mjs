// Real contract admission and lifecycle acceptance. No route stubs, SQL writes,
// fake clocks, injected failures or pre-created failed outbox rows.
import assert from 'node:assert/strict';
import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { authorSigningPayload } from './rtx.mjs';
import { register, request, handshake, socket, signingIdentityFromSeed, until } from './protocol.mjs';

export function contractV2(policy, overrides = {}) {
  const contract = {
    version: 2, required_capabilities: [], offered_capabilities: policy.agent_required_capabilities,
    redacted_topics: [], ...(policy.knowledge_domains_allowed == null ? {} : { knowledge_domains_allowed: policy.knowledge_domains_allowed }),
    retention_policy: policy.rtx_retention_seconds == null ? { kind: 'unlimited' }
      : { kind: 'max_age', seconds: policy.rtx_retention_seconds },
    max_exchange_size: policy.max_exchange_size ?? 1048576, ...overrides,
  };
  // Match the production optional-field serialization before signing. Null
  // means unrestricted/absent; an explicit empty array remains deny-all.
  if (contract.knowledge_domains_allowed == null) delete contract.knowledge_domains_allowed;
  return contract;
}

/** Parent provides genuine process and federation controls used by its existing
 * HTTP/DC session. Read callbacks inspect actual DB state but never mutate it. */
export async function runRtxContractAcceptance({ hostFounder, vmFounder, record, readStoredBundle,
  readStoredOutboxEnvelope, renegotiatePeers, restartPeer, setPeerOnline }) {
  for (const fn of [record, readStoredBundle, readStoredOutboxEnvelope, renegotiatePeers, restartPeer, setPeerOnline]) assert.equal(typeof fn, 'function');
  const founders = [hostFounder, vmFounder];
  const original = await Promise.all(founders.map(f => request(f.url, '/api/admin/policy', { token: f.token })));
  const policies = original.map(policy => ({ ...policy }));
  const tag = `contract-${randomUUID().slice(0, 12)}`;
  const actors = [];
  const emit = (action, details = {}) => record(`RTX contract: ${action}`, details);
  let peerOffline = false;
  const align = async (actor, override = {}) => {
    const body = handshake(policies[actor.node]);
    body.capability_contract = contractV2(policies[actor.node], override);
    const result = await request(actor.identity.url, '/api/vrp/agent-handshake', {
      method: 'POST', token: actor.identity.token,
      body: { pseudonymId: actor.identity.pseudonym, handshake: body, signingPubkey: actor.key.publicKey },
    });
    assert.equal(result.alignment_status, 'Aligned');
    assert.match(result.handshakeRef, /^vrp:\d+:\d+$/);
    actor.reference = result.handshakeRef;
    return result;
  };
  const change = async patch => {
    for (let i = 0; i < founders.length; i++) {
      await request(founders[i].url, '/api/admin/policy', { method: 'PUT', token: founders[i].token, body: { ...policies[i], ...patch } });
      policies[i] = await request(founders[i].url, '/api/admin/policy', { token: founders[i].token });
      for (const [key, value] of Object.entries(patch)) assert.deepEqual(policies[i][key], value);
    }
    await renegotiatePeers();
    for (const actor of actors) await align(actor);
  };
  const signed = (actor, overrides = {}) => {
    const bundle = { bundle_id: randomUUID(), source_pseudonym: actor.identity.pseudonym, source_server: actor.identity.url,
      domain_tags: [tag], summary: `Controlled RTX contract acceptance ${tag}.`, reasoning_chain: null, caveats: [],
      created_at: Date.now(), signature: '', vrp_handshake_ref: actor.reference, ...overrides };
    bundle.signature = actor.key.sign(createHash('sha256').update(authorSigningPayload(bundle)).digest());
    return bundle;
  };
  const publish = (actor, bundle, expected = 200) => request(actor.identity.url, '/api/rtx/publish', {
    method: 'POST', token: actor.identity.token, body: bundle, expected,
  });
  const received = async (actor, bundle) => {
    const frame = await actor.connection.wait(f => f.type === 'rtx_bundle' && f.bundle?.bundle_id === bundle.bundle_id, 'actual restricted RTX delivery');
    assert.deepEqual(frame.bundle, bundle);
    assert.deepEqual(frame.contract.knowledge_domains_allowed, [tag]);
    assert.equal(frame.contract.max_exchange_size, 4096);
    assert.equal(frame.expiresAtMs, bundle.created_at + frame.contract.retention_seconds * 1000);
    return frame;
  };
  const outbox = async id => (await request(hostFounder.url, '/api/admin/federation/outbox?limit=500', { token: hostFounder.token }))
    .entries.find(row => row.message_id === `rtx:${id}` && row.peer_base_url === vmFounder.url);
  const restore = async () => {
    for (let i = 0; i < founders.length; i++) {
      await request(founders[i].url, '/api/admin/policy', { method: 'PUT', token: founders[i].token, body: original[i] });
      policies[i] = original[i];
    }
    await renegotiatePeers();
  };
  try {
    for (let node = 0; node < 2; node++) for (let count = 0; count < 2; count++) {
      const actor = { node, identity: await register(founders[node].url, 2), key: signingIdentityFromSeed(randomBytes(32).toString('hex')) };
      await align(actor); actor.connection = await socket(actor.identity); actors.push(actor);
      await request(actor.identity.url, '/api/rtx/subscribe', { method: 'POST', token: actor.identity.token,
        body: { domain_filters: [tag], accept_federated: true } });
    }
    await change({ knowledge_domains_allowed: [tag], max_exchange_size: 4096, rtx_retention_seconds: 120 });
    const [hostAuthor, hostObserver, vmAuthor, vmObserver] = actors;
    for (const [author, local, remote] of [[hostAuthor, hostObserver, vmObserver], [vmAuthor, vmObserver, hostObserver]]) {
      const bundle = signed(author);
      await publish(author, bundle); await received(local, bundle); const frame = await received(remote, bundle);
      assert.equal(frame.provenance.origin.version, 3);
      assert.deepEqual(frame.provenance.origin.contract, frame.contract);
      for (const founder of founders) {
        const row = await until(() => readStoredBundle(founder.url, bundle.bundle_id), 'actual restricted RTX persisted');
        assert.equal(Number(row.expires_at_ms), bundle.created_at + 120_000);
      }
    }
    emit('Persisted domain, byte and lifetime policy negotiates through real agents and both federation directions');
    for (const overrides of [
      { domain_tags: [tag, 'outside-allowlist'] }, { domain_tags: [] }, { summary: 'x'.repeat(4096) },
      { created_at: Date.now() - 121_000 }, { created_at: Date.now() + 100_000 },
    ]) {
      const bundle = signed(hostAuthor, overrides); await publish(hostAuthor, bundle, 403);
      for (const founder of founders) assert.equal(await readStoredBundle(founder.url, bundle.bundle_id), null);
    }
    emit('Mixed domains, untagged content, oversize bytes, expired timestamp and future retention extension fail before storage');

    const restricted = await align(hostObserver, { knowledge_domains_allowed: [] });
    assert.deepEqual(restricted.negotiated_rtx.knowledge_domains_allowed, []);
    const denied = signed(vmAuthor); await publish(vmAuthor, denied); await received(hostAuthor, denied);
    await until(() => readStoredBundle(hostFounder.url, denied.bundle_id), 'independent host receipt before negative subscriber assertion');
    await new Promise(resolve => setTimeout(resolve, 1000));
    assert.equal(hostObserver.connection.ws.readyState, WebSocket.OPEN);
    assert(!hostObserver.connection.frames.some(f => f.type === 'rtx_bundle' && f.bundle?.bundle_id === denied.bundle_id));
    await align(hostObserver);
    const barrier = signed(vmAuthor); await publish(vmAuthor, barrier); await received(hostObserver, barrier);
    assert(!hostObserver.connection.frames.some(f => f.type === 'rtx_bundle' && f.bundle?.bundle_id === denied.bundle_id));
    emit('An explicitly empty agent allowlist blocks delivery while its real socket remains live; later positive marker proves continued transport');

    await change({ rtx_retention_seconds: 10 });
    await setPeerOnline(vmFounder.url, false); peerOffline = true;
    const expiring = signed(hostAuthor); await publish(hostAuthor, expiring);
    const row = await until(() => outbox(expiring.bundle_id), 'durable actual outage outbox intent');
    const envelope = await readStoredOutboxEnvelope(hostFounder.url, row.id);
    const wire = typeof envelope === 'string' ? JSON.parse(envelope) : envelope;
    assert.equal(wire.provenance.origin.version, 3);
    assert.equal(wire.provenance.origin.contract.retention_seconds, 10);
    await restartPeer(hostFounder.url);
    await until(async () => (await readStoredBundle(hostFounder.url, expiring.bundle_id)) === null, 'real elapsed retention expiry after publisher restart', 45_000);
    assert.equal(await outbox(expiring.bundle_id), undefined, 'Expired durable ciphertext must be removed, even while peer is unavailable');
    const governance = await request(hostFounder.url, `/api/rtx/governance/transfers?bundle_id=${expiring.bundle_id}`, { token: hostFounder.token });
    assert.equal(governance.total, 0);
    await setPeerOnline(vmFounder.url, true); peerOffline = false;
    await request(vmFounder.url, '/api/federation/rtx', { method: 'POST', body: wire, expected: 403 });
    assert.equal(await readStoredBundle(vmFounder.url, expiring.bundle_id), null);
    await restore(); await align(hostAuthor);
    await publish(hostAuthor, signed(hostAuthor, { bundle_id: expiring.bundle_id }), 403);
    assert.equal(await readStoredBundle(hostFounder.url, expiring.bundle_id), null);
    emit('Actual peer outage and publisher restart preserve expiry: body, governance and queued ciphertext erased; expired signed relay and fresh re-signed same-ID resurrection both refused', { bundleId: expiring.bundle_id, outboxId: row.id });
    return { complete: true, restrictions: ['knowledge_domains_allowed', 'max_exchange_size', 'retention_policy'], realRestartAndElapsedExpiry: true };
  } finally {
    if (peerOffline) await setPeerOnline(vmFounder.url, true);
    await restore();
    for (const actor of actors) {
      actor.connection?.ws.close();
      await request(actor.identity.url, '/api/rtx/subscribe', { method: 'DELETE', token: actor.identity.token });
    }
  }
}
