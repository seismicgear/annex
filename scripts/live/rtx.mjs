// Actual local and cross-server RTX acceptance. Imported by federation.mjs;
// this file does not start servers, seed databases, or substitute responses.
import assert from 'node:assert/strict';
import { createHash, createPublicKey, randomBytes, randomUUID, verify } from 'node:crypto';
import { register, request, handshake, socket, signingIdentityFromSeed, until } from './protocol.mjs';

const sha256 = value => createHash('sha256').update(value).digest();
const field = value => `${Buffer.byteLength(value, 'utf8')}:${value}\n`;

/** Independent encoding of annex-rtx/src/validation.rs:author_signing_payload.
 * Lengths are UTF-8 bytes, vectors preserve order, null and empty chain differ. */
export function authorSigningPayload(bundle) {
  let out = 'annex/rtx/author-sig/v1\n';
  out += field(bundle.bundle_id) + field(bundle.source_pseudonym) + field(bundle.source_server);
  out += `${bundle.created_at}\n` + field(bundle.vrp_handshake_ref) + field(bundle.summary);
  out += bundle.reasoning_chain === null ? '0\n' : `1\n${field(bundle.reasoning_chain)}`;
  out += `tags:${bundle.domain_tags.length}\n` + bundle.domain_tags.map(field).join('');
  out += `caveats:${bundle.caveats.length}\n` + bundle.caveats.map(field).join('');
  return out;
}
function authorSignatureValid(bundle, key) {
  const publicKey = createPublicKey({
    key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), Buffer.from(key.publicKey, 'hex')]),
    format: 'der', type: 'spki',
  });
  return verify(null, sha256(authorSigningPayload(bundle)), publicKey, Buffer.from(bundle.signature, 'hex'));
}
function normalizedStoredBundle(row) {
  return {
    bundle_id: row.bundle_id, source_pseudonym: row.source_pseudonym, source_server: row.source_server,
    domain_tags: row.domain_tags ?? JSON.parse(row.domain_tags_json), summary: row.summary,
    reasoning_chain: row.reasoning_chain ?? null, caveats: row.caveats ?? JSON.parse(row.caveats_json),
    created_at: Number(row.created_at_ms ?? row.created_at), signature: row.signature,
    vrp_handshake_ref: row.vrp_handshake_ref,
  };
}
const bundleFrame = (actor, id) => actor.connection.frames.find(frame => frame.type === 'rtx_bundle' && frame.bundle?.bundle_id === id);

/** Call only after the two actual peers have established their real agreements.
 * readStoredBundle(url,id), when supplied, returns a single actual read-only
 * rtx_bundles row (or null). No database mutation is performed by this module.
 * record receives only public IDs, hashes and outcomes, never auth/key bytes. */
export async function runRtxAcceptance({ hostFounder, vmFounder, policyHost, policyVm, record, readStoredBundle,
  setPeerOnline, restartPeer, readStoredOutboxEnvelope }) {
  assert.equal(typeof record, 'function');
  const tag = `rtx-${randomUUID().slice(0, 12)}`;
  const nodes = [
    { founder: hostFounder, policy: policyHost, label: 'Windows' },
    { founder: vmFounder, policy: policyVm, label: 'Ubuntu' },
  ];
  const actors = [];
  const published = [];
  const expectedStored = new Map();
  let durableRecovery = false;
  let authenticatedRetry = false;
  const forbidden = new Map();
  const emit = (action, details = {}) => record(`RTX: ${action}`, details);
  const transfers = (node, query = {}) => request(node.founder.url,
    `/api/rtx/governance/transfers?${new URLSearchParams({ limit: '500', ...query })}`, { token: node.founder.token });
  const subscribe = async (actor, filters = [tag], acceptFederated = true) => {
    const response = await request(actor.identity.url, '/api/rtx/subscribe', {
      method: 'POST', token: actor.identity.token, body: { domain_filters: filters, accept_federated: acceptFederated },
    });
    assert.equal(response.ok, true);
    assert.equal(response.subscription.subscriber_pseudonym, actor.identity.pseudonym);
    const stored = await request(actor.identity.url, '/api/rtx/subscriptions', { token: actor.identity.token });
    assert.deepEqual(stored.subscription.domain_filters, filters);
    assert.equal(stored.subscription.accept_federated, acceptFederated);
    actor.subscribed = true;
  };
  const align = async (actor, redactedTopics = [], includeKey = true) => {
    const payload = handshake(actor.node.policy);
    payload.capability_contract.redacted_topics = redactedTopics;
    const body = { pseudonymId: actor.identity.pseudonym, handshake: payload,
      ...(includeKey ? { signingPubkey: actor.key.publicKey } : {}) };
    const report = await request(actor.identity.url, '/api/vrp/agent-handshake', {
      method: 'POST', token: actor.identity.token, body,
    });
    assert.equal(report.alignment_status, 'Aligned');
    assert.equal(report.transfer_scope, actor.node.policy.allow_full_knowledge ? 'FullKnowledgeBundle' : 'ReflectionSummariesOnly');
    assert.match(report.handshakeRef, /^vrp:\d+:\d+$/);
    actor.handshakeRef = report.handshakeRef;
    const profile = await request(actor.identity.url, `/api/agents/${actor.identity.pseudonym}`, { token: actor.identity.token });
    assert.equal(profile.alignment_status, report.alignment_status);
    assert.equal(profile.transfer_scope, report.transfer_scope);
    assert.deepEqual(profile.capability_contract.redacted_topics, redactedTopics);
    assert.equal(profile.handshakeRef, report.handshakeRef, 'Profile must expose the actual accepted decision returned by the handshake');
    return report;
  };
  const make = (actor, label, overrides = {}) => {
    const bundle = {
      bundle_id: randomUUID(), source_pseudonym: actor.identity.pseudonym, source_server: actor.identity.url,
      domain_tags: [tag, 'systems'], summary: `${tag}: ${label}. UTF-8 boundary π; newline\nverified.`,
      reasoning_chain: null, caveats: ['Controlled acceptance content: no private knowledge.'],
      created_at: Date.now(), signature: '', vrp_handshake_ref: actor.handshakeRef, ...overrides,
    };
    bundle.signature = actor.key.sign(sha256(authorSigningPayload(bundle)));
    assert.equal(authorSignatureValid(bundle, actor.key), true, 'Producing agent must sign its exact actual bytes');
    return bundle;
  };
  const publish = async (actor, bundle) => {
    const response = await request(actor.identity.url, '/api/rtx/publish', { method: 'POST', token: actor.identity.token, body: bundle });
    assert.equal(response.ok, true);
    assert.equal(response.bundleId, bundle.bundle_id);
    published.push(bundle);
    expectedStored.set(bundle.bundle_id, actor.node.policy.allow_full_knowledge ? bundle : { ...bundle, reasoning_chain: null });
    return response;
  };
  const received = async (actor, expected, source, verifyAuthor = true) => {
    const frame = await actor.connection.wait(candidate => candidate.type === 'rtx_bundle' && candidate.bundle?.bundle_id === expected.bundle_id,
      `${actor.node.label} real RTX WebSocket ${expected.bundle_id}`);
    assert.deepEqual(frame.bundle, expected, 'Delivered bundle fields must equal the expected policy-scoped bytes');
    if (verifyAuthor) assert.equal(authorSignatureValid(frame.bundle, source.key), true, 'Receiver independently verifies actual author signature');
    if (actor.identity.url !== source.identity.url) {
      assert.equal(frame.federated, true);
      assert.equal(frame.provenance.origin_server, source.identity.url);
      assert.equal(frame.provenance.bundle_id, expected.bundle_id);
      assert.equal(frame.provenance.hops.length, 1, 'This bounded lane has two nodes and one real relay hop');
      assert.equal(frame.provenance.hops[0].server, source.identity.url);
      assert.match(frame.provenance.hops[0].signature, /^[0-9a-f]{128}$/i);
    } else assert.notEqual(frame.federated, true);
    return frame;
  };
  const stored = async (node, expected) => {
    const audit = await until(async () => {
      const result = await transfers(node, { bundle_id: expected.bundle_id });
      return result.total > 0 ? result : undefined;
    }, `persisted ${node.label} RTX governance entry`);
    assert(audit.transfers.some(row => row.source_pseudonym === expected.source_pseudonym && row.destination_pseudonym === null));
    if (readStoredBundle) {
      const row = await until(() => readStoredBundle(node.founder.url, expected.bundle_id), `stored ${node.label} RTX bytes`);
      assert.deepEqual(normalizedStoredBundle(row), expected, 'Independent stored bundle must preserve policy-scoped fields and author signature');
      if (expected.source_server !== node.founder.url) {
        const provenance = typeof row.provenance_json === 'string' ? JSON.parse(row.provenance_json) : row.provenance;
        assert.equal(provenance.origin_server, expected.source_server);
        assert.equal(provenance.bundle_id, expected.bundle_id);
      }
    }
    return audit;
  };
  const assertBlocked = async (blocked, bundle, positiveReceiver) => {
    assert.equal(blocked.connection.ws.readyState, WebSocket.OPEN, 'Negative delivery proof requires a live restricted subscriber');
    await positiveReceiver.connection.wait(frame => frame.type === 'rtx_bundle' && frame.bundle?.bundle_id === bundle.bundle_id,
      'independent positive-control RTX delivery');
    await stored(blocked.node, { ...bundle, reasoning_chain: null });
    // A real delivery to another subscriber proves the relay was processed.
    // Observe a bounded quiet interval, then recheck after later positive
    // markers on the SAME socket before this function returns its final result.
    await new Promise(resolve => setTimeout(resolve, 1_500));
    assert.equal(blocked.connection.ws.readyState, WebSocket.OPEN, 'Restricted subscriber must stay connected throughout the observation');
    assert.equal(bundleFrame(blocked, bundle.bundle_id), undefined, 'Filtered RTX bundle must not reach the restricted subscriber');
    const audit = await transfers(blocked.node, { bundle_id: bundle.bundle_id, destination: blocked.identity.pseudonym });
    assert.equal(audit.total, 0, 'Restricted subscriber must have no persisted delivery record');
    if (!forbidden.has(blocked)) forbidden.set(blocked, new Set());
    forbidden.get(blocked).add(bundle.bundle_id);
  };
  const assertRejected = async (actor, bundle, status) => {
    await request(actor.identity.url, '/api/rtx/publish', { method: 'POST', token: actor.identity.token, body: bundle, expected: status });
    for (const node of nodes) {
      assert.equal((await transfers(node, { bundle_id: bundle.bundle_id })).total, 0, 'Refused bundle must create no transfer rows');
      if (readStoredBundle) assert.equal(await readStoredBundle(node.founder.url, bundle.bundle_id), null, 'Refused bundle must not be persisted');
    }
    for (const subscriber of actors) assert.equal(bundleFrame(subscriber, bundle.bundle_id), undefined);
  };

  try {
    for (const node of nodes) {
      node.baseline = await request(node.founder.url, '/api/rtx/governance/summary', { token: node.founder.token });
      node.agents = [];
      for (let index = 0; index < 2; index += 1) {
        const identity = await register(node.founder.url, 2, `${tag}:${node.label}:${index}`);
        const actor = { identity, key: signingIdentityFromSeed(randomBytes(32).toString('hex')), node, subscribed: false };
        actors.push(actor); node.agents.push(actor);
        await align(actor);
        const before = await request(identity.url, '/api/rtx/subscriptions', { token: identity.token });
        assert.equal(before.ok, true);
        assert.equal(before.subscription, undefined);
        actor.connection = await socket(identity);
        await subscribe(actor);
        await request(identity.url, '/api/rtx/governance/transfers', { token: identity.token, expected: 403 });
        await request(identity.url, '/api/rtx/governance/summary', { token: identity.token, expected: 403 });
      }
    }
    const [host, vm] = nodes;
    const [hostAuthor, hostObserver] = host.agents;
    const [vmAuthor, vmObserver] = vm.agents;
    emit('Four genuine registered and aligned agents subscribed with independent Ed25519 keys', {
      agents: actors.map(actor => ({ server: actor.identity.url, pseudonym: actor.identity.pseudonym })), scope: 'ReflectionSummariesOnly',
    });

    for (const [source, local, remote, remoteObserver] of [
      [hostAuthor, hostObserver, vmAuthor, vmObserver], [vmAuthor, vmObserver, hostAuthor, hostObserver],
    ]) {
      const bundle = make(source, `${source.node.label} to independent local and federated recipients`);
      const response = await publish(source, bundle);
      assert(response.delivered_to >= 1, 'Actual local subscriber must receive the bundle');
      await received(local, bundle, source);
      await received(remote, bundle, source);
      await received(remoteObserver, bundle, source);
      for (const node of nodes) await stored(node, bundle);
      emit('Exact local and federated bundle content plus author signature verified', {
        bundleId: bundle.bundle_id, source: source.identity.url, canonicalSha256: sha256(authorSigningPayload(bundle)).toString('hex'),
        independentStoredBytes: Boolean(readStoredBundle),
      });
      await request(source.identity.url, '/api/rtx/publish', { method: 'POST', token: source.identity.token, body: bundle, expected: 409 });
      for (const recipient of [local, remote, remoteObserver]) {
        assert.equal(recipient.connection.frames.filter(frame => frame.type === 'rtx_bundle' && frame.bundle?.bundle_id === bundle.bundle_id).length, 1);
      }
    }

    const tamperChanges = {
      bundle_id: () => randomUUID(), summary: value => `${value} altered`,
      reasoning_chain: () => 'Injected unsanctioned content.', caveats: () => ['Altered caveat.'],
      domain_tags: () => [tag, 'altered'], created_at: value => value + 1,
      vrp_handshake_ref: value => `${value}:altered`, signature: () => '00'.repeat(64),
    };
    for (const [name, alter] of Object.entries(tamperChanges)) {
      const bundle = make(hostAuthor, `author integrity ${name}`);
      const tampered = { ...bundle, [name]: alter(bundle[name]) };
      assert.equal(authorSignatureValid(tampered, hostAuthor.key), false);
      await assertRejected(hostAuthor, tampered, 401);
    }
    await assertRejected(hostAuthor, make(hostAuthor, 'author cannot impersonate another agent', { source_pseudonym: hostObserver.identity.pseudonym }), 403);
    await assertRejected(hostAuthor, make(hostAuthor, 'author cannot claim another source server', { source_server: vm.founder.url }), 400);
    emit('Content tampering and forged signature return HTTP 401; mismatched author/server return 403/400 without persistence', { fieldsReturning401: Object.keys(tamperChanges) });
    await assertRejected(hostAuthor, make(hostAuthor, 'invented accepted decision', { vrp_handshake_ref: 'vrp:999999:999999' }), 403);
    await assertRejected(hostAuthor, make(hostAuthor, 'another author accepted decision', { vrp_handshake_ref: hostObserver.handshakeRef }), 403);
    const staleDecision = make(hostAuthor, 'previous accepted decision after real rehandshake');
    const previousRef = hostAuthor.handshakeRef;
    await align(hostAuthor, [], false);
    assert.notEqual(hostAuthor.handshakeRef, previousRef);
    await assertRejected(hostAuthor, staleDecision, 403);
    const missingKeyAttempt = make(hostAuthor, 'rehandshake cannot erase author signature requirement');
    missingKeyAttempt.signature = '00'.repeat(64);
    await assertRejected(hostAuthor, missingKeyAttempt, 401);
    emit('Real rehandshake without signingPubkey preserves author-signature enforcement');
    emit('Invented, another-author and stale accepted handshake references return 403 despite valid author signatures');

    await subscribe(hostObserver, [`${tag}-excluded`], true);
    const localFiltered = make(hostAuthor, 'local domain filter');
    await publish(hostAuthor, localFiltered);
    await assertBlocked(hostObserver, localFiltered, vmObserver);
    const remoteFiltered = make(vmAuthor, 'federated domain filter');
    await publish(vmAuthor, remoteFiltered);
    await assertBlocked(hostObserver, remoteFiltered, hostAuthor);
    await subscribe(hostObserver, [tag], true);
    const filterRestored = make(vmAuthor, 'filter restored positive marker');
    await publish(vmAuthor, filterRestored);
    await received(hostObserver, filterRestored, vmAuthor);
    emit('Domain filter prevents both local and federated delivery, then genuine resubscribe restores delivery');

    await subscribe(hostObserver, [tag], false);
    const remoteDenied = make(vmAuthor, 'accept_federated false');
    await publish(vmAuthor, remoteDenied);
    await assertBlocked(hostObserver, remoteDenied, hostAuthor);
    const localAllowed = make(hostAuthor, 'local delivery still works while federation opt-out is active');
    await publish(hostAuthor, localAllowed);
    await received(hostObserver, localAllowed, hostAuthor);
    await subscribe(hostObserver, [tag], true);
    const federatedRestored = make(vmAuthor, 'federation opt-in positive marker');
    await publish(vmAuthor, federatedRestored);
    await received(hostObserver, federatedRestored, vmAuthor);
    emit('Federated opt-out blocks remote delivery while local delivery continues, and opt-in restores remote delivery');

    const restricted = `${tag}-restricted`;
    await align(hostAuthor, [restricted]);
    const restrictedFields = [
      { domain_tags: [restricted] }, { summary: `Content about ${restricted.toUpperCase()}.` },
      { reasoning_chain: `Known test content mentioning ${restricted}.` }, { caveats: [`Avoid ${restricted}.`] },
    ];
    for (const content of restrictedFields) await assertRejected(hostAuthor, make(hostAuthor, 'restricted topic', content), 403);
    await align(hostAuthor, []);
    const redactionRestored = make(hostAuthor, 'new actual handshake removes redaction restriction', { domain_tags: [tag, restricted] });
    await publish(hostAuthor, redactionRestored);
    await received(vmObserver, redactionRestored, hostAuthor);
    emit('Actual capability-contract rehandshake blocks redacted tags, summary, reasoning and caveats with HTTP 403');

    const chain = 'Controlled arithmetic explanation: seven plus nineteen equals twenty-six.';
    const scoped = make(hostAuthor, 'negotiated summary-only transfer', { reasoning_chain: chain });
    await publish(hostAuthor, scoped);
    const delivered = { ...scoped, reasoning_chain: null };
    await received(hostObserver, delivered, hostAuthor, false);
    const remoteScoped = await received(vmObserver, delivered, hostAuthor, false);
    assert.equal(authorSignatureValid(scoped, hostAuthor.key), true);
    assert.equal(authorSignatureValid(delivered, hostAuthor.key), false,
      'A scope-redacted body cannot be claimed to verify as the original unmodified signed body');
    assert.equal(remoteScoped.provenance.origin.reasoning_commitment, sha256(chain).toString('hex'));
    const senderAudit = await stored(host, delivered);
    await stored(vm, delivered);
    assert(senderAudit.transfers.some(row => row.destination_pseudonym === null && row.redactions_applied === 'reasoning_chain_stripped'));
    emit('Negotiated ReflectionSummariesOnly strips known test reasoning from local and remote delivery and storage', {
      bundleId: scoped.bundle_id, originalAuthorSignatureVerified: true, rewrittenBodySignatureNotClaimedValid: true,
      reasoningCommitment: sha256(chain).toString('hex'), independentStoredBytes: Boolean(readStoredBundle),
    });

    const originalPolicies = nodes.map(node => ({ ...node.policy }));
    const changePolicy = async (node, policy) => {
      await request(node.founder.url, '/api/admin/policy', { method: 'PUT', token: node.founder.token, body: policy });
      const storedPolicy = await request(node.founder.url, '/api/admin/policy', { token: node.founder.token });
      assert.equal(storedPolicy.allow_full_knowledge, policy.allow_full_knowledge);
      node.policy = storedPolicy;
    };
    try {
      for (const node of nodes) await changePolicy(node, { ...node.policy, allow_full_knowledge: true });
      for (const actor of actors) await align(actor);
      for (const [source, local, remote] of [[hostAuthor, hostObserver, vmObserver], [vmAuthor, vmObserver, hostObserver]]) {
        const full = make(source, 'operator-authorized full knowledge', { reasoning_chain: 'Controlled full knowledge: seven plus nineteen equals twenty-six.' });
        await publish(source, full);
        await received(local, full, source);
        await received(remote, full, source);
        for (const node of nodes) await stored(node, full);
      }
      emit('Explicit operator policy opt-in negotiates FullKnowledgeBundle and preserves signed reasoning in both directions');
    } finally {
      for (const [index, node] of nodes.entries()) await changePolicy(node, originalPolicies[index]);
    }
    for (const actor of actors) await align(actor);
    const narrowed = make(hostAuthor, 'operator restored summary scope', { reasoning_chain: 'Controlled explanation removed after policy narrowing.' });
    await publish(hostAuthor, narrowed);
    await received(vmObserver, { ...narrowed, reasoning_chain: null }, hostAuthor, false);
    emit('Restoring conservative policy narrows actual subsequent transfer again');

    await request(hostObserver.identity.url, '/api/rtx/subscribe', { method: 'DELETE', token: hostObserver.identity.token });
    hostObserver.subscribed = false;
    assert.equal((await request(hostObserver.identity.url, '/api/rtx/subscriptions', { token: hostObserver.identity.token })).subscription, undefined);
    await request(hostObserver.identity.url, '/api/rtx/subscribe', { method: 'DELETE', token: hostObserver.identity.token, expected: 404 });
    const afterUnsubscribe = make(vmAuthor, 'unsubscribed receiver remains silent');
    await publish(vmAuthor, afterUnsubscribe);
    await assertBlocked(hostObserver, afterUnsubscribe, hostAuthor);
    await subscribe(hostObserver);
    const finalMarker = make(vmAuthor, 'resubscribe final ordered positive marker');
    await publish(vmAuthor, finalMarker);
    await received(hostObserver, finalMarker, vmAuthor);
    for (const [actor, deniedIds] of forbidden) for (const id of deniedIds) {
      assert.equal(bundleFrame(actor, id), undefined, 'Restricted messages must remain absent after subsequent real delivery barriers');
    }
    emit('Unsubscribe persists absence, repeated delete returns 404, and resubscribe delivers without replaying refused bundles');

    for (const node of nodes) {
      for (const bundle of published) await stored(node, expectedStored.get(bundle.bundle_id));
      const summary = await request(node.founder.url, '/api/rtx/governance/summary', { token: node.founder.token });
      assert(summary.unique_bundles >= node.baseline.unique_bundles + published.length);
      assert(summary.total_transfers > node.baseline.total_transfers);
      assert(summary.unique_sources >= 2);
      assert.equal(summary.by_scope.reduce((sum, item) => sum + item.count, 0), summary.total_transfers);
      const query = { source: hostAuthor.identity.pseudonym, limit: '1', offset: '0' };
      const first = await transfers(node, query);
      const second = await transfers(node, { ...query, offset: '1' });
      assert.equal(first.transfers.length, 1); assert.equal(second.transfers.length, 1);
      assert.equal(first.total, second.total); assert.notEqual(first.transfers[0].id, second.transfers[0].id);
      assert.equal(first.transfers[0].source_pseudonym, hostAuthor.identity.pseudonym);
      emit('Founder governance summary, source filter and pagination reflect actual transfers; agents denied access', {
        server: node.founder.url, totalTransfers: summary.total_transfers, uniqueBundles: summary.unique_bundles,
        scopes: summary.by_scope, redactedTransfers: summary.redacted_transfers,
      });
    }

    if (setPeerOnline && restartPeer) {
      const outgoing = async id => {
        const result = await request(host.founder.url, '/api/admin/federation/outbox?limit=500', { token: host.founder.token });
        return result.entries.find(row => row.message_id === `rtx:${id}` && row.peer_base_url === vm.founder.url);
      };
      const queued = make(hostAuthor, 'actual outage and process restart recovery', { reasoning_chain: 'Controlled arithmetic explanation stripped before durable relay.' });
      const expectedQueued = { ...queued, reasoning_chain: null };
      await setPeerOnline(vm.founder.url, false);
      try {
        await publish(hostAuthor, queued);
        await received(hostObserver, expectedQueued, hostAuthor, false);
        const failed = await until(async () => { const row = await outgoing(queued.bundle_id); return row?.status === 'failed' ? row : undefined; }, 'actual RTX transport failure retained in outbox', 120_000);
        assert(failed.attempts > 0);
        assert(failed.last_error);
        await restartPeer(host.founder.url);
        const preserved = await outgoing(queued.bundle_id);
        assert.equal(preserved.id, failed.id);
        assert.equal(preserved.status, 'failed');
        assert.equal(preserved.attempts, failed.attempts);
        assert.equal(preserved.envelope_bytes, failed.envelope_bytes);
        await setPeerOnline(vm.founder.url, true);
        for (const actor of actors) {
          actor.connection.ws.close();
          actor.connection = await socket(actor.identity);
          const subscription = await request(actor.identity.url, '/api/rtx/subscriptions', { token: actor.identity.token });
          assert.equal(subscription.subscription.subscriber_pseudonym, actor.identity.pseudonym);
        }
        await request(host.founder.url, `/api/admin/federation/outbox/${failed.id}/retry`, { method: 'POST', token: host.founder.token });
        await received(vmObserver, expectedQueued, hostAuthor, false);
        await received(vmAuthor, expectedQueued, hostAuthor, false);
        const deliveredRow = await until(async () => { const row = await outgoing(queued.bundle_id); return row?.status === 'delivered' ? row : undefined; }, 'RTX operator retry actual peer acknowledgement', 60_000);
        for (const node of nodes) await stored(node, expectedQueued);
        assert.equal(vmObserver.connection.frames.filter(frame => frame.type === 'rtx_bundle' && frame.bundle?.bundle_id === queued.bundle_id).length, 1);
        await request(host.founder.url, `/api/admin/federation/outbox/${failed.id}/retry`, { method: 'POST', token: host.founder.token, expected: 409 });
        durableRecovery = true;
        emit('Actual peer outage, durable failed row across publisher process restart, and operator retry deliver exactly once', {
          bundleId: queued.bundle_id, outboxId: failed.id, attemptsAfterRetry: deliveredRow.attempts,
        });
        if (readStoredOutboxEnvelope) {
          const storedEnvelope = await readStoredOutboxEnvelope(host.founder.url, failed.id);
          const wire = typeof storedEnvelope === 'string' ? JSON.parse(storedEnvelope) : storedEnvelope;
          assert.equal(wire.bundle.bundle_id, queued.bundle_id);
          assert.equal(wire.provenance.origin.version, 3);
          assert.equal(wire.bundle.reasoning_chain, null);
          assert.equal(wire.provenance.origin.reasoning_commitment, sha256(queued.reasoning_chain).toString('hex'));
          const before = await transfers(vm, { bundle_id: queued.bundle_id });
          const duplicate = await request(vm.founder.url, '/api/federation/rtx', { method: 'POST', body: wire });
          assert.equal(duplicate.delivered_to, 0);
          const after = await transfers(vm, { bundle_id: queued.bundle_id });
          assert.equal(after.total, before.total);
          for (const field of ['domain_tags', 'caveats']) {
            const changed = structuredClone(wire);
            changed.bundle[field].push(`unsigned-${field}`);
            await request(vm.founder.url, '/api/federation/rtx', { method: 'POST', body: changed, expected: 401 });
          }
          const marker = make(vmAuthor, 'ordered delivery after duplicate rejection');
          await publish(vmAuthor, marker);
          await received(vmObserver, marker, vmAuthor);
          await received(hostObserver, marker, vmAuthor);
          for (const node of nodes) await stored(node, marker);
          assert.equal(vmObserver.connection.frames.filter(frame => frame.type === 'rtx_bundle' && frame.bundle?.bundle_id === queued.bundle_id).length, 1);
          authenticatedRetry = true;
          emit('Actual signed envelope replay acknowledges without another delivery; tampered tags and caveats fail origin authentication', { bundleId: queued.bundle_id });
        }
      } finally {
        await setPeerOnline(vm.founder.url, true);
      }
    }
    for (const actor of actors) {
      assert.equal(actor.connection.ws.readyState, WebSocket.OPEN, 'All actual RTX sessions must remain connected through acceptance');
      assert.equal(actor.connection.frames.filter(frame => frame.type === 'error').length, 0, 'No unexpected WebSocket errors');
      await request(actor.identity.url, '/api/rtx/subscribe', { method: 'DELETE', token: actor.identity.token });
      actor.subscribed = false;
      assert.equal((await request(actor.identity.url, '/api/rtx/subscriptions', { token: actor.identity.token })).subscription, undefined);
    }
    emit('All four actual subscriptions removed and independently read back absent');
    return { completed: true, publishedBundleIds: published.map(bundle => bundle.bundle_id), independentStoredBytes: Boolean(readStoredBundle), durableRecovery, authenticatedRetry };
  } finally {
    // Do not swallow assertion failures; closing our sockets releases resources
    // without changing unrelated peer agreements or server policy.
    for (const actor of actors) actor.connection?.ws.close();
  }
}
